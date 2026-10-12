//! `agentplaza tasks`: Tasks: the work queue.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, confirm, Ctx};

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
        /// The budget.
        #[arg(long, default_value_t = 100)]
        budget: i64,
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
fn title_of(goal: &str) -> &str {
    goal.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Untitled task")
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
                println!("\n{}", ctx::s(t, "goal"));
                for (heading, key) in [("Acceptance", "acceptance"), ("Note", "note"), ("Outcome", "outcome")] {
                    let text = ctx::s(t, key);
                    if !text.is_empty() {
                        println!("\n{heading}\n{text}");
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
            let entries = ctx.client.get(&format!("/v1/agents/{cid}/transcript?after=0&limit=1000"))?;
            let kept: Vec<Value> = array(&entries)
                .iter()
                .filter(|e| TIMELINE_KINDS.iter().any(|k| *k == ctx::s(e, "kind")))
                .cloned()
                .collect();
            ctx.out(&json!(kept), |_| {
                for e in &kept {
                    let data: String = e.get("data").map(|d| d.to_string()).unwrap_or_default().chars().take(120).collect();
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
            let mut body = json!({ "goal": goal, "budget": budget });
            let acceptance = acceptance.filter(|a| !a.trim().is_empty()).map(|a| ctx::text_arg(&a)).transpose()?;
            if let Some(a) = acceptance {
                body["acceptance"] = json!(a);
            }
            if let Some(r) = role {
                body["role"] = json!(r);
            }
            if let Some(cid) = assign {
                body["assignee"] = json!(cid);
            }
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
