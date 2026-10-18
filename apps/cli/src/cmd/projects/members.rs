//! `projects members`: who works on a project. Every agent but the orchestrator works on one project at a time, or
//! is on the bench and does nothing; the owner puts agents on a project and sends them back
//! (`crates/apid/src/membership.rs`).

use clap::Subcommand;
use serde_json::{Value, json};

use crate::cmd::agents::name_of;
use crate::ctx::{self, Ctx, confirm};

#[derive(Subcommand, Debug)]
pub enum Action {
    /// The project's agents (the default).
    List,
    /// Put an agent on the project: from the bench, or from another project, whose work it stops.
    Add {
        /// The agent's cid (`agents list --bench` shows the agents on the bench).
        cid: u64,
    },
    /// Send an agent back to the bench: its work for the project stops. Asks to confirm unless --yes.
    Remove {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// The agents on no project that hold or held the project's tasks: who worked on it before projects had
    /// members, to put on it with `add`.
    Suggested,
}

/// The items of a hub array (none when the value is not one).
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// What `held` says about an agent: why it waits, or "-" when it works.
fn held_text(agent: &Value) -> String {
    match ctx::s(agent, "held") {
        "" => "-".to_string(),
        why => why.to_string(),
    }
}

/// The members as rows: cid, name, role, status, when they joined, and why they wait when they do (a paused
/// project holds its agents).
fn member_rows(members: &Value) -> Vec<Vec<String>> {
    items(members)
        .iter()
        .map(|a| {
            vec![
                ctx::text(a, "cid"),
                name_of(a),
                ctx::s(a, "role").to_string(),
                ctx::s(a, "status").to_string(),
                ctx::ago(&a["joined_ms"]),
                held_text(a),
            ]
        })
        .collect()
}

/// The suggested members as rows: cid, name, role, status, and the project's tasks they hold or held.
fn suggested_rows(agents: &Value) -> Vec<Vec<String>> {
    items(agents)
        .iter()
        .map(|a| {
            let tasks: Vec<&str> = a
                .get("tasks")
                .map(items)
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            vec![
                ctx::text(a, "cid"),
                name_of(a),
                ctx::s(a, "role").to_string(),
                ctx::s(a, "status").to_string(),
                tasks.join(", "),
            ]
        })
        .collect()
}

/// `projects members ID`: the project's agents; `add`, `remove` and `suggested` change them or say who could join.
pub fn members(ctx: &Ctx, id: &str, action: Option<Action>) -> Result<(), String> {
    let base = format!("/v1/projects/{}/members", ctx::seg(id));
    match action.unwrap_or(Action::List) {
        Action::List => {
            let members = ctx.client.get(&base)?;
            ctx.out(&members, |v| {
                ctx::table(
                    &["cid", "name", "role", "status", "joined", "held"],
                    &member_rows(v),
                );
            });
        }
        Action::Add { cid } => {
            let agent = ctx.client.post(&base, &json!({ "cid": cid }))?;
            ctx.out(&agent, |a| {
                println!("{} (agent {cid}) works on project {id} now", name_of(a))
            });
        }
        Action::Remove { cid, yes } => {
            let question =
                format!("Send agent {cid} back to the bench? Its work for project {id} stops.");
            if !confirm(yes, &question)? {
                println!("cancelled");
                return Ok(());
            }
            let left = ctx.client.delete(&format!("{base}/{cid}"))?;
            ctx.out(&left, |_| println!("agent {cid} is back on the bench"));
        }
        Action::Suggested => {
            let path = format!("/v1/projects/{}/suggested-members", ctx::seg(id));
            let agents = ctx.client.get(&path)?;
            ctx.out(&agents, |v| {
                let rows = suggested_rows(v);
                ctx::table(&["cid", "name", "role", "status", "tasks"], &rows);
                if !rows.is_empty() {
                    println!("\nput one on it: agentplaza projects members {id} add <cid>");
                }
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_show_when_they_joined_and_why_they_wait() {
        let members = json!([
            { "cid": 7, "name": "Ada", "role": "builder", "status": "idle", "project": "pr3", "joined_ms": null, "held": null },
            { "cid": 9, "name": "", "role": "qa", "status": "asleep", "project": "pr3", "joined_ms": null,
              "held": "project pr3 is paused: its agents wait until the owner resumes it" }
        ]);
        assert_eq!(
            member_rows(&members),
            [
                ["7", "Ada", "builder", "idle", "—", "-"]
                    .map(String::from)
                    .to_vec(),
                [
                    "9",
                    "agent 9",
                    "qa",
                    "asleep",
                    "—",
                    "project pr3 is paused: its agents wait until the owner resumes it"
                ]
                .map(String::from)
                .to_vec(),
            ]
        );
        assert!(member_rows(&json!({ "error": "x" })).is_empty());
    }

    #[test]
    fn a_suggested_member_lists_the_tasks_it_holds() {
        let agents = json!([{ "cid": 4, "name": "Ben", "role": "builder", "status": "idle", "tasks": ["t2", "t5"] }]);
        assert_eq!(
            suggested_rows(&agents),
            [["4", "Ben", "builder", "idle", "t2, t5"]
                .map(String::from)
                .to_vec()]
        );
    }
}
