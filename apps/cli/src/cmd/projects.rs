//! `agentplaza projects`: Projects. The owner opens one (its plan theirs), puts agents on it from the bench, pauses,
//! resumes and archives it, and sets its settings and budget; the agents' plans go through their stages, which the
//! owner approves or sends back. The submodules hold the members (`members`), and the settings and the budget
//! (`settings`).

pub mod members;
pub mod settings;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};

use super::tasks::title_of;

/// The stages a project goes through, in order (the pipeline on the Projects screen).
const STAGES: [&str; 8] = [
    "brief",
    "research",
    "proposal",
    "critique",
    "approval",
    "implementation",
    "review",
    "delivered",
];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them, with their stage and whether they are active, paused, archived or delivered.
    List,
    /// One project in full: its goal and state, its tasks and members, its documents, critiques and history.
    Show {
        /// The project's id.
        id: String,
    },
    /// Open a project as the owner: its plan is yours, so it is at implementation at once and takes tasks. Put agents
    /// on it with `projects members ID add CID`; `agentplaza chat` drafts, staffs and plans one with the orchestrator.
    Create {
        /// The project's title.
        title: String,
        /// What it is for: the goal its tasks work towards. `-` reads it from stdin.
        #[arg(long)]
        goal: String,
        /// `code` (it gets a repository; the hub's default) or `other`.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["code", "other"]))]
        kind: Option<String>,
    },
    /// A project's agents. Add `add CID` to put an agent on it (from the bench, or from another project, whose work
    /// stops), `remove CID` to send one back to the bench, or `suggested` for the agents that hold its tasks.
    Members {
        /// The project's id.
        id: String,
        #[command(subcommand)]
        action: Option<members::Action>,
    },
    /// Pause a project: its agents' work waits until it is resumed.
    Pause {
        /// The project's id.
        id: String,
    },
    /// Resume a paused project: its agents' waiting work goes out.
    Resume {
        /// The project's id.
        id: String,
    },
    /// Archive a project for good: its open tasks are withdrawn, its agents go back to the bench, and the rest of its
    /// budget goes back to the plaza. Asks to confirm unless --yes.
    Archive {
        /// The project's id.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// A project's settings: the plaza's settings it overrides for its agents. Add `effective` for every value they
    /// get and where it comes from, or `set KEY=VALUE…` and `unset KEY…` to change them.
    Settings {
        /// The project's id.
        id: String,
        #[command(subcommand)]
        action: Option<settings::Action>,
    },
    /// A project's budget: credits set aside from the plaza's budget for its tasks. Add `add CREDITS` to set more
    /// aside, or `return CREDITS` to give some back.
    Budget {
        /// The project's id.
        id: String,
        #[command(subcommand)]
        action: Option<settings::BudgetAction>,
    },
    /// Approve the project's plan (the owner's approval), with an optional note.
    Approve {
        /// The project's id.
        id: String,
        /// A note for the team.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Send a project back to an earlier stage, with the reason the team must act on.
    SendBack {
        /// The project's id.
        id: String,
        /// The stage it returns to (an earlier one than its current stage).
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(STAGES))]
        stage: String,
        /// What must change, and why (required).
        #[arg(long)]
        reason: String,
    },
}

/// The objects of a JSON array.
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The objects of a JSON array under `key`.
fn array_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).map(array).unwrap_or_default()
}

/// Where a project stands for its agents: `archived` or `delivered` (it is over, and they are back on the bench),
/// `paused` (their work waits), or `active`.
pub(crate) fn state_of(p: &Value) -> &'static str {
    let flag = |key: &str| p.get(key).and_then(Value::as_bool).unwrap_or(false);
    if flag("archived") {
        "archived"
    } else if ctx::s(p, "stage") == "delivered" {
        "delivered"
    } else if flag("paused") {
        "paused"
    } else {
        "active"
    }
}

