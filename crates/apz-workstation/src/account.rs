//! The OS account the agents work in, chosen in setup: a separate user created for the agents, or the owner's own.
//!
//! [`ensure_agent_user`] returns the plan that creates the user and its work folder. It runs the plan only when the
//! call is not a dry run and the process has administrator rights; otherwise it returns the plan without running it.
//! The plan builders ([`plan_agent_user`]) take the OS as a parameter, so each OS's commands are tested on any host.
//! Nothing in the tests creates a user.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use apz_station::{AccountMode, Os};

use crate::cli::AccountArg;
use crate::facts::host_os;
use crate::plan::Plan;

/// The user name setup proposes for the agents.
pub const DEFAULT_AGENT_USER: &str = "agentplaza";
/// The account's full name on every OS.
pub const AGENT_FULL_NAME: &str = "AgentPlaza agents";
/// The Linux groups the agents join when they exist: GPU render nodes, video, and uinput for input.
pub const LINUX_GROUPS: [&str; 3] = ["video", "render", "input"];
/// The work folder's name inside the account's home.
pub const WORK_FOLDER: &str = "AgentPlaza";

/// Names an OS or a person already uses: refused as the agents' account.
const RESERVED: [&str; 8] = [
    "root",
    "admin",
    "administrator",
    "daemon",
    "nobody",
    "system",
    "guest",
    "sys",
];

/// The account the daemon works in, as chosen in setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountChoice {
    /// A separate OS user created for the agents.
    Agent { user: String },
    /// The owner's own account: the agents act as the owner.
    Owner,
}

impl AccountChoice {
    /// From the `--account` argument: `None` is the default separate user.
    pub fn from_arg(arg: Option<AccountArg>) -> AccountChoice {
        match arg {
            Some(AccountArg::Agent(user)) => AccountChoice::Agent { user },
            Some(AccountArg::Owner) => AccountChoice::Owner,
            None => AccountChoice::Agent {
                user: DEFAULT_AGENT_USER.to_string(),
            },
        }
    }

    /// The account the daemon's record names: the agent user, or the owner's current account.
    pub fn account_mode(&self, owner: &str) -> AccountMode {
        match self {
            AccountChoice::Agent { user } => AccountMode::AgentUser { name: user.clone() },
            AccountChoice::Owner => AccountMode::Owner {
                name: owner.to_string(),
            },
        }
    }

    /// The OS account the daemon process runs as.
    pub fn run_as(&self, owner: &str) -> String {
        match self {
            AccountChoice::Agent { user } => user.clone(),
            AccountChoice::Owner => owner.to_string(),
        }
    }

    /// Refuses an agent user name the OS would not take, or that is the owner's own account.
    pub fn validate(&self, owner: &str) -> Result<(), String> {
        match self {
            AccountChoice::Agent { user } => {
                validate_user(user)?;
                if user == owner {
                    return Err(format!(
                        "the agents' account cannot be your own ({owner}): choose the owner's account instead, or another name"
                    ));
                }
                Ok(())
            }
            AccountChoice::Owner => Ok(()),
        }
    }
}

/// The line that goes with every machine acting as an owner's account.
pub fn owner_warning(owner: &str) -> String {
    format!(
        "agents will act as {owner}: they can use everything this account can open, read and change, and the badge \"agents act as {owner}\" stays on the machine wherever it shows"
    )
}

/// The owner's current account name (`USER`, else `USERNAME`).
pub fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "agent".to_string())
}

/// `agent:<name>` or the owner's account, as the status lines show it.
pub fn describe(account: &AccountMode) -> String {
    match account {
        AccountMode::AgentUser { name } => format!("agent:{name}"),
        AccountMode::Owner { name } => format!("owner ({name}): agents act as this account"),
    }
}

