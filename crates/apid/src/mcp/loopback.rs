//! The loopback exemption of stdio servers on Windows. A server runs in an AppContainer there, and it reaches the hub's
//! egress proxy on loopback only once its container is exempt. Exempting it needs an administrator, so the owner
//! approves it once: the approval runs the hub's own binary elevated (`apid windows-loopback`) through Windows' UAC
//! prompt. Other platforms reach loopback without this, and [`host`] gives them no machine.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::{Value, json};

use super::config::McpServer;
use super::runtime::Runtime;
use crate::approvals::{Approval, State};
use crate::state::App;

/// The approval kind of a server that waits for the loopback exemption.
pub const KIND: &str = "mcp.loopback";

/// What the hub needs from the machine to exempt a container: the checks, which need no administrator, and the
/// elevated helper. Windows has the real one; tests use a fake.
pub trait Machine: Send + Sync {
    /// The SID of the AppContainer a server's workdir runs under, as text.
    fn container_sid(&self, workdir: &Path) -> Result<String, String>;
    /// Whether the container may reach loopback already.
    fn is_exempt(&self, sid: &str) -> Result<bool, String>;
    /// Whether Windows' administrator prompt can reach the owner from here.
    fn has_desktop(&self) -> bool;
    /// Runs the elevated helper that exempts `sid` and keeps its loopback to `port`.
    fn grant(&self, sid: &str, port: u16) -> Result<(), String>;
    /// Runs the elevated helper that takes the exemption and the filters away.
    fn revoke(&self, sid: &str) -> Result<(), String>;
}

/// A container that a server's start needs exempted before it can reach the proxy on `port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    pub sid: String,
    pub port: u16,
}

/// The machine of this hub: Windows' own on Windows, none elsewhere.
#[cfg(windows)]
pub fn host() -> Option<Arc<dyn Machine>> {
    Some(Arc::new(Elevated))
}

/// The machine of this hub: none, since sandboxed servers reach loopback on this platform.
#[cfg(not(windows))]
pub fn host() -> Option<Arc<dyn Machine>> {
    None
}

/// Windows: the sandbox's own checks, and the helper run through the UAC prompt.
#[cfg(windows)]
struct Elevated;

#[cfg(windows)]
impl Machine for Elevated {
    fn container_sid(&self, workdir: &Path) -> Result<String, String> {
        apz_sandbox::loopback::container_sid(workdir).map_err(|e| e.to_string())
    }

    fn is_exempt(&self, sid: &str) -> Result<bool, String> {
        apz_sandbox::loopback::is_exempt(sid).map_err(|e| e.to_string())
    }

    fn has_desktop(&self) -> bool {
        apz_sandbox::loopback::has_desktop()
    }

    fn grant(&self, sid: &str, port: u16) -> Result<(), String> {
        helper(&[
            "grant".to_string(),
            "--sid".to_string(),
            sid.to_string(),
            "--port".to_string(),
            port.to_string(),
        ])
    }

    fn revoke(&self, sid: &str) -> Result<(), String> {
        helper(&["revoke".to_string(), "--sid".to_string(), sid.to_string()])
    }
}

/// Runs `apid windows-loopback <args>` elevated, and waits for it. Its exit code is the answer.
#[cfg(windows)]
fn helper(args: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the hub's program: {e}"))?;
    let mut line = vec!["windows-loopback".to_string()];
    line.extend(args.iter().cloned());
    match apz_sandbox::loopback::run_elevated(&exe, &line) {
        Ok(0) => Ok(()),
        Ok(code) => Err(format!("the elevated helper stopped with exit code {code}")),
        Err(why) => Err(why.to_string()),
    }
}

