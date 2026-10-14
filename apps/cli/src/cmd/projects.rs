//! `agentplaza projects`: Projects and their repositories.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

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
    /// List them, with their stage.
    List,
    /// One project in full: its goal, documents, critiques and history.
    Show {
        /// The project's id.
        id: String,
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

/// The project's tasks in its order. The project lists them by id only, so each is joined with
/// the task list (its state and goal); an id the list does not show stays a bare id.
fn task_rows(project_tasks: &[Value], all_tasks: &[Value]) -> Vec<Value> {
    project_tasks
        .iter()
        .map(|t| {
            let id = t.as_str().unwrap_or_default();
            all_tasks.iter().find(|task| ctx::s(task, "id") == id).cloned().unwrap_or_else(|| json!({ "id": id }))
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
                            ctx::text(p, "repo"),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "title", "kind", "stage", "repo"], &rows);
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let p = ctx.client.get(&format!("/v1/projects/{}", ctx::seg(&id)))?;
            // The JSON is the hub's project as it is; the task list is read only for the table view.
            let tasks = if ctx.json {
                Vec::new()
            } else {
                let all = ctx.client.get("/v1/tasks")?;
                task_rows(array_at(&p, "tasks"), array_at(&all, "tasks"))
            };
            ctx.out(&p, |p| {
                ctx::fields(&[
                    ("id", ctx::text(p, "id")),
                    ("title", ctx::text(p, "title")),
                    ("kind", ctx::text(p, "kind")),
                    ("stage", ctx::text(p, "stage")),
                    ("repo", ctx::text(p, "repo")),
                    ("approved by", ctx::text(p, "approved_by")),
                    ("revision", ctx::text(p, "revision")),
                ]);
                println!("\n{}", ctx::s(p, "goal"));
                println!("\nTasks ({})", array_at(p, "tasks").len());
                for t in &tasks {
                    let goal = ctx::s(t, "goal");
                    let title = if goal.is_empty() { "(not in the task list)" } else { title_of(goal) };
                    println!("  {}  {}  {}", ctx::s(t, "id"), ctx::s(t, "state"), title);
                }
                println!("\nDocuments ({})", array_at(p, "docs").len());
                for d in array_at(p, "docs") {
                    // The file's id is what `library download` takes; a document without a file has none.
                    let file = ctx::text(d, "file");
                    let file = if file.is_empty() { "-".to_string() } else { file };
                    println!("  {}  {}  file {}: {}", ctx::s(d, "kind"), ctx::s(d, "title"), file, ctx::s(d, "summary"));
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
                    let note = if note.is_empty() { String::new() } else { format!(" ({note})") };
                    println!("  {}  {} {}{}", ctx::when(&e["at_ms"]), ctx::s(e, "by"), ctx::s(e, "what"), note);
                }
            });
            Ok(())
        }
        Cmd::Approve { id, note } => {
            let p = ctx.client.post(&format!("/v1/projects/{}/approve", ctx::seg(&id)), &json!({ "note": note }))?;
            ctx.out(&p, |_| println!("approved the plan of project {id}"));
            Ok(())
        }
        Cmd::SendBack { id, stage, reason } => {
            if reason.trim().is_empty() {
                return Err("give a reason so the team knows what to change".into());
            }
            let body = json!({ "stage": stage, "reason": reason.trim() });
            let p = ctx.client.post(&format!("/v1/projects/{}/veto", ctx::seg(&id)), &body)?;
            ctx.out(&p, |_| println!("sent project {id} back to {stage}"));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_tasks_keep_its_order_and_take_their_state_from_the_list() {
        let project_tasks = vec![json!("t2"), json!("t9"), json!("t1")];
        let all = vec![
            json!({ "id": "t1", "state": "done", "goal": "Write the plan\n\nDetails" }),
            json!({ "id": "t2", "state": "open", "goal": "Draw it" }),
        ];
        let rows = task_rows(&project_tasks, &all);
        let ids: Vec<&str> = rows.iter().map(|t| ctx::s(t, "id")).collect();
        assert_eq!(ids, vec!["t2", "t9", "t1"]);
        assert_eq!(ctx::s(&rows[0], "state"), "open");
        assert_eq!(ctx::s(&rows[1], "state"), "", "an unknown task stays a bare id");
        assert_eq!(title_of(ctx::s(&rows[2], "goal")), "Write the plan");
    }
}