/// A user name every OS takes: 1 to 20 lowercase ASCII letters, digits, `-` or `_`, starting with a letter or `_`,
/// and not a reserved name.
pub fn validate_user(user: &str) -> Result<(), String> {
    let first_ok = user
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    let rest_ok = user
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if user.is_empty() || user.len() > 20 || !first_ok || !rest_ok {
        return Err(format!(
            "{user:?} is not a usable account name: 1 to 20 lowercase letters, digits, - or _, starting with a letter"
        ));
    }
    if RESERVED.contains(&user) {
        return Err(format!("{user:?} is a reserved account name"));
    }
    Ok(())
}

/// The new user's home on each OS. Windows puts profiles under the system drive.
pub fn home_for(os: Os, user: &str, system_drive: &str) -> PathBuf {
    match os {
        Os::Linux => PathBuf::from("/home").join(user),
        Os::Macos => PathBuf::from("/Users").join(user),
        Os::Windows => PathBuf::from(format!("{system_drive}\\Users")).join(user),
    }
}

/// A password that meets Windows' complexity rule (upper, lower, digit and symbol) and is random per call.
/// It is never printed: [`plan_agent_user`] shows a placeholder.
pub fn generate_password() -> String {
    format!("{}Aa1!", apz_trust::b64(&apz_trust::random::<18>()))
}

/// The plan that creates the agents' user on `os` and its work folder. Pure: nothing runs.
///
/// `present_groups` are the Linux groups from [`LINUX_GROUPS`] that exist on the machine; `exists` says whether the
/// user already exists (then only the group membership and the work folder are planned); `password` is used on macOS
/// and Windows and never appears in the printed plan.
pub fn plan_agent_user(
    os: Os,
    user: &str,
    home: &Path,
    present_groups: &[&str],
    exists: bool,
    password: &str,
) -> Plan {
    let work = home.join(WORK_FOLDER);
    let work_text = work.display().to_string();
    let mut plan = Plan {
        privileged: true,
        ..Plan::default()
    };
    match os {
        Os::Linux => {
            if !exists {
                plan.run("useradd", &["--create-home", "--shell", "/bin/bash", user]);
            }
            if !present_groups.is_empty() {
                let groups = present_groups.join(",");
                plan.run("usermod", &["-aG", groups.as_str(), user]);
            }
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
                    work_text.as_str(),
                ],
            );
            plan.notes.push(
                "The agents' user gets a home directory and a login shell, and joins video, render and input when they exist (GPU access and input)."
                    .to_string(),
            );
        }
        Os::Macos => {
            if !exists {
                plan.run_secret(
                    "sysadminctl",
                    vec![
                        "-addUser".into(),
                        user.into(),
                        "-fullName".into(),
                        AGENT_FULL_NAME.into(),
                        "-password".into(),
                        password.into(),
                        "-home".into(),
                        home.display().to_string(),
                    ],
                    vec![
                        "-addUser".into(),
                        user.into(),
                        "-fullName".into(),
                        AGENT_FULL_NAME.into(),
                        "-password".into(),
                        "********".into(),
                        "-home".into(),
                        home.display().to_string(),
                    ],
                );
            }
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
                    work_text.as_str(),
                ],
            );
            plan.notes.push(
                "macOS asks for Screen Recording and Accessibility once for apz-workstation, signed in as this account (section 12)."
                    .to_string(),
            );
        }
        Os::Windows => {
            if !exists {
                plan.run_secret(
                    "net",
                    vec![
                        "user".into(),
                        user.into(),
                        password.into(),
                        "/add".into(),
                        format!("/fullname:{AGENT_FULL_NAME}"),
                    ],
                    vec![
                        "user".into(),
                        user.into(),
                        "********".into(),
                        "/add".into(),
                        format!("/fullname:{AGENT_FULL_NAME}"),
                    ],
                );
            }
            let script =
                format!("New-Item -ItemType Directory -Force -Path '{work_text}' | Out-Null");
            plan.run("powershell", &["-NoProfile", "-Command", script.as_str()]);
            let grant = format!("{user}:(OI)(CI)F");
            plan.run("icacls", &[work_text.as_str(), "/grant", grant.as_str()]);
            plan.notes.push(
                "The password is generated and never shown. Windows runs the workstation as a logon task, so the agents' user signs in once to start it: set a password you know with `net user <user> *` first."
                    .to_string(),
            );
        }
    }
    plan
}

