//! First-run setup of a local plaza. Every requirement is checked ([`check_all`]), and each missing
//! one can be installed or set up ([`fix`]) after the user has read what that will do
//! ([`Check::fix`]) and agreed. The desktop app's chooser (`console/boot.rs`) and the CLI
//! (`agentplaza setup`, and the TUI's Dependencies step) both use this crate.
//!
//! - `checks`: what is there (the services next to the app, Docker, its daemon, the Postgres,
//!   Forgejo and Garage containers, the hypervisor, the ports).
//! - `docker`: installing Docker and starting its daemon, per platform.
//! - `provision`: the Postgres, Forgejo and Garage containers, and their settings in apid's `keys.env`.
//! - `runner`: the commands and files of the above, behind [`Runner`] so tests can mock them.
//! - `node`: the node's service plan (apz-link, vsockd, noded) and the running-process check, shared
//!   by the desktop app and the CLI so neither starts a second set.
//!
//! Nothing here runs without the user asking: `fix` is called only from the user's click or a
//! confirmed command.

mod checks;
mod docker;
pub mod node;
mod provision;
mod runner;

use std::path::PathBuf;
use std::str::FromStr;

pub use checks::{check_all, check_with};
pub use runner::{Output, Runner, SystemRunner};

/// One requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Need {
    /// apid, vsockd and noded next to this app (they ship with it; never installed here).
    Services,
    /// The `docker` command.
    Docker,
    /// Docker's daemon answering (`docker info`).
    DockerRunning,
    /// The container `agentplaza-postgres` running, and `APZ_DATABASE_URL` in keys.env.
    Postgres,
    /// The container `agentplaza-forgejo` running, with its admin token in keys.env.
    Forgejo,
    /// The container `agentplaza-garage` (the S3 object store the library keeps files in) running,
    /// and its endpoint and keys in keys.env. Optional: apid keeps files locally without it.
    Garage,
    /// Agents' VMs: /dev/kvm usable (Linux), Apple silicon (macOS), the Windows Hypervisor
    /// Platform feature (Windows). Only a warning: the hub runs without it.
    Hypervisor,
    /// The plaza's ports free (7700, 8899, 19080; 5434 and 3300 unless our containers hold them).
    Ports,
}

impl Need {
    /// Every need, in the order the checklist shows them.
    pub const ALL: [Need; 8] = [
        Need::Services,
        Need::Docker,
        Need::DockerRunning,
        Need::Postgres,
        Need::Forgejo,
        Need::Garage,
        Need::Hypervisor,
        Need::Ports,
    ];

    /// The name the command line uses: `docker-running`, `postgres`, ….
    pub fn name(self) -> &'static str {
        match self {
            Need::Services => "services",
            Need::Docker => "docker",
            Need::DockerRunning => "docker-running",
            Need::Postgres => "postgres",
            Need::Forgejo => "forgejo",
            Need::Garage => "garage",
            Need::Hypervisor => "hypervisor",
            Need::Ports => "ports",
        }
    }
}

impl FromStr for Need {
    type Err = String;

    fn from_str(s: &str) -> Result<Need, String> {
        let wanted = s.trim().to_ascii_lowercase();
        Need::ALL.iter().copied().find(|need| need.name() == wanted).ok_or_else(|| {
            let names: Vec<&str> = Need::ALL.iter().map(|need| need.name()).collect();
            format!("unknown requirement \"{s}\" (one of: {})", names.join(", "))
        })
    }
}

/// Where a requirement stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Present and working.
    Ok,
    /// Absent, and `fix` can install or set it up.
    Missing,
    /// Absent or broken, and the user must act (`detail` says how); Start stays disabled if `required`.
    Blocked,
    /// Not usable, but the plaza starts without it (e.g. no hypervisor: the hub runs, agents do not).
    Warning,
}

/// One row of the checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub need: Need,
    /// Short name: "Docker", "Postgres", …
    pub label: &'static str,
    pub state: State,
    /// One line: what was found, or what is wrong.
    pub detail: String,
    /// When `state` is `Missing`: what the fix will do, in words, for the confirmation
    /// (e.g. "Install Docker Engine with apt (asks for your password), start it, add you to the
    /// docker group"). `None` otherwise.
    pub fix: Option<String>,
    /// Whether Start needs it.
    pub required: bool,
}

/// Which plaza the checks are for: `Hub` adds nothing yet, but keeps the door open for hub-only
/// needs (a public address, a certificate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Local,
    Hub,
}

/// The rows that a fix can be run for: missing, with a description of what it does.
pub fn fixable(checks: &[Check]) -> Vec<Check> {
    checks.iter().filter(|c| c.state == State::Missing && c.fix.is_some()).cloned().collect()
}

/// What the setup works against: the host's commands and files, keys.env's path, and the login name.
/// [`Env::system`] is the real machine; tests build one around a mock runner and a temp directory.
pub struct Env<'a> {
    pub runner: &'a dyn Runner,
    /// `<home>/.config/agentplaza/keys.env`; `None` when there is no home directory.
    pub keys: Option<PathBuf>,
    /// The user's login name (`USER`), for the docker group on Linux.
    pub user: Option<String>,
}

impl Env<'static> {
    /// The real machine.
    pub fn system() -> Env<'static> {
        Env {
            runner: &SystemRunner,
            keys: apz_os::dirs::home().map(|home| home.join(".config").join("agentplaza").join("keys.env")),
            user: std::env::var("USER").ok().filter(|user| !user.is_empty()),
        }
    }
}

/// Installs or sets up `need` on this machine, reporting progress line by line through `log` (the
/// chooser and the CLI show them). Blocking: call it off the UI thread. Afterwards, run
/// [`check_all`] again.
pub fn fix(need: Need, log: &mut dyn FnMut(String)) -> Result<(), String> {
    fix_with(&Env::system(), need, log)
}

/// [`fix`] against `env`.
pub fn fix_with(env: &Env, need: Need, log: &mut dyn FnMut(String)) -> Result<(), String> {
    match need {
        Need::Docker => docker::install(env, log),
        Need::DockerRunning => docker::start(env, log),
        Need::Postgres => provision::postgres(env, log),
        Need::Forgejo => provision::forgejo(env, log),
        Need::Garage => provision::garage(env, log),
        Need::Services | Need::Hypervisor | Need::Ports => {
            Err("this one needs you: see its note".to_string())
        }
    }
}

/// Whether every required check passes (Start may run).
pub fn ready(checks: &[Check]) -> bool {
    checks.iter().all(|c| !c.required || c.state == State::Ok)
}
