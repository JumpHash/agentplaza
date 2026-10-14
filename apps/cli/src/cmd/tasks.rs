//! `agentplaza tasks`: Tasks: the work queue.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, confirm, Ctx};
use crate::rich;

/// The board's lanes (the Tasks screen): open, in progress, done, failed or withdrawn.
const LANES: [&str; 4] = ["open", "in-progress", "done", "failed"];
/// The timeline's events: what the assignee's work started, called, ended or hit.
const TIMELINE_KINDS: [&str; 4] = ["work_start", "tool_request", "work_end", "error"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them, with their lane and assignee.
    List {
        /// Only this lane: open, in-progress, done or failed.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LANES))]
        lane: Option<String>,
    },
    /// One task in full: its goal, acceptance, budget and outcome.
    Show {
        /// The task's id.
        id: String,
    },
    /// The timeline of a task's work, from its assignee's transcript.
    Timeline {
        /// The task's id.
        id: String,
    },
    /// Create a task. Without --assign the scheduler chooses the worker.
    Create {
        /// The title (the first line of the goal).
        title: String,
        /// The details under the title; `-` reads stdin.
        #[arg(long)]
        details: Option<String>,
        /// What counts as done; `-` reads stdin.
        #[arg(long)]
        acceptance: Option<String>,
        /// The role that should do it.
        #[arg(long)]
        role: Option<String>,
        /// The budget in credits, the escrow that pays the worker's work. Omit it for the
        /// hub's default (2000); the hub refuses a budget under 200.
        #[arg(long)]
        budget: Option<u64>,
        /// Assign it now to this agent (its cid).
        #[arg(long)]
        assign: Option<i64>,
    },
    /// Assign an open task to an idle worker agent (by its cid).
    Assign {
        /// The task's id.
        id: String,
        /// The agent's cid.
        cid: i64,
    },
    /// Withdraw a task that has not finished.
    Withdraw {
        /// The task's id.
        id: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
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

/// The lane a state belongs to; unknown states count as in progress.
fn lane_of(state: &str) -> &'static str {
    match state {
        "open" => "open",
        "done" => "done",
        "failed" | "withdrawn" | "cancelled" => "failed",
        _ => "in-progress",
    }
}

/// The first line of a goal, the task's title.
pub(super) fn title_of(goal: &str) -> &str {
    goal.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Untitled task")
}

/// One task's timeline: the work_start entries that name the task (in their `origin`, or at the
/// top level, as the web and the desktop read them), and the timeline steps of those works.
/// Entries of the agent's other work are left out.
fn timeline_of(task_id: &str, entries: &[Value]) -> Vec<Value> {
    let names_task = |e: &Value| {
        let data = e.get("data");
        let origin_task = data.and_then(|d| d.get("origin")).and_then(|o| o.get("task")).and_then(Value::as_str);
        let top_task = data.and_then(|d| d.get("task")).and_then(Value::as_str);
        origin_task == Some(task_id) || top_task == Some(task_id)
    };
    let works: Vec<u64> = entries
        .iter()
        .filter(|e| ctx::s(e, "kind") == "work_start" && names_task(e))
        .filter_map(|e| e.get("work").and_then(Value::as_u64))
        .collect();
    entries
        .iter()
        .filter(|e| {
            e.get("work").and_then(Value::as_u64).is_some_and(|w| works.contains(&w))
                && TIMELINE_KINDS.iter().any(|k| *k == ctx::s(e, "kind"))
        })
        .cloned()
        .collect()
}

/// The `POST /v1/tasks` body with only the fields the user gave. An omitted budget
/// is left out, so the hub applies its default (the CLI holds no copy of it).
fn create_body(goal: String, acceptance: Option<String>, role: Option<String>, budget: Option<u64>, assign: Option<i64>) -> Value {
    let mut body = json!({ "goal": goal });
    if let Some(budget) = budget {
        body["budget"] = json!(budget);
    }
    if let Some(a) = acceptance {
        body["acceptance"] = json!(a);
    }
    if let Some(r) = role {
        body["role"] = json!(r);
    }
    if let Some(cid) = assign {
        body["assignee"] = json!(cid);
    }
    body
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { lane } => {
            let hub = ctx.client.get("/v1/tasks")?;
            let tasks: Vec<Value> = array_at(&hub, "tasks")
                .iter()
                .filter(|t| lane.as_deref().map_or(true, |l| lane_of(ctx::s(t, "state")) == l))
                .cloned()
                .collect();
            ctx.out(&json!({ "tasks": tasks }), |_| {
                let rows: Vec<Vec<String>> = tasks
                    .iter()
                    .map(|t| {
                        vec![
                            ctx::s(t, "id").to_string(),
                            lane_of(ctx::s(t, "state")).to_string(),
                            ctx::s(t, "state").to_string(),
                            ctx::text(t, "role"),
                            ctx::text(t, "agent_cid"),
                            title_of(ctx::s(t, "goal")).to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "lane", "state", "role", "agent", "title"], &rows);
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let t = ctx.client.get(&format!("/v1/tasks/{}", ctx::seg(&id)))?;
            let rich_ctx = rich::context_for(ctx);
            ctx.out(&t, |t| {
                ctx::fields(&[
                    ("id", ctx::text(t, "id")),
                    ("state", ctx::text(t, "state")),
                    ("role", ctx::text(t, "role")),
                    ("budget", ctx::text(t, "budget")),
                    ("agent", ctx::text(t, "agent_cid")),
                    ("assignee", ctx::text(t, "assignee_id")),
                    ("created", ctx::when(&t["created_at"])),
                    ("updated", ctx::when(&t["updated_at"])),
                ]);
                println!("\n{}", rich::render(ctx::s(t, "goal"), &rich_ctx, &ctx.rich));
                for (heading, key) in [("Acceptance", "acceptance"), ("Note", "note"), ("Outcome", "outcome")] {
                    let text = ctx::s(t, key);
                    if !text.is_empty() {
                        println!("\n{heading}\n{}", rich::render(text, &rich_ctx, &ctx.rich));
                    }
                }
            });
            Ok(())
        }
        Cmd::Timeline { id } => {
            let task = ctx.client.get(&format!("/v1/tasks/{}", ctx::seg(&id)))?;
            let cid = task
                .get("agent_cid")
                .and_then(Value::as_i64)
                .ok_or("this task has no agent yet, so it has no timeline")?;
            // The newest 1000 entries (the hub's `last`), not the oldest.
            let entries = ctx.client.get(&format!("/v1/agents/{cid}/transcript?last=1000"))?;
            let kept = timeline_of(&id, array(&entries));
            let rich_ctx = rich::context_for(ctx);
            ctx.out(&json!(kept), |_| {
                for e in &kept {
                    let data: String = e.get("data").map(|d| d.to_string()).unwrap_or_default().chars().take(120).collect();
                    let data = rich::render(&data, &rich_ctx, &ctx.rich);
                    println!("{:>5}  {:<14}  {}", ctx::text(e, "seq"), ctx::s(e, "kind"), data);
                }
            });
            Ok(())
        }
        Cmd::Create { title, details, acceptance, role, budget, assign } => {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err("a task needs a title".into());
            }
            let details = details.filter(|d| !d.trim().is_empty()).map(|d| ctx::text_arg(&d)).transpose()?;
            let goal = match details {
                Some(d) => format!("{title}\n\n{d}"),
                None => title,
            };
            let acceptance = acceptance.filter(|a| !a.trim().is_empty()).map(|a| ctx::text_arg(&a)).transpose()?;
            let body = create_body(goal, acceptance, role, budget, assign);
            let t = ctx.client.post("/v1/tasks", &body)?;
            ctx.out(&t, |t| println!("created task {} ({})", ctx::s(t, "id"), ctx::s(t, "state")));
            Ok(())
        }
        Cmd::Assign { id, cid } => {
            let w = ctx.client.post(&format!("/v1/agents/{cid}/assign"), &json!({ "task_id": id }))?;
            ctx.out(&w, |w| println!("assigned task {id} to agent {cid} (work {})", ctx::text(w, "work")));
            Ok(())
        }
        Cmd::Withdraw { id, yes } => {
            if !confirm(yes, &format!("Withdraw task {id}."))? {
                return Err("not done".into());
            }
            let t = ctx.client.delete(&format!("/v1/tasks/{}", ctx::seg(&id)))?;
            ctx.out(&t, |_| println!("withdrew task {id}"));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_omits_the_budget_when_none_is_given() {
        let bare = create_body("t".into(), None, None, None, None);
        assert!(bare.get("budget").is_none(), "no budget field: {bare}");
        assert_eq!(bare["goal"], "t");
        let budgeted = create_body("t".into(), None, None, Some(500), Some(7));
        assert_eq!(budgeted["budget"], 500);
        assert_eq!(budgeted["assignee"], 7);
        assert!(budgeted.get("acceptance").is_none());
    }

    #[test]
    fn a_timeline_keeps_only_its_tasks_work() {
        let entries = json!([
            { "seq": 1, "work": 1, "kind": "work_start", "data": { "origin": { "task": "t1" } } },
            { "seq": 2, "work": 1, "kind": "tool_request", "data": {} },
            { "seq": 3, "work": 2, "kind": "work_start", "data": { "origin": { "task": "t2" } } },
            { "seq": 4, "work": 2, "kind": "tool_request", "data": {} },
            { "seq": 5, "work": 3, "kind": "work_start", "data": { "kind": "chat" } },
            { "seq": 6, "work": 3, "kind": "work_end", "data": {} },
            { "seq": 7, "work": 1, "kind": "work_end", "data": { "status": "done" } },
            { "seq": 8, "work": 4, "kind": "work_start", "data": { "task": "t1" } },
            { "seq": 9, "work": 4, "kind": "message", "data": {} }
        ]);
        let kept: Vec<u64> = timeline_of("t1", array(&entries)).iter().map(|e| e["seq"].as_u64().unwrap_or(0)).collect();
        assert_eq!(kept, vec![1, 2, 7, 8]);
        assert!(timeline_of("t9", array(&entries)).is_empty());
    }
}