/// Plans, and when `dry_run` is false and the process has administrator rights, creates the agents' OS user and
/// its work folder. Returns the plan either way. Without administrator rights and without a dry run it refuses,
/// with the plan in the error so the owner can run the same commands as an administrator.
pub fn ensure_agent_user(user: &str, dry_run: bool) -> Result<Plan, String> {
    validate_user(user)?;
    let os = host_os();
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let home = home_for(os, user, &drive);
    let exists = user_exists(os, user);
    let present: Vec<&str> = if os == Os::Linux {
        LINUX_GROUPS
            .iter()
            .copied()
            .filter(|g| succeeds("getent", &["group", g]))
            .collect()
    } else {
        Vec::new()
    };
    let plan = plan_agent_user(os, user, &home, &present, exists, &generate_password());
    if dry_run {
        return Ok(plan);
    }
    if !is_privileged() {
        return Err(format!(
            "creating the agents' account needs administrator rights. As an administrator, run:\n  {}",
            plan.render().join("\n  ")
        ));
    }
    plan.execute()?;
    Ok(plan)
}

/// Whether this process has administrator rights (root on Unix, an elevated session on Windows).
pub fn is_privileged() -> bool {
    if cfg!(windows) {
        succeeds("net", &["session"])
    } else {
        Command::new("id")
            .arg("-u")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false)
    }
}

/// Whether the OS user exists.
fn user_exists(os: Os, user: &str) -> bool {
    match os {
        Os::Windows => succeeds("net", &["user", user]),
        _ => succeeds("id", &["-u", user]),
    }
}

