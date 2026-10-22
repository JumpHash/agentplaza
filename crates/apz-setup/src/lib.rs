//! First-run setup of a plaza. The requirements of its [`Mode`] are checked on this machine
//! ([`check_all`]), and each one that is not met says what the user must do. The setup installs
//! nothing: a plaza needs no Docker, no database and no object store. It needs its services, shipped
//! next to the app (apid for a hub; apz-link, vsockd and noded for a node), their ports free or held
//! by AgentPlaza itself, and, optionally, a hypervisor for the agents' VMs. The desktop app's chooser
//! (`console/boot.rs`) and the CLI (`agentplaza setup`, and the TUI's Dependencies step) both use this
//! crate.
//!
//! - `checks`: what is there for the mode: the services, the ports, and the hypervisor where a node runs.
//! - `runner`: the host's commands, behind [`Runner`] so they can be faked.
//! - `node`: the node's service plan (apz-link, vsockd, noded) and the process check (a process is the
//!   plan's by its name and exact arguments, so a hub's services on the same machine are never taken
//!   for the node's), shared by the desktop app and the CLI so neither starts a second set.
//!
//! Nothing here runs without the user asking: [`check_all`] only reads the machine, and [`fix`] refuses.

mod checks;
pub mod node;
mod runner;

use std::str::FromStr;

pub use checks::{check_all, check_with, not_run_here, runs_here, runs_on};
pub use runner::{Output, Runner, SystemRunner};

/// One requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Need {
    /// The services of the mode, next to this app (they ship with it; never installed here): apid for
    /// a hub; apz-link, vsockd and noded for a node; all of them for a local plaza.
    Services,
    /// Agents' VMs: /dev/kvm usable (Linux), Apple silicon (macOS), the Windows Hypervisor Platform
    /// feature (Windows). Optional and never blocking: only a mode that runs a node checks it, and the
    /// plaza starts without it.
    Hypervisor,
    /// The ports the mode's services listen on, free or held by AgentPlaza itself.
    Ports,
}

impl Need {
    /// Every need, in the order the checklist shows them.
    pub const ALL: [Need; 3] = [Need::Services, Need::Hypervisor, Need::Ports];

    /// The name the command line uses: `services`, `hypervisor`, `ports`.
    pub fn name(self) -> &'static str {
        match self {
            Need::Services => "services",
            Need::Hypervisor => "hypervisor",
            Need::Ports => "ports",
        }
    }
}

impl FromStr for Need {
    type Err = String;

    fn from_str(s: &str) -> Result<Need, String> {
        let wanted = s.trim().to_ascii_lowercase();
        Need::ALL
            .iter()
            .copied()
            .find(|need| need.name() == wanted)
            .ok_or_else(|| {
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
    /// Absent, and `fix` can set it up. The setup installs nothing now, so no check reports this: an
    /// absent service is `Blocked`.
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
    /// The short name the checklist shows: "AgentPlaza services", "Hypervisor", "Ports".
    pub label: &'static str,
    pub state: State,
    /// One line: what was found, or what is wrong.
    pub detail: String,
    /// When `state` is `Missing`: what the fix will do, in words, for the confirmation. `None` otherwise.
    pub fix: Option<String>,
    /// Whether Start needs it.
    pub required: bool,
}

/// Which plaza the checks are for. Each mode checks only what it runs on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The hub and a node on this machine, loopback only: apid, vsockd and noded, and the hypervisor.
    Local,
    /// A hub other machines join: apid and its port. No node daemon and no hypervisor: the agents run on
    /// nodes.
    Hub,
    /// This machine is a node of another hub: apz-link, vsockd and noded, their ports, and the
    /// hypervisor. No hub port: the hub is elsewhere.
    Node,
}

impl Mode {
    /// Whether this mode runs a node here, so needs its daemons, its ports and the hypervisor agents use.
    pub(crate) fn runs_node(self) -> bool {
        matches!(self, Mode::Local | Mode::Node)
    }
}

/// The rows that a fix can be run for: missing, with a description of what it does.
pub fn fixable(checks: &[Check]) -> Vec<Check> {
    checks
        .iter()
        .filter(|c| c.state == State::Missing && c.fix.is_some())
        .cloned()
        .collect()
}

/// What the checks work against: the host's commands. [`Env::system`] is the real machine.
pub struct Env<'a> {
    pub runner: &'a dyn Runner,
}

impl Env<'static> {
    /// The real machine.
    pub fn system() -> Env<'static> {
        Env {
            runner: &SystemRunner,
        }
    }
}

/// Refuses `need`: the setup installs nothing, so every requirement is either present or needs the
/// user, and its row says how. Nothing runs and nothing is logged. Kept so that the callers which offer
/// a fix keep their shape.
pub fn fix(need: Need, _log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err(format!(
        "{} needs you: the setup installs nothing, so see its row for what to do",
        need.name()
    ))
}

/// Whether every required check passes (Start may run). The hypervisor is never required.
pub fn ready(checks: &[Check]) -> bool {
    checks.iter().all(|c| !c.required || c.state == State::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(need: Need, state: State, required: bool) -> Check {
        Check {
            need,
            label: "row",
            state,
            detail: String::new(),
            fix: None,
            required,
        }
    }

    #[test]
    fn the_requirements_are_services_hypervisor_and_ports() {
        let names: Vec<&str> = Need::ALL.iter().map(|need| need.name()).collect();
        assert_eq!(names, ["services", "hypervisor", "ports"]);
        for need in Need::ALL {
            assert_eq!(need.name().parse::<Need>(), Ok(need));
        }
        for gone in ["docker", "docker-running", "postgres", "garage"] {
            assert!(
                gone.parse::<Need>().is_err(),
                "{gone} is no longer a requirement"
            );
        }
    }

    #[test]
    fn the_hypervisor_never_blocks_the_start() {
        for state in [State::Ok, State::Missing, State::Blocked, State::Warning] {
            let checks = [
                row(Need::Services, State::Ok, true),
                row(Need::Hypervisor, state, false),
                row(Need::Ports, State::Ok, true),
            ];
            assert!(ready(&checks), "{state:?}");
        }
        assert!(!ready(&[row(Need::Ports, State::Blocked, true)]));
    }

    #[test]
    fn fixable_lists_only_the_missing_rows_with_a_fix() {
        let checks = vec![
            row(Need::Services, State::Ok, true),
            Check {
                fix: Some("Free the port".into()),
                ..row(Need::Ports, State::Missing, true)
            },
            row(Need::Services, State::Blocked, true),
            Check {
                fix: Some("Start it".into()),
                ..row(Need::Hypervisor, State::Missing, false)
            },
        ];
        let fixable: Vec<Need> = fixable(&checks).iter().map(|c| c.need).collect();
        assert_eq!(fixable, vec![Need::Ports, Need::Hypervisor]);
    }

    #[test]
    fn fix_installs_nothing_and_refuses_every_need() {
        for need in Need::ALL {
            let mut logged = Vec::new();
            let err = fix(need, &mut |line| logged.push(line)).unwrap_err();
            assert!(err.starts_with(need.name()), "{err}");
            assert!(logged.is_empty(), "{logged:?}");
        }
    }
}
