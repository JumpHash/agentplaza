//! `agentplaza setup`: what a local plaza needs on this machine (Docker, its daemon, Postgres,
//! Forgejo, Garage, the services, the ports), and fixing the missing ones. A fix runs only after
//! the user confirms it, having seen what it will do. The checks and fixes are `apz-setup`'s; the
//! TUI's Dependencies step uses the same crate.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use apz_setup::{Check, Mode, Need, State};
use serde_json::{Value, json};

use crate::ctx::{self, Client};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Fix one requirement: services, docker, docker-running, postgres, forgejo, garage, hypervisor or ports.
    #[arg(long, value_name = "NEED", conflicts_with = "fix_all")]
    fix: Option<Need>,
    /// Fix every missing requirement that has a fix, in the order of the checklist.
    #[arg(long)]
    fix_all: bool,
    /// Also check the hub the way the web and the desktop Setup do: reachable, identity, service, node,
    /// network, machine, vault, model providers, default model and git host. `--hub NAME` checks the
    /// saved hub (or URL) NAME; a bare `--hub` checks the hub the global `--hub` names, else the default.
    #[arg(long, num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["fix", "fix_all"])]
    hub: Option<String>,
}

pub fn run(
    args: Args,
    json: bool,
    hub_flag: Option<String>,
    token: Option<String>,
    pin: Option<String>,
) -> Result<(), String> {
    let checks = apz_setup::check_all(Mode::Local);
    let plan = match (args.fix, args.fix_all) {
        (Some(need), _) => select(&checks, Some(need))?,
        (None, true) => select(&checks, None)?,
        (None, false) => {
            let hub_rows = match args.hub.as_deref() {
                None => None,
                Some("") => Some(hub_checks(
                    &ctx::resolve(hub_flag, token, pin, json)?.client,
                )),
                Some(named) => Some(hub_checks(
                    &ctx::resolve(Some(named.to_string()), token, pin, json)?.client,
                )),
            };
            match (json, hub_rows) {
                (true, None) => println!("{}", to_json(&checks)),
                (true, Some(rows)) => println!(
                    "{}",
                    json!({ "local": to_json(&checks), "hub": hub_to_json(&rows) })
                ),
                (false, rows) => {
                    print_checks(&checks);
                    if let Some(rows) = rows {
                        println!();
                        println!("hub");
                        print_hub(&rows);
                    }
                }
            }
            return Ok(());
        }
    };
    if plan.is_empty() {
        println!("nothing to fix: every requirement that can be fixed here is in place");
        return Ok(());
    }
    if !confirm(&plan)? {
        println!("cancelled: nothing was changed");
        return Ok(());
    }
    for check in &plan {
        println!("== {}", check.label);
        apz_setup::fix(check.need, &mut |line| println!("   {line}"))
            .map_err(|e| format!("{}: {e}", check.label))?;
    }
    println!();
    print_checks(&apz_setup::check_all(Mode::Local));
    Ok(())
}

/// What a fix run would do: the one requirement asked for (it must be missing and have a fix, else
/// the reason it cannot be fixed here), or with `None` every fixable one, in the checklist's order.
pub fn select(checks: &[Check], only: Option<Need>) -> Result<Vec<Check>, String> {
    let Some(need) = only else {
        return Ok(apz_setup::fixable(checks));
    };
    let check = checks
        .iter()
        .find(|check| check.need == need)
        .ok_or_else(|| format!("{} is not checked on this machine", need.name()))?;
    match check.state {
        State::Ok => Err(format!("{} is already set up", check.label)),
        State::Missing if check.fix.is_some() => Ok(vec![check.clone()]),
        State::Missing => Err(format!("{}: {}", check.label, check.detail)),
        State::Blocked | State::Warning => {
            Err(format!("{} needs you: {}", check.label, check.detail))
        }
    }
}

/// The lines that say what a plan will run: each requirement, and what its fix does, in words.
pub fn plan_lines(plan: &[Check]) -> Vec<String> {
    plan.iter()
        .map(|check| format!("  {}: {}", check.label, check.fix.as_deref().unwrap_or("")))
        .collect()
}

/// Whether an answer confirms the plan: `y` or `yes`, in any case. Anything else, Enter included,
/// declines.
pub fn confirms(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Shows the plan and asks for a y/N answer on the terminal. Without a terminal nothing runs.
fn confirm(plan: &[Check]) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err("not on a terminal, so the fixes were not run: run this again in a terminal to confirm them".into());
    }
    println!("This will run:");
    for line in plan_lines(plan) {
        println!("{line}");
    }
    print!("Run these now? [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(confirms(&answer))
}

