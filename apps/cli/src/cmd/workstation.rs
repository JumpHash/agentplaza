//! `agentplaza workstation`: this machine as a workstation, which the agents work through the hub: join with a join
//! code, status, leave.
//!
//! The enrolment, the account and the service are apz-workstation's (`enrol`, `service`). This file shows the plans,
//! asks, and then runs them. Nothing is joined, created or installed before its plan is shown and confirmed.

use apz_station::AccountMode;
use apz_workstation::account::{self, AccountChoice};
use apz_workstation::cli::parse_account;
use apz_workstation::enrol::{self, Enrol};
use apz_workstation::facts::host_os;
use apz_workstation::plan;
use apz_workstation::service;
use clap::Subcommand;
use serde_json::json;

use crate::ctx::confirm;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Add this machine as a workstation with a join code from the hub, then set up the account the agents work in
    /// and the service that keeps the workstation running. The plans are shown first; they run after you confirm.
    Join {
        /// The code the hub showed once.
        code: String,
        /// The station's name on the hub (default: this machine's host name).
        #[arg(long)]
        name: Option<String>,
        /// The account the agents work in: `agent:<user>` (a separate user, created if missing; the default is
        /// agent:agentplaza) or `owner` (your own account: the agents act as you).
        #[arg(long, value_name = "agent:<user>|owner")]
        account: Option<String>,
        /// Go ahead without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Show whether this machine is a workstation, to which hub, the account, and whether its link is up.
    Status,
    /// Forget this machine's enrolment. The service is not stopped by this: its removal steps are printed.
    Leave,
}

/// Runs a `workstation` command. `hub` and `pin` are the global `--hub` (the hub's address) and `--pin` (its
/// certificate's SHA-256, hex).
pub fn run(cmd: Cmd, json: bool, hub: Option<String>, pin: Option<String>) -> Result<(), String> {
    match cmd {
        Cmd::Join {
            code,
            name,
            account,
            yes,
        } => join(hub, pin, code, name, account, yes),
        Cmd::Status => status(json),
        Cmd::Leave => leave(),
    }
}

/// The hub's address: `https://` (or `http://` for a loopback hub).
fn check_hub(hub: Option<String>) -> Result<String, String> {
    let hub =
        hub.ok_or("--hub is the hub's address, for example --hub https://hub.example:7443")?;
    let hub = hub.trim_end_matches('/').to_string();
    if hub.starts_with("https://") || hub.starts_with("http://") {
        Ok(hub)
    } else {
        Err("--hub is an https:// URL".to_string())
    }
}

/// The account choice from `--account`; the default is the separate `agentplaza` user.
fn choice_from(account: Option<&str>) -> Result<AccountChoice, String> {
    let arg = account.map(parse_account).transpose()?;
    Ok(AccountChoice::from_arg(arg))
}

/// The line a machine acting as an owner's account shows, or nothing for a separate account.
fn owner_notice(choice: &AccountChoice, owner: &str) -> Option<String> {
    match choice {
        AccountChoice::Owner => Some(format!("warning: {}", account::owner_warning(owner))),
        AccountChoice::Agent { .. } => None,
    }
}

fn join(
    hub: Option<String>,
    pin: Option<String>,
    code: String,
    name: Option<String>,
    account: Option<String>,
    yes: bool,
) -> Result<(), String> {
    let hub = check_hub(hub)?;
    let choice = choice_from(account.as_deref())?;
    let owner = account::current_user();
    if let Some(notice) = owner_notice(&choice, &owner) {
        println!("{notice}");
    }
    let dir = enrol::default_dir()?;
    // The plans first: what the account and the service will do, and whether they need administrator rights.
    let plans = enrol::enrolment_plans(&dir, &choice)?;
    println!("Joining the hub, then:");
    for (title, plan) in &plans {
        for line in plan::block(title, plan) {
            println!("{line}");
        }
    }
    if enrol::needs_admin(&plans) {
        return Err("the account and the service need administrator rights: nothing was joined. Run this command as an administrator".to_string());
    }
    if !confirm(
        yes,
        "Join the hub, create the account and install the service?",
    )? {
        println!("nothing was done");
        return Ok(());
    }
    let record = enrol::join_here(
        &dir,
        &Enrol {
            hub,
            code,
            name,
            account: choice.clone(),
            pin,
        },
    )?;
    println!(
        "joined {} as station {} ({}), working as {}",
        record.hub,
        record.station,
        record.name,
        account::describe(&record.account)
    );
    enrol::finish(&dir, &choice)?;
    if let AccountChoice::Agent { user } = &choice {
        println!(
            "account {user} is ready; its work folder is ~/{}",
            account::WORK_FOLDER
        );
    }
    println!(
        "the workstation service is installed for {}",
        choice.run_as(&owner)
    );
    for note in enrol::permission_notes(host_os()) {
        println!("{note}");
    }
    Ok(())
}