/// Runs a read-only command and reports whether it succeeded. Its output is discarded.
fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "PLAIN-SECRET-123Aa1!";

    #[test]
    fn user_names_must_be_short_lowercase_and_not_reserved() {
        assert!(validate_user("agentplaza").is_ok());
        assert!(validate_user("agent_2-x").is_ok());
        assert!(validate_user("").is_err());
        assert!(validate_user("Agent").is_err());
        assert!(validate_user("1agent").is_err());
        assert!(validate_user("a b").is_err());
        assert!(validate_user("a;rm").is_err());
        assert!(validate_user("a/b").is_err());
        assert!(validate_user(&"x".repeat(21)).is_err());
        assert!(validate_user("root").is_err());
        assert!(validate_user("Administrator").is_err());
    }

    #[test]
    fn the_agents_account_cannot_be_the_owners_own() {
        let agent = AccountChoice::Agent {
            user: "zarathos".into(),
        };
        assert!(agent.validate("zarathos").is_err());
        assert!(agent.validate("someone").is_ok());
        assert!(AccountChoice::Owner.validate("zarathos").is_ok());
    }

    #[test]
    fn the_default_is_the_separate_agentplaza_user() {
        assert_eq!(
            AccountChoice::from_arg(None),
            AccountChoice::Agent {
                user: DEFAULT_AGENT_USER.into()
            }
        );
        assert_eq!(
            AccountChoice::from_arg(Some(AccountArg::Owner)),
            AccountChoice::Owner
        );
    }

    #[test]
    fn the_run_as_account_and_the_record_follow_the_choice() {
        let agent = AccountChoice::Agent {
            user: "agentplaza".into(),
        };
        assert_eq!(agent.run_as("zarathos"), "agentplaza");
        assert_eq!(
            agent.account_mode("zarathos"),
            AccountMode::AgentUser {
                name: "agentplaza".into()
            }
        );
        assert_eq!(AccountChoice::Owner.run_as("zarathos"), "zarathos");
        assert_eq!(
            AccountChoice::Owner.account_mode("zarathos"),
            AccountMode::Owner {
                name: "zarathos".into()
            }
        );
    }

    #[test]
    fn the_owner_warning_names_the_account() {
        let text = owner_warning("zarathos");
        assert!(text.starts_with("agents will act as zarathos"), "{text}");
    }

    #[test]
    fn homes_follow_each_os() {
        assert_eq!(
            home_for(Os::Linux, "agentplaza", "C:"),
            PathBuf::from("/home/agentplaza")
        );
        assert_eq!(
            home_for(Os::Macos, "agentplaza", "C:"),
            PathBuf::from("/Users/agentplaza")
        );
        assert_eq!(
            home_for(Os::Windows, "agentplaza", "D:"),
            PathBuf::from("D:\\Users").join("agentplaza")
        );
    }

    #[test]
    fn a_generated_password_meets_the_windows_rule_and_is_random() {
        let a = generate_password();
        let b = generate_password();
        assert_ne!(a, b);
        assert!(a.len() >= 20);
        assert!(a.chars().any(|c| c.is_ascii_uppercase()));
        assert!(a.chars().any(|c| c.is_ascii_lowercase()));
        assert!(a.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn linux_creates_the_user_joins_the_present_groups_and_makes_the_work_folder() {
        let plan = plan_agent_user(
            Os::Linux,
            "agentplaza",
            Path::new("/home/agentplaza"),
            &["video", "input"],
            false,
            PASSWORD,
        );
        let lines = plan.render();
        assert_eq!(
            lines[0],
            "$ useradd --create-home --shell /bin/bash agentplaza"
        );
        assert_eq!(lines[1], "$ usermod -aG video,input agentplaza");
        assert_eq!(
            lines[2],
            "$ install -d -m 700 -o agentplaza -g agentplaza /home/agentplaza/AgentPlaza"
        );
        assert_eq!(lines.len(), 3);
        assert!(plan.privileged);
        assert!(!lines.join("\n").contains(PASSWORD));
    }

    #[test]
    fn linux_skips_creation_for_an_existing_user_and_missing_groups() {
        let plan = plan_agent_user(
            Os::Linux,
            "agentplaza",
            Path::new("/home/agentplaza"),
            &[],
            true,
            PASSWORD,
        );
        assert_eq!(plan.steps.len(), 1, "{:?}", plan.render());
        assert!(plan.render()[0].starts_with("$ install -d"));
    }

    #[test]
    fn macos_creates_the_user_with_sysadminctl_and_hides_the_password() {
        let plan = plan_agent_user(
            Os::Macos,
            "agentplaza",
            Path::new("/Users/agentplaza"),
            &[],
            false,
            PASSWORD,
        );
        let lines = plan.render();
        assert_eq!(
            lines[0],
            "$ sysadminctl -addUser agentplaza -fullName \"AgentPlaza agents\" -password ******** -home /Users/agentplaza"
        );
        assert_eq!(
            lines[1],
            "$ install -d -m 700 -o agentplaza -g staff /Users/agentplaza/AgentPlaza"
        );
        assert!(!lines.join("\n").contains(PASSWORD));
        // The arguments that run do carry the password.
        match &plan.steps[0] {
            crate::plan::Step::Run { args, .. } => assert!(args.contains(&PASSWORD.to_string())),
            other => panic!("not a command: {other:?}"),
        }
    }

    #[test]
    fn windows_creates_the_account_grants_the_folder_and_hides_the_password() {
        let plan = plan_agent_user(
            Os::Windows,
            "agentplaza",
            Path::new("C:\\Users\\agentplaza"),
            &[],
            false,
            PASSWORD,
        );
        let lines = plan.render();
        assert_eq!(
            lines[0],
            "$ net user agentplaza ******** /add \"/fullname:AgentPlaza agents\""
        );
        assert!(
            lines[1].starts_with("$ powershell -NoProfile -Command"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].ends_with("/grant agentplaza:(OI)(CI)F"),
            "{}",
            lines[2]
        );
        assert!(!lines.join("\n").contains(PASSWORD));
    }

    #[test]
    fn windows_with_an_existing_user_only_makes_the_folder() {
        let plan = plan_agent_user(
            Os::Windows,
            "agentplaza",
            Path::new("C:\\Users\\agentplaza"),
            &[],
            true,
            PASSWORD,
        );
        assert_eq!(plan.steps.len(), 2);
    }
}