/// The state of one hub check, as the web's and the desktop's Setup word it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubState {
    Ok,
    Warning,
    Failed,
    /// Not checkable from here (the hub did not answer for it, or it needs the hub to be unsealed first).
    Pending,
}

impl HubState {
    fn word(self) -> &'static str {
        match self {
            HubState::Ok => "OK",
            HubState::Warning => "WARNING",
            HubState::Failed => "FAILED",
            HubState::Pending => "PENDING",
        }
    }
}

/// One check of the hub: its label, state, what it found, and the fix a warning or failure names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubCheck {
    pub label: &'static str,
    pub state: HubState,
    pub detail: String,
    pub fix: Option<String>,
}

impl HubCheck {
    fn new(
        label: &'static str,
        state: HubState,
        detail: impl Into<String>,
        fix: Option<&str>,
    ) -> HubCheck {
        HubCheck {
            label,
            state,
            detail: detail.into(),
            fix: fix.map(str::to_string),
        }
    }
}

/// The hub-side checks, read through the hub's API (the same reads as the web's Setup). A read that
/// failed is passed on as no answer, and its row says what that means.
pub fn hub_checks(client: &Client) -> Vec<HubCheck> {
    let mut client = client.clone();
    client.timeout = Duration::from_secs(10);
    let overview = client.get("/v1/overview");
    let identity = client.get("/v1/hub/identity").ok();
    let machine = client.get("/v1/machine").ok();
    let vault = client.get("/v1/vault").ok();
    let providers = client.get("/v1/providers").ok();
    let git = client.get("/v1/git").ok();
    let link = client.get("/v1/node-link").ok();

    let mut rows = vec![reachable_row(&overview), identity_row(identity.as_ref())];
    if let Ok(over) = &overview {
        rows.push(service_row(over));
        rows.push(node_row(over));
    }
    rows.push(network_row(link.as_ref()));
    rows.push(machine_row(machine.as_ref()));
    rows.push(vault_row(vault.as_ref()));
    rows.push(providers_row(providers.as_ref()));
    if let Ok(over) = &overview {
        rows.push(default_model_row(over));
    }
    rows.push(git_row(git.as_ref()));
    rows
}

fn count(v: Option<&Value>) -> u64 {
    v.and_then(Value::as_u64).unwrap_or(0)
}