/// The `POST /v1/projects` body: the title and goal, and the kind only when one is given (the hub's default is code).
fn create_body(title: &str, goal: &str, kind: Option<&str>) -> Value {
    let mut body = json!({ "title": title, "goal": goal });
    if let Some(kind) = kind {
        body["kind"] = json!(kind);
    }
    body
}

/// The project's tasks in its order. The project lists them by id only, so each is joined with
/// the task list (its state and goal); an id the list does not show stays a bare id.
fn task_rows(project_tasks: &[Value], all_tasks: &[Value]) -> Vec<Value> {
    project_tasks
        .iter()
        .map(|t| {
            let id = t.as_str().unwrap_or_default();
            all_tasks
                .iter()
                .find(|task| ctx::s(task, "id") == id)
                .cloned()
                .unwrap_or_else(|| json!({ "id": id }))
        })
        .collect()
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let projects = ctx.client.get("/v1/projects")?;
            ctx.out(&projects, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|p| {
                        vec![
                            ctx::s(p, "id").to_string(),
                            ctx::s(p, "title").to_string(),
                            ctx::s(p, "kind").to_string(),
                            ctx::s(p, "stage").to_string(),
                            state_of(p).to_string(),
                            ctx::text(p, "repo"),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "title", "kind", "stage", "state", "repo"], &rows);
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let p = ctx.client.get(&format!("/v1/projects/{}", ctx::seg(&id)))?;
            // The JSON is the hub's project as it is; the task list and the members are read only for the table view.
            let (tasks, members) = if ctx.json {
                (Vec::new(), Value::Null)
            } else {
                let all = ctx.client.get("/v1/tasks")?;
                let members = ctx
                    .client
                    .get(&format!("/v1/projects/{}/members", ctx::seg(&id)))?;
                (
                    task_rows(array_at(&p, "tasks"), array_at(&all, "tasks")),
                    members,
                )
            };
            ctx.out(&p, |p| {
                ctx::fields(&[
                    ("id", ctx::text(p, "id")),
                    ("title", ctx::text(p, "title")),
                    ("kind", ctx::text(p, "kind")),
                    ("stage", ctx::text(p, "stage")),
                    ("state", state_of(p).to_string()),
                    ("repo", ctx::text(p, "repo")),
                    ("approved by", ctx::text(p, "approved_by")),
                    ("revision", ctx::text(p, "revision")),
                ]);
                println!("\n{}", ctx::s(p, "goal"));
                println!("\nTasks ({})", array_at(p, "tasks").len());
                for t in &tasks {
                    let goal = ctx::s(t, "goal");
                    let title = if goal.is_empty() {
                        "(not in the task list)"
                    } else {
                        title_of(goal)
                    };
                    println!("  {}  {}  {}", ctx::s(t, "id"), ctx::s(t, "state"), title);
                }
                println!("\nMembers ({})", array(&members).len());
                for a in array(&members) {
                    let held = match ctx::s(a, "held") {
                        "" => String::new(),
                        why => format!(" (held: {why})"),
                    };
                    println!(
                        "  {}  {}  {}  {}{held}",
                        ctx::text(a, "cid"),
                        super::agents::name_of(a),
                        ctx::s(a, "role"),
                        ctx::s(a, "status")
                    );
                }
                println!("\nDocuments ({})", array_at(p, "docs").len());
                for d in array_at(p, "docs") {
                    // The file's id is what `library download` takes; a document without a file has none.
                    let file = ctx::text(d, "file");
                    let file = if file.is_empty() {
                        "-".to_string()
                    } else {
                        file
                    };
                    println!(
                        "  {}  {}  file {}: {}",
                        ctx::s(d, "kind"),
                        ctx::s(d, "title"),
                        file,
                        ctx::s(d, "summary")
                    );
                }
                println!("\nCritiques ({})", array_at(p, "critiques").len());
                for c in array_at(p, "critiques") {
                    println!(
                        "  revision {} {} by {}: {}",
                        ctx::text(c, "revision"),
                        ctx::s(c, "verdict"),
                        ctx::s(c, "by"),
                        ctx::s(c, "text")
                    );
                }
                println!("\nHistory");
                for e in array_at(p, "history") {
                    let note = ctx::s(e, "note");
                    let note = if note.is_empty() {
                        String::new()
                    } else {
                        format!(" ({note})")
                    };
                    println!(
                        "  {}  {} {}{}",
                        ctx::when(&e["at_ms"]),
                        ctx::s(e, "by"),
                        ctx::s(e, "what"),
                        note
                    );
                }
            });
            Ok(())
        }
        Cmd::Create { title, goal, kind } => {
            let title = title.trim();
            if title.is_empty() {
                return Err("a project needs a title".into());
            }
            let goal = ctx::text_arg(&goal)?;
            let p = ctx
                .client
                .post("/v1/projects", &create_body(title, &goal, kind.as_deref()))?;
            ctx.out(&p, |p| {
                let id = ctx::s(p, "id");
                println!("opened project {id} ({})", ctx::s(p, "stage"));
                println!("put agents on it: agentplaza projects members {id} add <cid>");
            });
            Ok(())
        }
        Cmd::Members { id, action } => members::members(ctx, &id, action),
        Cmd::Pause { id } => lifecycle(ctx, &id, "pause", "paused project"),
        Cmd::Resume { id } => lifecycle(ctx, &id, "resume", "resumed project"),
        Cmd::Archive { id, yes } => {
            let question = format!(
                "Archive project {id}? Its open tasks are withdrawn, its agents go back to the bench, and the rest of its budget goes back to the plaza."
            );
            if !confirm(yes, &question)? {
                println!("cancelled");
                return Ok(());
            }
            lifecycle(ctx, &id, "archive", "archived project")
        }
        Cmd::Settings { id, action } => settings::settings(ctx, &id, action),
        Cmd::Budget { id, action } => settings::budget(ctx, &id, action),
        Cmd::Approve { id, note } => {
            let p = ctx.client.post(
                &format!("/v1/projects/{}/approve", ctx::seg(&id)),
                &json!({ "note": note }),
            )?;
            ctx.out(&p, |_| println!("approved the plan of project {id}"));
            Ok(())
        }
        Cmd::SendBack { id, stage, reason } => {
            if reason.trim().is_empty() {
                return Err("give a reason so the team knows what to change".into());
            }
            let body = json!({ "stage": stage, "reason": reason.trim() });
            let p = ctx
                .client
                .post(&format!("/v1/projects/{}/veto", ctx::seg(&id)), &body)?;
            ctx.out(&p, |_| println!("sent project {id} back to {stage}"));
            Ok(())
        }
    }
}

/// Pauses, resumes or archives project `id` (`verb` is the route's last segment), and says so with `done`.
fn lifecycle(ctx: &Ctx, id: &str, verb: &str, done: &str) -> Result<(), String> {
    let p = ctx
        .client
        .post(&format!("/v1/projects/{}/{verb}", ctx::seg(id)), &json!({}))?;
    ctx.out(&p, |_| println!("{done} {id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_active_paused_or_over() {
        assert_eq!(state_of(&json!({ "stage": "implementation" })), "active");
        assert_eq!(
            state_of(&json!({ "stage": "implementation", "paused": true })),
            "paused"
        );
        assert_eq!(
            state_of(&json!({ "stage": "delivered", "paused": true })),
            "delivered"
        );
        assert_eq!(
            state_of(&json!({ "stage": "review", "paused": true, "archived": true })),
            "archived"
        );
    }

    #[test]
    fn create_names_the_kind_only_when_one_is_given() {
        assert_eq!(
            create_body("Release 0.9", "Tag it", None),
            json!({ "title": "Release 0.9", "goal": "Tag it" })
        );
        assert_eq!(
            create_body("Notes", "Write them", Some("other"))["kind"],
            "other"
        );
    }
}