/// Asks the owner for the loopback exemption when `outcome` is a start refused for want of it. A pending approval for
/// the server is reused, so there is at most one. The refusal then names that approval. Any other outcome passes.
pub fn asked<T>(app: &App, server: &McpServer, outcome: Result<T, String>) -> Result<T, String> {
    if outcome.is_ok() {
        return outcome;
    }
    let Some(need) = app.mcp.runtime.loopback_need(&server.id) else {
        return outcome;
    };
    let item = approval_for(
        &app.approvals.list(),
        &server.id,
        &need,
        |subject, detail| crate::approvals::submit(app, KIND, subject, detail),
    );
    let why = format!(
        "{} cannot reach the egress proxy until Windows administrator approval is given: approval {} waits for the owner",
        server.id, item.id
    );
    app.mcp.runtime.set_error(&server.id, &why);
    Err(why)
}

/// The owner's approval for `need` on `server`: the pending one when the list has it, else the one `submit` queues.
pub fn approval_for(
    list: &[Approval],
    server: &str,
    need: &Need,
    submit: impl FnOnce(&str, Value) -> Approval,
) -> Approval {
    if let Some(item) = list.iter().find(|item| pending(item, server)) {
        return item.clone();
    }
    let (subject, detail) = request(server, need);
    submit(&subject, detail)
}

/// Whether `item` is a loopback approval for `server` that the owner has not decided.
fn pending(item: &Approval, server: &str) -> bool {
    item.kind == KIND
        && item.state == State::Pending
        && item.detail["server"].as_str() == Some(server)
}

/// The subject and detail of the approval: the server, the one port it may reach, and what approving does.
pub fn request(server: &str, need: &Need) -> (String, Value) {
    let subject = format!("{server} needs Windows administrator approval once");
    let why = format!(
        "{server} needs Windows administrator approval once, so its sandbox can reach the hub's egress proxy on \
         port {} and only that port. Approving adds its container {} to the loopback exemption list, and a \
         firewall filter that blocks its other loopback ports.",
        need.port, need.sid
    );
    let detail = json!({
        "server": server,
        "sid": need.sid,
        "port": need.port,
        "why": why,
    });
    (subject, detail)
}