fn plural(n: u64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn reachable_row(overview: &Result<Value, String>) -> HubCheck {
    match overview {
        Ok(over) => HubCheck::new(
            "Hub reachable",
            HubState::Ok,
            format!(
                "apid answers, {} on this plaza",
                plural(count(over.pointer("/agents/total")), "agent")
            ),
            None,
        ),
        // A hub that wants the owner signed in answers with that refusal, so it is reachable.
        Err(e) if e.contains("sign in") => HubCheck::new(
            "Hub reachable",
            HubState::Warning,
            "the hub wants the owner signed in",
            Some("sign in with the owner token: agentplaza hub login <url> --token <token>"),
        ),
        Err(_) => HubCheck::new(
            "Hub reachable",
            HubState::Failed,
            "apid does not answer",
            Some(
                "start AgentPlaza on this machine (desktop app: Start locally), then run the checks again",
            ),
        ),
    }
}

fn identity_row(identity: Option<&Value>) -> HubCheck {
    let Some(id) = identity else {
        return HubCheck::new(
            "Hub identity",
            HubState::Failed,
            "the hub does not say who it is",
            None,
        );
    };
    let who = format!("{} {}", ctx::s(id, "name"), ctx::s(id, "version"));
    let https = id
        .get("fingerprint")
        .and_then(Value::as_str)
        .is_some_and(|f| !f.is_empty());
    let nodes = count(id.get("nodes"));
    if https || nodes == 0 {
        let scheme = if https { "HTTPS" } else { "plain HTTP" };
        HubCheck::new(
            "Hub identity",
            HubState::Ok,
            format!("{who}, {scheme}"),
            None,
        )
    } else {
        HubCheck::new(
            "Hub identity",
            HubState::Warning,
            format!("{who}, plain HTTP with {} joined", plural(nodes, "node")),
            Some("other machines should join over HTTPS: run the hub with TLS"),
        )
    }
}

fn service_row(over: &Value) -> HubCheck {
    if flag(over, "hub") {
        HubCheck::new(
            "Hub service",
            HubState::Ok,
            "the hub serves the console and the API",
            None,
        )
    } else {
        HubCheck::new(
            "Hub service",
            HubState::Failed,
            "the hub service is not running",
            Some("start the services (desktop app: Start locally)"),
        )
    }
}

fn node_row(over: &Value) -> HubCheck {
    if flag(over, "noded") {
        HubCheck::new("Node here", HubState::Ok, "this machine runs agents", None)
    } else {
        HubCheck::new(
            "Node here",
            HubState::Warning,
            "no node on this machine: agents cannot run here",
            Some(
                "add this machine as a node: agentplaza nodes code, then agentplaza node join <code> on the machine",
            ),
        )
    }
}

fn network_row(link: Option<&Value>) -> HubCheck {
    let Some(net) = link else {
        return HubCheck::new(
            "Network",
            HubState::Warning,
            "could not read the node link",
            Some("see `agentplaza nodes list`"),
        );
    };
    if !flag(net, "joined") {
        return HubCheck::new(
            "Network",
            HubState::Ok,
            "this hub runs its own network",
            None,
        );
    }
    let hub = match ctx::s(net, "hub") {
        "" => "another hub",
        hub => hub,
    };
    let running = flag(net, "running");
    let detail = format!(
        "joined {hub}, node {}",
        if running { "running" } else { "stopped" }
    );
    if running {
        HubCheck::new("Network", HubState::Ok, detail, None)
    } else {
        HubCheck::new(
            "Network",
            HubState::Warning,
            detail,
            Some("start the node on that machine: agentplaza node start"),
        )
    }
}

fn machine_row(machine: Option<&Value>) -> HubCheck {
    let Some(m) = machine else {
        return HubCheck::new(
            "Machine and hypervisor",
            HubState::Pending,
            "the hub did not report the machine; agentplaza setup checks the hypervisor here",
            None,
        );
    };
    let backend = ctx::s(m, "backend");
    let detail = format!(
        "{} on {}, {} CPUs, {} MiB",
        if backend.is_empty() {
            "no hypervisor backend"
        } else {
            backend
        },
        ctx::s(m, "arch"),
        ctx::text(m, "cpus"),
        ctx::text(m, "memory_mib"),
    );
    if backend.is_empty() {
        HubCheck::new(
            "Machine and hypervisor",
            HubState::Warning,
            detail,
            Some("turn on virtualization in the firmware: agents run in VMs"),
        )
    } else {
        HubCheck::new("Machine and hypervisor", HubState::Ok, detail, None)
    }
}

fn vault_row(vault: Option<&Value>) -> HubCheck {
    const LABEL: &str = "Vault";
    let Some(v) = vault else {
        return HubCheck::new(
            LABEL,
            HubState::Warning,
            "could not read the vault",
            Some("see `agentplaza vault status`"),
        );
    };
    match ctx::s(v, "state") {
        "uninitialized" => HubCheck::new(
            LABEL,
            HubState::Warning,
            "not created yet: secrets have no encrypted home",
            Some("open the Vault in the desktop app or the web console"),
        ),
        "sealed" => HubCheck::new(
            LABEL,
            HubState::Warning,
            "sealed: agents cannot read secrets until it is unsealed",
            Some("agentplaza vault unseal"),
        ),
        "tampered" => {
            let why = match ctx::s(v, "tampered") {
                "" => String::new(),
                reason => format!(": {reason}"),
            };
            HubCheck::new(
                LABEL,
                HubState::Failed,
                format!("tampered{why}"),
                Some("read the audit log and recover: agentplaza vault audit"),
            )
        }
        "unsealed" => {
            let env_keys = count(v.get("env_file_keys"));
            if env_keys > 0 {
                return HubCheck::new(
                    LABEL,
                    HubState::Warning,
                    format!(
                        "{} still in the env file, not in the vault",
                        plural(env_keys, "key")
                    ),
                    Some("agentplaza vault import"),
                );
            }
            match v.get("audit_ok") {
                None | Some(Value::Null) => HubCheck::new(
                    LABEL,
                    HubState::Pending,
                    "audit chain not checked until unsealed",
                    None,
                ),
                Some(Value::Bool(false)) => HubCheck::new(
                    LABEL,
                    HubState::Warning,
                    "unsealed, but the audit chain does not verify",
                    Some("agentplaza vault audit"),
                ),
                Some(_) => {
                    let slots = v
                        .get("slots")
                        .and_then(Value::as_array)
                        .map_or(0, |s| s.len() as u64);
                    let detail = format!(
                        "unsealed: {}, {}",
                        plural(count(v.get("entries")), "secret"),
                        plural(slots, "unlock slot")
                    );
                    HubCheck::new(LABEL, HubState::Ok, detail, None)
                }
            }
        }
        other => HubCheck::new(
            LABEL,
            HubState::Warning,
            format!(
                "unknown state {}",
                if other.is_empty() { "-" } else { other }
            ),
            None,
        ),
    }
}

fn providers_row(providers: Option<&Value>) -> HubCheck {
    const LABEL: &str = "Model providers";
    let Some(list) = providers.and_then(Value::as_array) else {
        return HubCheck::new(
            LABEL,
            HubState::Warning,
            "could not list the providers",
            Some("see `agentplaza models providers`"),
        );
    };
    if list.is_empty() {
        return HubCheck::new(
            LABEL,
            HubState::Failed,
            "none: agents cannot call a model",
            Some("add a provider and its key: agentplaza models add"),
        );
    }
    let missing: Vec<&str> = list
        .iter()
        .filter(|p| ctx::s(p, "credential") == "missing")
        .map(|p| ctx::s(p, "id"))
        .collect();
    if missing.is_empty() {
        HubCheck::new(
            LABEL,
            HubState::Ok,
            format!("{}, every key set", plural(list.len() as u64, "provider")),
            None,
        )
    } else {
        HubCheck::new(
            LABEL,
            HubState::Warning,
            format!(
                "{} without a key (of {})",
                missing.join(", "),
                plural(list.len() as u64, "provider")
            ),
            Some("set the key for each one: agentplaza models key"),
        )
    }
}

fn default_model_row(over: &Value) -> HubCheck {
    match ctx::s(over, "default_model") {
        "" => HubCheck::new(
            "Default model",
            HubState::Warning,
            "none set: agents have no model by default",
            Some("pick a default model: agentplaza models --help"),
        ),
        model => HubCheck::new("Default model", HubState::Ok, model, None),
    }
}

fn git_row(git: Option<&Value>) -> HubCheck {
    let Some(g) = git else {
        return HubCheck::new(
            "Git host",
            HubState::Warning,
            "could not ask the git host",
            None,
        );
    };
    if flag(g, "configured") {
        HubCheck::new(
            "Git host",
            HubState::Ok,
            format!("org {}", ctx::s(g, "org")),
            None,
        )
    } else {
        HubCheck::new(
            "Git host",
            HubState::Warning,
            "the git host is not configured",
            Some("set FORGEJO_URL and FORGEJO_TOKEN for the hub, then restart apid"),
        )
    }
}

fn print_hub(rows: &[HubCheck]) {
    let width = rows.iter().map(|row| row.label.len()).max().unwrap_or(0);
    for row in rows {
        println!(
            "{:<width$}  {:<8}  {}",
            row.label,
            row.state.word(),
            row.detail
        );
        if let Some(fix) = &row.fix {
            println!("{:<width$}  {:<8}  fix: {fix}", "", "");
        }
    }
}

fn hub_to_json(rows: &[HubCheck]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| {
                json!({
                    "label": row.label,
                    "state": row.state.word().to_ascii_lowercase(),
                    "detail": row.detail,
                    "fix": row.fix,
                })
            })
            .collect(),
    )
}

