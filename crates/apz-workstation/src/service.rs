//! The workstation daemon installed as a service, so it starts by itself in the account it works for.
//!
//! - **Linux**: a systemd system unit with `User=` the account the daemon runs as. For the agents' own account that
//!   is the agents' user, and the enrolment record is copied to `/var/lib/agentplaza-workstation`, owned by it. For
//!   the owner's own account (a shared session) the unit runs as the owner and uses the owner's state directory. A
//!   system unit has no desktop session of its own: a shared display needs the owner's DISPLAY and D-Bus address,
//!   which the owner's login provides; the agents' account gets its own private display per agent instead.
//! - **macOS**: a LaunchAgent in the account's own session (the screen and accessibility APIs need a login session),
//!   with the record copied into that account's state directory for the agents' account. The account must be signed
//!   in for the agent to run.
//! - **Windows**: a service cannot reach the desktop, so the daemon runs as a scheduled task at logon, in the
//!   account's session (`/IT`: no password is stored).
//!
//! Everything here builds a [`Plan`]. Nothing runs until the caller executes the plan.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use apz_station::Os;

use crate::account::{self, home_for};
use crate::facts::host_os;
use crate::plan::Plan;
use crate::state::RECORD_FILE;

/// The systemd unit's name (without `.service`).
pub const UNIT: &str = "apz-workstation";
/// Where the unit is written.
pub const UNIT_PATH: &str = "/etc/systemd/system/apz-workstation.service";
/// The state directory for an agents' account on Linux, owned by that account.
pub const LINUX_AGENT_STATE: &str = "/var/lib/agentplaza-workstation";
/// The LaunchAgent's label, and its file name under `Library/LaunchAgents`.
pub const LAUNCHD_LABEL: &str = "com.agentplaza.workstation";
/// The scheduled task's name on Windows.
pub const TASK_NAME: &str = "AgentPlaza Workstation";

/// The inputs a service plan needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    pub os: Os,
    /// The daemon binary the service starts.
    pub binary: PathBuf,
    /// The state directory the record is in now (the owner's, after `join`).
    pub state_dir: PathBuf,
    /// The owner's current account.
    pub owner: String,
    /// The account the daemon runs as.
    pub run_as: String,
    /// The home directory of `run_as`.
    pub run_as_home: PathBuf,
    /// The numeric id of `run_as` on Unix, for `launchctl bootstrap gui/<uid>`.
    pub uid: Option<u32>,
}

impl ServiceSpec {
    /// Whether the daemon runs as an account other than the owner's own.
    pub fn is_agent_account(&self) -> bool {
        self.run_as != self.owner
    }
}

/// The directory the daemon keeps its state in, for the account it runs as: the record is copied there for an
/// agents' account, and is used where it is for the owner's account.
pub fn service_state_dir(spec: &ServiceSpec) -> PathBuf {
    if !spec.is_agent_account() {
        return spec.state_dir.clone();
    }
    match spec.os {
        Os::Linux => PathBuf::from(LINUX_AGENT_STATE),
        Os::Macos => spec
            .run_as_home
            .join("Library/Application Support/AgentPlaza Workstation"),
        // ProgramData is shared on Windows: the record stays put, and the agents' account is granted it.
        Os::Windows => spec.state_dir.clone(),
    }
}

/// The plan that installs the service. Pure.
pub fn plan_service(spec: &ServiceSpec) -> Plan {
    match spec.os {
        Os::Linux => plan_linux(spec),
        Os::Macos => plan_macos(spec),
        Os::Windows => plan_windows(spec),
    }
}

/// The plan that stops and removes the service. Pure.
pub fn plan_remove(spec: &ServiceSpec) -> Plan {
    let mut plan = Plan {
        // Linux's unit is a system file; another account's agent files and task are not the owner's to remove.
        privileged: spec.os == Os::Linux || spec.is_agent_account(),
        ..Plan::default()
    };
    // The agents' copy of the record must go too, or the agents' daemon keeps the old token.
    if spec.is_agent_account() && spec.os != Os::Windows {
        let record = service_state_dir(spec)
            .join(RECORD_FILE)
            .display()
            .to_string();
        plan.run("rm", &["-f", record.as_str()]);
    }
    match spec.os {
        Os::Linux => {
            let unit = format!("{UNIT}.service");
            plan.run("systemctl", &["disable", "--now", unit.as_str()]);
            plan.run("rm", &["-f", UNIT_PATH]);
            plan.run("systemctl", &["daemon-reload"]);
        }
        Os::Macos => {
            let plist = launch_agent_path(spec);
            let plist_text = plist.display().to_string();
            match spec.uid {
                Some(uid) => {
                    let target = format!("gui/{uid}");
                    plan.run(
                        "launchctl",
                        &["bootout", target.as_str(), plist_text.as_str()],
                    );
                }
                None => plan.run("launchctl", &["unload", "-w", plist_text.as_str()]),
            }
            plan.run("rm", &["-f", plist_text.as_str()]);
        }
        Os::Windows => {
            plan.run("schtasks", &["/Delete", "/TN", TASK_NAME, "/F"]);
        }
    }
    plan
}

