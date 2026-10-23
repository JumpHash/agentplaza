//! The scratch stack a task runs on: a fresh `apid` (its own state folder, ports and empty env file), `vsockd` serving
//! the guest CIDs on a Unix socket each, and the stand-in noded that starts the harnesses. Everything is in the
//! scratch folder; nothing the owner has on this machine is read.

use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::Options;
use crate::noded::{self, Harness, Noded};

/// How long the stack may take to come up, and each daemon's readiness.
const STARTUP: Duration = Duration::from_secs(30);

/// The binaries the stack runs, in the folder `--bin-dir` names.
pub struct Binaries {
    pub apid: PathBuf,
    pub vsockd: PathBuf,
    pub harness: PathBuf,
}

impl Binaries {
    pub fn in_dir(dir: &Path) -> Binaries {
        Binaries {
            apid: dir.join("apid"),
            vsockd: dir.join("vsockd"),
            harness: dir.join("agent-harness"),
        }
    }
}

/// A running stack. Dropped, it stops its daemons and its VMs.
pub struct Stack {
    /// apid's public port (the owner's calls go here).
    pub apid_port: u16,
    children: Vec<Child>,
    /// The stand-in noded: kept for its harnesses, which stop when it is dropped.
    _noded: Noded,
}

impl Stack {
    /// Starts the stack in `scratch`, with the mock script `script` and the roles manifest `roles`.
    pub fn start(
        options: &Options,
        bins: &Binaries,
        scratch: &Path,
        script: &Path,
        roles: &Path,
    ) -> Result<Stack, String> {
        let state = scratch.join("state");
        let config = scratch.join("config");
        let uds = scratch.join("uds");
        let logs = scratch.join("logs");
        let home = scratch.join("home");
        for dir in [&state, &config, &uds, &logs, &home] {
            fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        }
        let env_file = match &options.env_file {
            Some(path) => path.clone(),
            None => {
                let path = scratch.join("empty.env");
                fs::write(&path, "").map_err(|err| format!("{}: {err}", path.display()))?;
                path
            }
        };
        let (apid_port, hub_port, noded_port) = (free_port()?, free_port()?, free_port()?);

        let noded = Noded::serve(
            noded_port,
            Harness {
                binary: bins.harness.clone(),
                uds_dir: uds.clone(),
                logs: logs.clone(),
            },
        )
        .map_err(|err| format!("the stand-in noded: {err}"))?;

        let mut stack = Stack {
            apid_port,
            children: Vec::new(),
            _noded: noded,
        };
        let apid = Command::new(&bins.apid)
            .arg("--port")
            .arg(apid_port.to_string())
            .arg("--hub-port")
            .arg(hub_port.to_string())
            .arg("--noded-port")
            .arg(noded_port.to_string())
            .arg("--state-dir")
            .arg(&state)
            .arg("--vault-key")
            .arg(config.join("vault.key"))
            .arg("--env-file")
            .arg(&env_file)
            .arg("--roles")
            .arg(roles)
            .arg("--prompts")
            .arg(scratch.join("no-prompts"))
            .arg("--console")
            .arg(scratch.join("no-console"))
            .env("HOME", &home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("APZ_SHARE", scratch.join("no-share"))
            .env("APZ_MOCK_SCRIPT", script)
            .stdin(Stdio::null())
            .stdout(log_file(&logs.join("apid.log"))?)
            .stderr(log_file(&logs.join("apid.err.log"))?)
            .spawn()
            .map_err(|err| format!("cannot start apid: {err}"))?;
        stack.children.push(apid);
        wait_for_log(&logs.join("apid.log"), "apid: listening on")?;

        let cids: Vec<String> = (0..20)
            .map(|i| (noded::FIRST_CID + i).to_string())
            .collect();
        let vsockd = Command::new(&bins.vsockd)
            .arg("--uds-dir")
            .arg(&uds)
            .arg("--cids")
            .arg(cids.join(","))
            .arg("--port")
            .arg(noded::PORT.to_string())
            .arg("--http-port")
            .arg(hub_port.to_string())
            .arg("--apid")
            .arg(format!("http://127.0.0.1:{apid_port}"))
            .arg("--internal-token-file")
            .arg(state.join("internal.token"))
            .arg("--egress")
            .arg("off")
            .stdin(Stdio::null())
            .stdout(log_file(&logs.join("vsockd.log"))?)
            .stderr(log_file(&logs.join("vsockd.err.log"))?)
            .spawn()
            .map_err(|err| format!("cannot start vsockd: {err}"))?;
        stack.children.push(vsockd);
        let socket = uds.join(format!("vm{}_{}", noded::FIRST_CID, noded::PORT));
        wait_until(|| socket.exists(), "vsockd's socket")?;
        wait_until(
            || {
                apz_http::client::send(hub_port, "GET", "/v1/status", None, Duration::from_secs(2))
                    .is_ok_and(|reply| reply.status == 200)
            },
            "vsockd's HTTP surface",
        )?;
        Ok(stack)
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A free loopback port: bound once to learn it, then released for the daemon to take.
fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| format!("a port: {err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("a port: {err}"))?
        .port();
    Ok(port)
}

fn log_file(path: &Path) -> Result<Stdio, String> {
    let file: File = File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Stdio::from(file))
}

/// Waits until the log at `path` says `line`.
fn wait_for_log(path: &Path, line: &str) -> Result<(), String> {
    wait_until(
        || fs::read_to_string(path).is_ok_and(|text| text.contains(line)),
        line,
    )
}

/// Waits until `ready` holds, for at most [`STARTUP`]: an error naming `what` otherwise.
pub fn wait_until(mut ready: impl FnMut() -> bool, what: &str) -> Result<(), String> {
    let deadline = Instant::now() + STARTUP;
    loop {
        if ready() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the stack did not come up: no {what} within {STARTUP:?}"
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}
