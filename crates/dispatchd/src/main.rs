//! `dispatchd` — the fleet dispatcher's entry point.
//!
//! ```text
//! dispatchd [--noded URL] [--hub URL] [--roles MANIFEST] [--once] [--interval-secs N]
//! ```
//!
//! Each pass reads the ledger and the fleet register, claims every open task
//! whose role has a connected agent and a script the role's grants allow
//! (`--roles`), drives that agent through the planned workflow, and finishes the
//! task with the artifacts its workspace now holds — or, for a read-only
//! workflow, with the line the script read out — or with the reason it could
//! not. See [`dispatchd`] (the library) for what a pass does and
//! [`dispatchd::dispatch`] for the retry and finish semantics.
//!
//! `--once` runs exactly one pass and exits, which is the shape a test or a
//! cron wants: it reports a failure the pass could not read the ledger, so a
//! one-shot run's exit status is worth something. Otherwise the daemon runs
//! pass, interval, pass until it is signalled — `SIGTERM`/`SIGINT` end it the
//! ordinary way, and it installs no handler of its own, so a stop is
//! immediate.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dispatchd::roles::Manifest;
use dispatchd::{Dispatcher, Hub, Noded, Social, StdoutLog};

/// The node daemon's loopback address unless `--noded` says otherwise.
const DEFAULT_NODED: &str = "http://127.0.0.1:8899";

/// The hub's admin surface unless `--hub` says otherwise.
const DEFAULT_HUB: &str = "http://127.0.0.1:19081";

/// How long to wait between passes unless `--interval-secs` says otherwise.
const DEFAULT_INTERVAL_SECS: u64 = 5;

const USAGE: &str = "\
usage: dispatchd [--noded URL] [--hub URL] [--roles MANIFEST] [--once]
                 [--interval-secs N]

  --noded URL        the node daemon (the ledger and the fleet register)
                     (default http://127.0.0.1:8899)
  --hub URL          the hub's admin surface (POST /tell — the way into an
                     agent's session) (default http://127.0.0.1:19081)
  --roles MANIFEST   the roles manifest the codex's tool grants are read from
                     (docs/agents/initial-roles.json): a task whose role cannot
                     be driven with the tools its family's script needs — a
                     librarian, whose codex grants give it `search` and nothing
                     else — runs the read-only workflow instead, and a role with
                     nothing drivable at all is left open and logged rather than
                     driven into a refusal. Default: off, and then every role
                     drives its family's script.
  --once             run exactly one pass and exit
  --interval-secs N  how long to wait between passes when looping (default 5,
                     at least 1)
  -h, --help         print this help

A pass reads GET {noded}/api/v0/tasks and GET {noded}/api/v0/hub, and for each
open task that names a role: decides what the role's grants allow (the
manifest's script, the read-only fallback, or nothing), finds a CONNECTED agent
of that role, claims the task for it (POST /api/v0/tasks/{id}/claim), drives it
through the planned workflow (one tool call at a time, through POST
{hub}/tell), and finishes it (POST /api/v0/tasks/{id}/finish) with the note
naming what the run left. A task whose role has no connected agent, or whose
role cannot be driven, is left open and logged. An agent that answers and
refuses finishes the task failed at once; an agent that cannot be reached is
retried for at most 3 passes, then finished failed with the evidence. Every URL
is a loopback `http://127.0.0.1:PORT` or `http://localhost:PORT`.

Every fifth pass is also a social pass, run first, over the same snapshot and
before any claim: one idle connected agent (the register walked round-robin,
agents with a claim in hand skipped) reads the board (POST
{noded}/api/v0/board/read up to the newest post) and, when a post names an open
task of that agent's role it has not already spoken for, posts a templated
interest note (POST {noded}/api/v0/board, subject `available for <task>`,
posted_by `dispatchd`) composed from the task's own words and the role's
grants. A social call that fails is logged and the pass moves on: the claims
are never blocked by the board. At the default 5 s interval a social pass is
about one every 25 s.";

/// Everything `dispatchd` accepts on its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    noded: String,
    hub: String,
    roles: Option<PathBuf>,
    once: bool,
    interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            noded: DEFAULT_NODED.to_string(),
            hub: DEFAULT_HUB.to_string(),
            roles: None,
            once: false,
            interval: Duration::from_secs(DEFAULT_INTERVAL_SECS),
        }
    }
}