/// The installed plan for this machine: the daemon beside this executable, the state directory in `state_dir`,
/// and the account `run_as`. Queries the account's id on Unix (read-only); nothing is changed.
pub fn install_service(state_dir: &Path, run_as: &str) -> Result<Plan, String> {
    let spec = current_spec(state_dir, run_as)?;
    Ok(plan_service(&spec))
}

/// The removal plan for this machine's service, for the account `run_as`.
pub fn remove_service(state_dir: &Path, run_as: &str) -> Result<Plan, String> {
    let spec = current_spec(state_dir, run_as)?;
    Ok(plan_remove(&spec))
}

/// The spec for this machine, read from the environment (the one place the OS is consulted).
pub fn current_spec(state_dir: &Path, run_as: &str) -> Result<ServiceSpec, String> {
    let os = host_os();
    let owner = account::current_user();
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let run_as_home = if run_as == owner {
        apz_os::dirs::home().ok_or_else(|| "no home directory for this account".to_string())?
    } else {
        home_for(os, run_as, &drive)
    };
    Ok(ServiceSpec {
        os,
        binary: resolve_binary(),
        state_dir: state_dir.to_path_buf(),
        owner,
        run_as: run_as.to_string(),
        run_as_home,
        uid: if cfg!(windows) { None } else { uid_of(run_as) },
    })
}

/// The daemon binary: beside this executable when it is there, else the first one on PATH.
pub fn resolve_binary() -> PathBuf {
    let file = apz_os::process::exe("apz-workstation");
    match std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(&file))
        .filter(|path| path.is_file())
    {
        Some(sibling) => sibling,
        None => PathBuf::from(file),
    }
}

