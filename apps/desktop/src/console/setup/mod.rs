//! First-run setup for **Start locally** and **Host a hub**: every requirement of a local plaza is
//! checked ([`check_all`]), and each missing one can be installed or set up ([`fix`]) after the
//! user has read what that will do ([`Check::fix`]) and agreed. The chooser (`boot.rs`) shows the
//! list and keeps Start disabled until every required check passes.
//!
//! - `checks`: what is there (the services next to the app, Docker, its daemon, the Postgres and
//!   Forgejo containers, the hypervisor, the ports).
//! - `docker`: installing Docker and starting its daemon, per platform.
//! - `provision`: the Postgres and Forgejo containers, and their settings in apid's `keys.env`.
//!
//! Nothing here runs without the user asking: `fix` is called only from the user's click.

mod checks;
mod docker;
mod provision;

pub use checks::check_all;

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
    /// Agents' VMs: /dev/kvm usable (Linux), Apple silicon (macOS), the Windows Hypervisor
    /// Platform feature (Windows). Only a warning: the hub runs without it.
    Hypervisor,
    /// The plaza's ports free (7700, 8899, 19080; 5434 and 3300 unless our containers hold them).
    Ports,
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
    /// When `state` is `Missing`: what Install will do, in words, for the confirmation
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

/// Installs or sets up `need`, reporting progress line by line through `log` (the chooser shows
/// them). Blocking: call it off the UI thread. Afterwards, run [`check_all`] again.
pub fn fix(need: Need, log: &mut dyn FnMut(String)) -> Result<(), String> {
    match need {
        Need::Docker => docker::install(log),
        Need::DockerRunning => docker::start(log),
        Need::Postgres => provision::postgres(log),
        Need::Forgejo => provision::forgejo(log),
        Need::Services | Need::Hypervisor | Need::Ports => {
            Err("this one needs you: see its note".to_string())
        }
    }
}

/// A command for a probe or an installer step: on Windows it opens no console window (the app
/// has none, so each console program would flash its own).
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let cmd = std::process::Command::new(program);
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    };
    cmd
}

/// Whether every required check passes (Start may run).
pub fn ready(checks: &[Check]) -> bool {
    checks.iter().all(|c| !c.required || c.state == State::Ok)
}