fn word(state: State) -> &'static str {
    match state {
        State::Ok => "OK",
        State::Missing => "MISSING",
        State::Blocked => "BLOCKED",
        State::Warning => "WARNING",
    }
}

fn print_checks(checks: &[Check]) {
    let width = checks
        .iter()
        .map(|check| check.label.len())
        .max()
        .unwrap_or(0);
    for check in checks {
        let optional = if check.required { "" } else { " (optional)" };
        println!(
            "{:<width$}  {:<8}  {}{optional}",
            check.label,
            word(check.state),
            check.detail
        );
        if check.fix.is_some() {
            println!(
                "{:<width$}  {:<8}  fix: agentplaza setup --fix {}",
                "",
                "",
                check.need.name()
            );
        }
    }
}

fn to_json(checks: &[Check]) -> Value {
    Value::Array(
        checks
            .iter()
            .map(|check| {
                json!({
                    "need": check.need.name(),
                    "label": check.label,
                    "state": word(check.state).to_ascii_lowercase(),
                    "detail": check.detail,
                    "fix": check.fix,
                    "required": check.required,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn row(need: Need, state: State, fix: Option<&str>) -> Check {
        Check {
            need,
            label: need.name(),
            state,
            detail: format!("{} detail", need.name()),
            fix: fix.map(str::to_string),
            required: true,
        }
    }

    #[test]
    fn only_y_and_yes_confirm() {
        for yes in ["y", "Y", "yes", "YES", " y \n", "Yes\r\n"] {
            assert!(confirms(yes), "{yes:?} confirms");
        }
        for no in ["", "\n", "n", "N", "no", "yess", "sure", "y es"] {
            assert!(!confirms(no), "{no:?} declines");
        }
    }

    #[test]
    fn a_plan_names_each_requirement_and_what_its_fix_does() {
        let plan = vec![row(
            Need::Postgres,
            State::Missing,
            Some("Create the container agentplaza-postgres"),
        )];
        assert_eq!(
            plan_lines(&plan),
            vec!["  postgres: Create the container agentplaza-postgres".to_string()]
        );
    }

    #[test]
    fn fix_one_needs_a_missing_requirement_with_a_fix() {
        let checks = vec![
            row(Need::Docker, State::Ok, None),
            row(Need::Postgres, State::Missing, Some("Create it")),
            row(Need::Forgejo, State::Missing, None),
            row(Need::Garage, State::Blocked, None),
            row(Need::Hypervisor, State::Warning, None),
        ];
        let picked = select(&checks, Some(Need::Postgres)).unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].need, Need::Postgres);

        let err = select(&checks, Some(Need::Docker)).unwrap_err();
        assert_eq!(err, "docker is already set up");
        let err = select(&checks, Some(Need::Forgejo)).unwrap_err();
        assert_eq!(err, "forgejo: forgejo detail");
        let err = select(&checks, Some(Need::Garage)).unwrap_err();
        assert_eq!(err, "garage needs you: garage detail");
        let err = select(&checks, Some(Need::Hypervisor)).unwrap_err();
        assert_eq!(err, "hypervisor needs you: hypervisor detail");
        let err = select(&[], Some(Need::Ports)).unwrap_err();
        assert_eq!(err, "ports is not checked on this machine");
    }

    #[test]
    fn fix_all_takes_every_fixable_requirement_in_order_and_nothing_else() {
        let checks = vec![
            row(Need::Services, State::Blocked, None),
            row(Need::Docker, State::Missing, Some("Install Docker")),
            row(Need::DockerRunning, State::Missing, Some("Start Docker")),
            row(Need::Postgres, State::Missing, None),
            row(Need::Garage, State::Missing, Some("Create Garage")),
        ];
        let plan: Vec<Need> = select(&checks, None)
            .unwrap()
            .iter()
            .map(|c| c.need)
            .collect();
        assert_eq!(plan, vec![Need::Docker, Need::DockerRunning, Need::Garage]);
        assert!(select(&checks[..1], None).unwrap().is_empty());
    }

    #[test]
    fn the_flags_parse_and_conflict() {
        let parsed =
            crate::Cli::try_parse_from(["agentplaza", "setup", "--fix", "docker-running"]).unwrap();
        match parsed.command {
            crate::Command::Setup(args) => assert_eq!(args.fix, Some(Need::DockerRunning)),
            other => panic!("not setup: {other:?}"),
        }
        let parsed = crate::Cli::try_parse_from(["agentplaza", "setup", "--fix-all"]).unwrap();
        match parsed.command {
            crate::Command::Setup(args) => assert!(args.fix_all && args.fix.is_none()),
            other => panic!("not setup: {other:?}"),
        }
        assert!(crate::Cli::try_parse_from(["agentplaza", "setup", "--fix", "nope"]).is_err());
        assert!(
            crate::Cli::try_parse_from(["agentplaza", "setup", "--fix", "garage", "--fix-all"])
                .is_err()
        );
    }

    #[test]
    fn the_hub_flag_checks_the_hub_and_takes_no_fix() {
        let parsed = crate::Cli::try_parse_from(["agentplaza", "setup", "--hub"]).unwrap();
        match parsed.command {
            crate::Command::Setup(args) => {
                assert!(args.hub.as_deref() == Some("") && args.fix.is_none() && !args.fix_all)
            }
            other => panic!("not setup: {other:?}"),
        }
        let named = crate::Cli::try_parse_from(["agentplaza", "setup", "--hub", "work"]).unwrap();
        match named.command {
            crate::Command::Setup(args) => assert_eq!(args.hub.as_deref(), Some("work")),
            other => panic!("not setup: {other:?}"),
        }
        assert!(crate::Cli::try_parse_from(["agentplaza", "setup", "--hub", "--fix-all"]).is_err());
    }

    #[test]
    fn an_unreachable_hub_fails_and_a_signed_out_one_warns() {
        let refused: Result<Value, String> = Err("connection refused".into());
        assert_eq!(reachable_row(&refused).state, HubState::Failed);
        let signed_out: Result<Value, String> = Err("sign in with the owner token".into());
        let row = reachable_row(&signed_out);
        assert_eq!(row.state, HubState::Warning);
        assert!(row.fix.unwrap().contains("hub login"));
        let answered: Result<Value, String> = Ok(json!({ "agents": { "total": 3 } }));
        assert_eq!(
            reachable_row(&answered).detail,
            "apid answers, 3 agents on this plaza"
        );
    }

    #[test]
    fn a_plain_http_hub_with_nodes_is_a_warning() {
        let plain = json!({ "name": "lab", "version": "0.0.8", "fingerprint": null, "nodes": 2 });
        let row = identity_row(Some(&plain));
        assert_eq!(row.state, HubState::Warning);
        assert_eq!(row.detail, "lab 0.0.8, plain HTTP with 2 nodes joined");
        let tls = json!({ "name": "lab", "version": "0.0.8", "fingerprint": "abc", "nodes": 2 });
        assert_eq!(identity_row(Some(&tls)).detail, "lab 0.0.8, HTTPS");
        assert_eq!(identity_row(None).state, HubState::Failed);
    }

    #[test]
    fn the_vault_reads_its_state_in_the_web_order() {
        assert_eq!(
            vault_row(Some(&json!({ "state": "sealed" }))).state,
            HubState::Warning
        );
        assert_eq!(
            vault_row(Some(
                &json!({ "state": "tampered", "tampered": "bad chain" })
            ))
            .detail,
            "tampered: bad chain"
        );
        let loose = json!({ "state": "unsealed", "env_file_keys": 1, "audit_ok": true });
        assert_eq!(
            vault_row(Some(&loose)).detail,
            "1 key still in the env file, not in the vault"
        );
        let unchecked = json!({ "state": "unsealed", "env_file_keys": 0, "audit_ok": null });
        assert_eq!(vault_row(Some(&unchecked)).state, HubState::Pending);
        let ok = json!({ "state": "unsealed", "env_file_keys": 0, "audit_ok": true, "entries": 4, "slots": [{}] });
        let row = vault_row(Some(&ok));
        assert_eq!(row.state, HubState::Ok);
        assert_eq!(row.detail, "unsealed: 4 secrets, 1 unlock slot");
    }

    #[test]
    fn providers_name_the_ones_without_a_key() {
        assert_eq!(providers_row(Some(&json!([]))).state, HubState::Failed);
        let some_missing = json!([{ "id": "openai", "credential": "secret" }, { "id": "anthropic", "credential": "missing" }]);
        let row = providers_row(Some(&some_missing));
        assert_eq!(row.state, HubState::Warning);
        assert_eq!(row.detail, "anthropic without a key (of 2 providers)");
        let all = json!([{ "id": "openai", "credential": "secret" }]);
        assert_eq!(
            providers_row(Some(&all)).detail,
            "1 provider, every key set"
        );
    }

    #[test]
    fn the_network_and_the_default_model_say_what_they_found() {
        assert_eq!(
            network_row(Some(&json!({ "joined": false }))).detail,
            "this hub runs its own network"
        );
        let stopped = json!({ "joined": true, "hub": "other.example", "running": false });
        let row = network_row(Some(&stopped));
        assert_eq!(row.state, HubState::Warning);
        assert_eq!(row.detail, "joined other.example, node stopped");
        assert_eq!(
            default_model_row(&json!({ "default_model": "" })).state,
            HubState::Warning
        );
        assert_eq!(
            default_model_row(&json!({ "default_model": "glm-5" })).detail,
            "glm-5"
        );
    }

    #[test]
    fn the_hub_rows_go_out_as_json_with_their_fixes() {
        let rows = vec![HubCheck::new(
            "Git host",
            HubState::Warning,
            "not configured",
            Some("set it"),
        )];
        let value = hub_to_json(&rows);
        assert_eq!(value[0]["state"], "warning");
        assert_eq!(value[0]["fix"], "set it");
    }
}
