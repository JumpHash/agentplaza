//! `agentplaza setup`: what a local plaza needs on this machine (Docker, its daemon, Postgres,
//! Forgejo, Garage, the services, the ports), and fixing the missing ones. A fix runs only after
//! the user confirms it, having seen what it will do. The checks and fixes are `apz-setup`'s; the
//! TUI's Dependencies step uses the same crate.

use std::io::{IsTerminal, Write};

use apz_setup::{Check, Mode, Need, State};
use serde_json::{json, Value};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Fix one requirement: services, docker, docker-running, postgres, forgejo, garage, hypervisor or ports.
    #[arg(long, value_name = "NEED", conflicts_with = "fix_all")]
    fix: Option<Need>,
    /// Fix every missing requirement that has a fix, in the order of the checklist.
    #[arg(long)]
    fix_all: bool,
}

pub fn run(args: Args, json: bool) -> Result<(), String> {
    let checks = apz_setup::check_all(Mode::Local);
    let plan = match (args.fix, args.fix_all) {
        (Some(need), _) => select(&checks, Some(need))?,
        (None, true) => select(&checks, None)?,
        (None, false) => {
            if json {
                println!("{}", to_json(&checks));
            } else {
                print_checks(&checks);
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
        State::Blocked | State::Warning => Err(format!("{} needs you: {}", check.label, check.detail)),
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
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(confirms(&answer))
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
    let width = checks.iter().map(|check| check.label.len()).max().unwrap_or(0);
    for check in checks {
        let optional = if check.required { "" } else { " (optional)" };
        println!("{:<width$}  {:<8}  {}{optional}", check.label, word(check.state), check.detail);
        if check.fix.is_some() {
            println!("{:<width$}  {:<8}  fix: agentplaza setup --fix {}", "", "", check.need.name());
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
        let plan = vec![row(Need::Postgres, State::Missing, Some("Create the container agentplaza-postgres"))];
        assert_eq!(plan_lines(&plan), vec!["  postgres: Create the container agentplaza-postgres".to_string()]);
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
        let plan: Vec<Need> = select(&checks, None).unwrap().iter().map(|c| c.need).collect();
        assert_eq!(plan, vec![Need::Docker, Need::DockerRunning, Need::Garage]);
        assert!(select(&checks[..1], None).unwrap().is_empty());
    }

    #[test]
    fn the_flags_parse_and_conflict() {
        let parsed = crate::Cli::try_parse_from(["agentplaza", "setup", "--fix", "docker-running"]).unwrap();
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
        assert!(crate::Cli::try_parse_from(["agentplaza", "setup", "--fix", "garage", "--fix-all"]).is_err());
    }
}