fn status(json: bool) -> Result<(), String> {
    let dir = enrol::default_dir()?;
    let snapshot = enrol::status_here(&dir)?;
    let Some(record) = snapshot.record.as_ref() else {
        if json {
            println!("{}", json!({ "joined": false }));
        } else {
            println!(
                "not a workstation; join with: agentplaza workstation join <code> --hub <url>"
            );
        }
        return Ok(());
    };
    let link = enrol::link_line(&snapshot);
    let account = account::describe(&record.account);
    if json {
        println!(
            "{}",
            json!({
                "joined": true,
                "hub": record.hub,
                "station": record.station,
                "name": record.name,
                "account": account,
                "link": link,
            })
        );
        return Ok(());
    }
    let width = 8;
    println!("{:>width$}  {}", "hub", record.hub);
    println!(
        "{:>width$}  {} ({})",
        "station", record.station, record.name
    );
    println!("{:>width$}  {}", "account", account);
    println!("{:>width$}  {}", "link", link);
    Ok(())
}

fn leave() -> Result<(), String> {
    let dir = enrol::default_dir()?;
    let snapshot = enrol::status_here(&dir)?;
    let Some(record) = snapshot.record else {
        println!("not a workstation; nothing to leave");
        return Ok(());
    };
    let run_as = match &record.account {
        AccountMode::AgentUser { name } | AccountMode::Owner { name } => name.clone(),
    };
    enrol::leave_here(&dir)?;
    println!(
        "left: this machine is no longer a workstation (its log is kept in {})",
        dir.display()
    );
    println!("the hub still lists the station until the owner removes it there");
    let plan = service::remove_service(&dir, &run_as)?;
    for line in plan::block("if the service was installed, remove it with", &plan) {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_workstation::plan::Plan;
    use apz_workstation::service::ServiceSpec;
    use clap::Parser;
    use std::path::PathBuf;

    /// The subcommand alone, so each test reads as `agentplaza workstation <args>`.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn parse(args: &[&str]) -> Result<Cmd, String> {
        Probe::try_parse_from(std::iter::once("workstation").chain(args.iter().copied()))
            .map(|p| p.cmd)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn join_takes_the_code_and_its_options() {
        match parse(&[
            "join",
            "ABC-123",
            "--name",
            "studio",
            "--account",
            "owner",
            "--yes",
        ])
        .unwrap()
        {
            Cmd::Join {
                code,
                name,
                account,
                yes,
            } => {
                assert_eq!(code, "ABC-123");
                assert_eq!(name.as_deref(), Some("studio"));
                assert_eq!(account.as_deref(), Some("owner"));
                assert!(yes);
            }
            other => panic!("not a join: {other:?}"),
        }
    }

    #[test]
    fn join_needs_a_code_and_status_and_leave_take_nothing() {
        assert!(parse(&["join"]).is_err());
        assert!(matches!(parse(&["status"]).unwrap(), Cmd::Status));
        assert!(matches!(parse(&["leave"]).unwrap(), Cmd::Leave));
        assert!(parse(&["leave", "extra"]).is_err());
    }

    #[test]
    fn the_hub_must_be_a_url() {
        assert_eq!(
            check_hub(Some("https://hub.example:7443/".into())).unwrap(),
            "https://hub.example:7443"
        );
        assert!(check_hub(Some("ftp://hub".into())).is_err());
        assert!(check_hub(None).is_err());
    }

    #[test]
    fn the_account_defaults_to_the_separate_user_and_owner_is_its_own_choice() {
        assert_eq!(
            choice_from(None).unwrap(),
            AccountChoice::Agent {
                user: "agentplaza".into()
            }
        );
        assert_eq!(choice_from(Some("owner")).unwrap(), AccountChoice::Owner);
        assert_eq!(
            choice_from(Some("agent:bob")).unwrap(),
            AccountChoice::Agent { user: "bob".into() }
        );
        assert!(choice_from(Some("root")).is_err());
    }

    #[test]
    fn owner_shows_the_warning_and_a_separate_account_does_not() {
        let owner = owner_notice(&AccountChoice::Owner, "zarathos").unwrap();
        assert!(owner.contains("agents will act as zarathos"), "{owner}");
        assert!(
            owner_notice(
                &AccountChoice::Agent {
                    user: "agentplaza".into()
                },
                "zarathos"
            )
            .is_none()
        );
    }

    #[test]
    fn a_service_plan_prints_its_steps_its_privilege_and_its_notes() {
        let spec = ServiceSpec {
            os: apz_station::Os::Linux,
            binary: PathBuf::from("/opt/apz-workstation"),
            state_dir: PathBuf::from("/home/zarathos/.local/state/agentplaza-workstation"),
            owner: "zarathos".into(),
            run_as: "agentplaza".into(),
            run_as_home: PathBuf::from("/home/agentplaza"),
            uid: Some(1001),
        };
        let lines = plan::block("service", &service::plan_service(&spec));
        assert_eq!(lines[0], "service:");
        assert!(
            lines
                .iter()
                .any(|l| l == "  $ systemctl enable --now apz-workstation.service"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "  (needs administrator rights)"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("  note: The service runs as agentplaza")),
            "{lines:?}"
        );
    }

    #[test]
    fn an_account_plan_never_prints_its_password() {
        let plan: Plan = account::plan_agent_user(
            apz_station::Os::Macos,
            "agentplaza",
            std::path::Path::new("/Users/agentplaza"),
            &[],
            false,
            "HIDDEN-PASSWORD-9Aa!",
        );
        let text = plan::block("account", &plan).join("\n");
        assert!(!text.contains("HIDDEN-PASSWORD"), "{text}");
        assert!(text.contains("sysadminctl -addUser agentplaza"), "{text}");
    }
}