/// The owner's decision on a loopback approval (see `decide_approval`).
pub fn decided(app: &App, item: &Approval, approve: bool) -> Result<(), String> {
    let server = item.detail["server"]
        .as_str()
        .ok_or("the approval names no server")?;
    let sid = item.detail["sid"]
        .as_str()
        .ok_or("the approval names no container")?;
    let port = item.detail["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .ok_or("the approval names no port")?;
    let need = Need {
        sid: sid.to_string(),
        port,
    };
    settle(&app.mcp.runtime, server, &need, approve)
}

/// Applies the owner's decision to `server`. Approved, the elevated helper exempts the container and the server's next
/// start may run. Denied, nothing changes. A failure is kept as the server's error, and the server stays refused.
pub fn settle(runtime: &Runtime, server: &str, need: &Need, approve: bool) -> Result<(), String> {
    if !approve {
        runtime.set_error(
            server,
            "the owner declined the loopback approval, so the server stays refused",
        );
        return Ok(());
    }
    let granted = match runtime.loopback() {
        None => Err("this hub has no Windows loopback exemption to grant".to_string()),
        Some(machine) if !machine.has_desktop() => Err(format!(
            "no desktop session can show Windows' administrator prompt: approve from a desktop \
             session, or run `{}` from an elevated prompt",
            grant_command(need)
        )),
        Some(machine) => machine.grant(&need.sid, need.port),
    };
    match granted {
        Ok(()) => {
            runtime.reset(server);
            Ok(())
        }
        Err(why) => {
            runtime.set_error(server, &why);
            Err(why)
        }
    }
}

/// Before a server is deleted: takes its container's exemption away, if it has one. Best effort. A failure is logged,
/// and returned as the note the deletion's response carries.
pub fn revoke_for_delete(runtime: &Runtime, workdir: &Path) -> Option<String> {
    let machine = runtime.loopback()?;
    let sid = match machine.container_sid(workdir) {
        Ok(sid) => sid,
        Err(why) => return Some(note(&format!("its container is unknown: {why}"))),
    };
    match machine.is_exempt(&sid) {
        Ok(false) => return None,
        Ok(true) => {}
        Err(why) => return Some(note(&format!("its exemption is unknown: {why}"))),
    }
    let revoked = if machine.has_desktop() {
        machine.revoke(&sid)
    } else {
        Err("no desktop session can show Windows' administrator prompt".to_string())
    };
    match revoked {
        Ok(()) => None,
        Err(why) => Some(note(&format!(
            "its loopback exemption was not revoked ({why}): run `{}` from an elevated prompt",
            revoke_command(&sid)
        ))),
    }
}

/// The note of a deletion whose exemption could not be revoked. It is logged too.
fn note(why: &str) -> String {
    let note = format!("the server is deleted, but {why}");
    eprintln!("apid: {note}");
    note
}

/// Withdraws the pending loopback approvals of a deleted server: there is nothing left to exempt.
pub fn withdraw(app: &App, server: &str) {
    for item in app
        .approvals
        .list()
        .iter()
        .filter(|item| pending(item, server))
    {
        if let Some(item) = app.approvals.withdraw(&item.id, "the server was deleted") {
            app.bus.publish("approval", &item);
        }
    }
}

fn grant_command(need: &Need) -> String {
    format!(
        "apid windows-loopback grant --sid {} --port {}",
        need.sid, need.port
    )
}

fn revoke_command(sid: &str) -> String {
    format!("apid windows-loopback revoke --sid {sid}")
}

/// The elevated helper's arguments: `grant --sid <SID> --port <PORT>` or `revoke --sid <SID>`.
#[derive(Debug, PartialEq, Eq)]
pub enum Helper {
    Grant { sid: String, port: u16 },
    Revoke { sid: String },
}

/// `apid windows-loopback ...`: the helper the hub runs through the UAC prompt, or an owner runs from an elevated
/// prompt. Its exit code is the answer: 0 done, 1 failed, 2 wrong arguments.
pub fn helper_main(args: &[String]) -> ExitCode {
    let command = match parse_helper(args) {
        Ok(command) => command,
        Err(why) => {
            eprintln!(
                "apid windows-loopback: {why}\nusage: apid windows-loopback grant --sid <SID> --port <PORT> | revoke --sid <SID>"
            );
            return ExitCode::from(2);
        }
    };
    match run_helper(&command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("apid windows-loopback: {why}");
            ExitCode::from(1)
        }
    }
}

/// Reads the helper's arguments. Only AppContainer SIDs and a real port are accepted.
pub fn parse_helper(args: &[String]) -> Result<Helper, String> {
    let (verb, rest) = args.split_first().ok_or("say grant or revoke")?;
    let mut sid = None;
    let mut port = None;
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--sid" => sid = Some(value.clone()),
            "--port" => {
                port = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| format!("{value} is not a port"))?,
                )
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let sid = sid.ok_or("--sid is needed")?;
    if !sid.starts_with("S-1-15-2-") {
        return Err(format!("{sid} is not an AppContainer SID"));
    }
    match verb.as_str() {
        "grant" => match port {
            Some(port) if port != 0 => Ok(Helper::Grant { sid, port }),
            _ => Err("--port is needed, from 1 to 65535".to_string()),
        },
        "revoke" => Ok(Helper::Revoke { sid }),
        other => Err(format!("unknown command {other}")),
    }
}

#[cfg(windows)]
fn run_helper(command: &Helper) -> Result<(), String> {
    let result = match command {
        Helper::Grant { sid, port } => apz_sandbox::loopback::grant(sid, *port),
        Helper::Revoke { sid } => apz_sandbox::loopback::revoke(sid),
    };
    result.map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn run_helper(_: &Helper) -> Result<(), String> {
    Err("the loopback exemption exists only on Windows".to_string())
}