fn uid_of(user: &str) -> Option<u32> {
    let out = Command::new("id")
        .args(["-u", user])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn launch_agent_path(spec: &ServiceSpec) -> PathBuf {
    spec.run_as_home
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

fn plan_linux(spec: &ServiceSpec) -> Plan {
    let mut plan = Plan {
        privileged: true,
        ..Plan::default()
    };
    let state = service_state_dir(spec);
    let state_text = state.display().to_string();
    if spec.is_agent_account() {
        let user = spec.run_as.as_str();
        plan.run(
            "install",
            &[
                "-d",
                "-m",
                "700",
                "-o",
                user,
                "-g",
                user,
                state_text.as_str(),
            ],
        );
        let record = spec.state_dir.join(RECORD_FILE).display().to_string();
        let target = state.join(RECORD_FILE).display().to_string();
        plan.run(
            "install",
            &[
                "-m",
                "600",
                "-o",
                user,
                "-g",
                user,
                record.as_str(),
                target.as_str(),
            ],
        );
    }
    plan.write(UNIT_PATH, linux_unit(spec, &state_text), 0o644);
    plan.run("systemctl", &["daemon-reload"]);
    let unit = format!("{UNIT}.service");
    plan.run("systemctl", &["enable", "--now", unit.as_str()]);
    plan.notes.push(format!(
        "The service runs as {} with state in {}. A shared display needs the owner's DISPLAY and D-Bus address; the agents' account uses its own display.",
        spec.run_as, state_text
    ));
    plan
}

fn linux_unit(spec: &ServiceSpec, state: &str) -> String {
    format!(
        "# AgentPlaza workstation daemon. Written by `agentplaza workstation join`.\n\
         # The unit runs as the account the agents work in (User=). Its state is in {state}.\n\
         [Unit]\n\
         Description=AgentPlaza workstation daemon\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         User={user}\n\
         ExecStart={binary} run --state-dir {state}\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        user = spec.run_as,
        binary = spec.binary.display(),
    )
}

fn plan_macos(spec: &ServiceSpec) -> Plan {
    let mut plan = Plan::default();
    let state = service_state_dir(spec);
    let state_text = state.display().to_string();
    let plist = launch_agent_path(spec);
    let plist_text = plist.display().to_string();
    if spec.is_agent_account() {
        let user = spec.run_as.as_str();
        plan.privileged = true;
        let agents = spec
            .run_as_home
            .join("Library/LaunchAgents")
            .display()
            .to_string();
        plan.run(
            "install",
            &["-d", "-o", user, "-g", "staff", agents.as_str()],
        );
        plan.run(
            "install",
            &[
                "-d",
                "-m",
                "700",
                "-o",
                user,
                "-g",
                "staff",
                state_text.as_str(),
            ],
        );
        let record = spec.state_dir.join(RECORD_FILE).display().to_string();
        let target = state.join(RECORD_FILE).display().to_string();
        plan.run(
            "install",
            &[
                "-m",
                "600",
                "-o",
                user,
                "-g",
                "staff",
                record.as_str(),
                target.as_str(),
            ],
        );
    }
    plan.write(&plist, macos_plist(spec, &state_text), 0o644);
    if spec.is_agent_account() {
        let owner = format!("{}:staff", spec.run_as);
        plan.run("chown", &[owner.as_str(), plist_text.as_str()]);
    }
    match spec.uid {
        Some(uid) => {
            let target = format!("gui/{uid}");
            plan.run(
                "launchctl",
                &["bootstrap", target.as_str(), plist_text.as_str()],
            );
        }
        None => plan.run("launchctl", &["load", "-w", plist_text.as_str()]),
    }
    plan.notes.push(format!(
        "A LaunchAgent in {}'s login session: it runs only while that account is signed in. Screen Recording and Accessibility are granted once for apz-workstation (section 12).",
        spec.run_as
    ));
    plan
}

fn macos_plist(spec: &ServiceSpec, state: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LAUNCHD_LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{binary}</string>\n\
         \t\t<string>run</string>\n\
         \t\t<string>--state-dir</string>\n\
         \t\t<string>{state}</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n",
        binary = xml_escape(&spec.binary.display().to_string()),
        state = xml_escape(state),
    )
}

fn plan_windows(spec: &ServiceSpec) -> Plan {
    let mut plan = Plan::default();
    let state = service_state_dir(spec);
    let state_text = state.display().to_string();
    plan.privileged = spec.is_agent_account();
    if spec.is_agent_account() {
        let grant = format!("{}:(OI)(CI)M", spec.run_as);
        plan.run(
            "icacls",
            &[state_text.as_str(), "/grant", grant.as_str(), "/T"],
        );
    }
    let action = format!(
        "\"{}\" run --state-dir \"{}\"",
        spec.binary.display(),
        state_text
    );
    plan.run(
        "schtasks",
        &[
            "/Create",
            "/TN",
            TASK_NAME,
            "/TR",
            action.as_str(),
            "/SC",
            "ONLOGON",
            "/RU",
            spec.run_as.as_str(),
            "/IT",
            "/F",
        ],
    );
    if !spec.is_agent_account() {
        plan.run("schtasks", &["/Run", "/TN", TASK_NAME]);
    }
    plan.notes.push(format!(
        "A Windows service cannot reach the desktop, so the workstation runs as a logon task in {}'s session. It starts when that account signs in; no password is stored.",
        spec.run_as
    ));
    plan
}

/// Escapes the five XML characters a plist value may contain.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(os: Os, run_as: &str) -> ServiceSpec {
        ServiceSpec {
            os,
            binary: PathBuf::from("/opt/ap/apz-workstation"),
            state_dir: PathBuf::from("/home/zarathos/.local/state/agentplaza-workstation"),
            owner: "zarathos".into(),
            run_as: run_as.into(),
            run_as_home: if run_as == "zarathos" {
                PathBuf::from("/home/zarathos")
            } else {
                PathBuf::from("/home/agentplaza")
            },
            uid: Some(1001),
        }
    }

    #[test]
    fn the_linux_unit_runs_as_the_agents_user_with_its_own_state() {
        let plan = plan_service(&spec(Os::Linux, "agentplaza"));
        let text = plan.render().join("\n");
        assert!(
            text.contains(
                "install -d -m 700 -o agentplaza -g agentplaza /var/lib/agentplaza-workstation"
            ),
            "{text}"
        );
        assert!(
            text.contains("write /etc/systemd/system/apz-workstation.service"),
            "{text}"
        );
        assert!(
            text.contains("$ systemctl enable --now apz-workstation.service"),
            "{text}"
        );
        assert!(plan.privileged);
        let unit = match &plan.steps[2] {
            crate::plan::Step::Write { contents, mode, .. } => {
                assert_eq!(*mode, 0o644);
                contents.clone()
            }
            other => panic!("not the unit: {other:?}"),
        };
        assert!(unit.contains("User=agentplaza\n"), "{unit}");
        assert!(unit.contains("ExecStart=/opt/ap/apz-workstation run --state-dir /var/lib/agentplaza-workstation\n"), "{unit}");
        assert!(unit.contains("Restart=always"));
    }

    #[test]
    fn the_linux_unit_for_the_owners_shared_session_stays_in_the_owners_state() {
        let plan = plan_service(&spec(Os::Linux, "zarathos"));
        let text = plan.render().join("\n");
        assert!(!text.contains("/var/lib/agentplaza-workstation"), "{text}");
        assert!(!text.contains("install -m 600"), "{text}");
        let unit = match &plan.steps[0] {
            crate::plan::Step::Write { contents, .. } => contents.clone(),
            other => panic!("not the unit: {other:?}"),
        };
        assert!(unit.contains("User=zarathos\n"), "{unit}");
        assert!(
            unit.contains("--state-dir /home/zarathos/.local/state/agentplaza-workstation"),
            "{unit}"
        );
    }

    #[test]
    fn the_macos_plist_is_a_launch_agent_in_the_accounts_session() {
        let plan = plan_service(&spec(Os::Macos, "agentplaza"));
        let text = plan.render().join("\n");
        assert!(
            text.contains("/home/agentplaza/Library/LaunchAgents/com.agentplaza.workstation.plist"),
            "{text}"
        );
        assert!(text.contains("$ chown agentplaza:staff"), "{text}");
        assert!(text.contains("$ launchctl bootstrap gui/1001"), "{text}");
        let plist = plan
            .steps
            .iter()
            .find_map(|s| match s {
                crate::plan::Step::Write { contents, .. } => Some(contents.clone()),
                _ => None,
            })
            .expect("a plist");
        assert!(plist.contains("<string>com.agentplaza.workstation</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains("<true/>"));
    }

    #[test]
    fn the_macos_owner_plist_needs_no_administrator_rights() {
        let plan = plan_service(&spec(Os::Macos, "zarathos"));
        assert!(!plan.privileged);
        assert!(!plan.render().join("\n").contains("chown"));
    }

    #[test]
    fn windows_runs_a_logon_task_in_the_accounts_session_without_a_password() {
        let plan = plan_service(&spec(Os::Windows, "agentplaza"));
        let text = plan.render().join("\n");
        assert!(
            text.contains("$ schtasks /Create /TN \"AgentPlaza Workstation\""),
            "{text}"
        );
        assert!(text.contains("/SC ONLOGON /RU agentplaza /IT /F"), "{text}");
        assert!(!text.contains("/RP"), "{text}");
        assert!(text.contains("/grant agentplaza:(OI)(CI)M /T"), "{text}");
        assert!(
            !text.contains("schtasks /Run"),
            "an agents' task starts at its logon: {text}"
        );
    }

    #[test]
    fn windows_for_the_owner_starts_the_task_now() {
        let plan = plan_service(&spec(Os::Windows, "zarathos"));
        assert!(
            plan.render()
                .join("\n")
                .contains("$ schtasks /Run /TN \"AgentPlaza Workstation\"")
        );
        assert!(!plan.privileged);
    }

    #[test]
    fn removing_the_service_stops_it_and_deletes_its_file() {
        let linux = plan_remove(&spec(Os::Linux, "agentplaza"))
            .render()
            .join("\n");
        assert!(linux.contains("$ systemctl disable --now apz-workstation.service"));
        assert!(linux.contains("$ rm -f /etc/systemd/system/apz-workstation.service"));
        let mac = plan_remove(&spec(Os::Macos, "agentplaza"))
            .render()
            .join("\n");
        assert!(mac.contains("$ launchctl bootout gui/1001"));
        let win = plan_remove(&spec(Os::Windows, "agentplaza"))
            .render()
            .join("\n");
        assert!(win.contains("$ schtasks /Delete /TN \"AgentPlaza Workstation\" /F"));
    }

    #[test]
    fn removing_the_agents_service_also_removes_its_copy_of_the_record() {
        let linux = plan_remove(&spec(Os::Linux, "agentplaza"))
            .render()
            .join("\n");
        assert!(
            linux.contains("$ rm -f /var/lib/agentplaza-workstation/station.json"),
            "{linux}"
        );
        let owners = plan_remove(&spec(Os::Linux, "zarathos"))
            .render()
            .join("\n");
        assert!(!owners.contains("station.json"), "{owners}");
    }

    #[test]
    fn the_plist_escapes_xml_in_its_paths() {
        assert_eq!(
            xml_escape("a & <b> \"c\""),
            "a &amp; &lt;b&gt; &quot;c&quot;"
        );
    }
}