/// Reads `--noded`, `--hub`, `--roles`, `--once` and `--interval-secs`, leaving
/// the rest at its default.
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match argument.as_str() {
            "--noded" => config.noded = value("--noded")?,
            "--hub" => config.hub = value("--hub")?,
            "--roles" => config.roles = Some(PathBuf::from(value("--roles")?)),
            "--once" => config.once = true,
            "--interval-secs" => {
                let seconds: u64 = value("--interval-secs")?
                    .parse()
                    .map_err(|err| format!("--interval-secs: {err}"))?;
                if seconds == 0 {
                    return Err("--interval-secs must be at least 1".to_string());
                }
                config.interval = Duration::from_secs(seconds);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(config)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let config = match parse_args(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("dispatchd: {err}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // Both URLs are checked to be the loopback before any socket is opened, so
    // a typo fails here with a message rather than at the first pass.
    let ledger = match Noded::new(&config.noded) {
        Ok(noded) => noded,
        Err(err) => {
            eprintln!("dispatchd: --noded: {err}");
            return ExitCode::FAILURE;
        }
    };
    let hub = match Hub::new(&config.hub) {
        Ok(hub) => hub,
        Err(err) => {
            eprintln!("dispatchd: --hub: {err}");
            return ExitCode::FAILURE;
        }
    };
    // The codex's tool grants, when a manifest was named: read and checked
    // before the first pass, so a dispatcher that cannot read the policy it was
    // given fails here rather than driving the fleet as if it had none.
    let roles = match &config.roles {
        Some(path) => match Manifest::load(path) {
            Ok(manifest) => Some(manifest),
            Err(err) => {
                eprintln!("dispatchd: --roles: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let mut dispatcher = Dispatcher::with_roles(ledger.clone(), hub, Arc::new(StdoutLog), roles)
        .with_social(Arc::new(ledger) as Arc<dyn Social + Send + Sync>);

    if config.once {
        return match dispatcher.pass() {
            Ok(_report) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("dispatchd: {err}");
                ExitCode::FAILURE
            }
        };
    }

    // The loop: a pass, then the interval, forever. A pass that could not read
    // the ledger is logged and the loop goes on — the node may be restarting,
    // and stopping the dispatcher over it would leave the fleet idle.
    loop {
        if let Err(err) = dispatcher.pass() {
            eprintln!("dispatchd: {err}");
        }
        thread::sleep(config.interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_loopback_stack() {
        let config = parse_args(Vec::new()).expect("no arguments is valid");
        assert_eq!(config.noded, DEFAULT_NODED);
        assert_eq!(config.hub, DEFAULT_HUB);
        assert_eq!(config.roles, None, "no manifest, no grant policy");
        assert!(!config.once, "the daemon loops unless asked for one pass");
        assert_eq!(config.interval, Duration::from_secs(5));
    }

    #[test]
    fn explicit_flags_override_their_defaults() {
        let config = parse_args(vec![
            "--noded".to_string(),
            "http://127.0.0.1:8901".to_string(),
            "--hub".to_string(),
            "http://localhost:19082".to_string(),
            "--roles".to_string(),
            "docs/agents/initial-roles.json".to_string(),
            "--once".to_string(),
            "--interval-secs".to_string(),
            "30".to_string(),
        ])
        .expect("explicit flags parse");
        assert_eq!(config.noded, "http://127.0.0.1:8901");
        assert_eq!(config.hub, "http://localhost:19082");
        assert_eq!(
            config.roles,
            Some(PathBuf::from("docs/agents/initial-roles.json"))
        );
        assert!(config.once);
        assert_eq!(config.interval, Duration::from_secs(30));
    }

    #[test]
    fn bad_arguments_are_reported() {
        assert!(parse_args(vec!["--noded".to_string()]).is_err());
        assert!(parse_args(vec!["--hub".to_string()]).is_err());
        assert!(parse_args(vec!["--roles".to_string()]).is_err());
        assert!(parse_args(vec!["--interval-secs".to_string()]).is_err());
        assert!(parse_args(vec!["--interval-secs".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(vec!["--interval-secs".to_string(), "0".to_string()]).is_err());
        assert!(parse_args(vec!["--nope".to_string()]).is_err());
    }

    /// A manifest that cannot be read is refused before the first pass — the
    /// same rule `vsockd --roles` follows: a daemon that cannot read the policy
    /// it was given must not go on as if it had none.
    #[test]
    fn a_manifest_outside_the_contract_is_refused_before_any_pass() {
        assert!(Manifest::load(std::path::Path::new("/no/such/dispatchd-roles.json")).is_err());
        assert!(Manifest::parse(r#"{"roles": {"builder": {"role": "builder"}}}"#).is_err());
    }

    /// `--once` is a flag, not a value: it takes no argument, and everything
    /// after it still parses.
    #[test]
    fn once_is_a_flag() {
        let config = parse_args(vec![
            "--once".to_string(),
            "--noded".to_string(),
            "http://127.0.0.1:1".to_string(),
        ])
        .expect("a flag then a value parses");
        assert!(config.once);
        assert_eq!(config.noded, "http://127.0.0.1:1");
    }
}
