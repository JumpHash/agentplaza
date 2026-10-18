//! `agentplaza tasks`: Tasks: the work queue. Every task belongs to a project and goes only to that project's agents.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};
use crate::rich;

/// The board's lanes (the Tasks screen): open, in progress, done, failed or withdrawn.
const LANES: [&str; 4] = ["open", "in-progress", "done", "failed"];
/// The timeline's events: what the assignee's work started, called, ended or hit.
const TIMELINE_KINDS: [&str; 4] = ["work_start", "tool_request", "work_end", "error"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them, with their project, lane and assignee.
    List {
        /// Only this lane: open, in-progress, done or failed.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LANES))]
        lane: Option<String>,
        /// Only this project's tasks (its id, such as `pr3`).
        #[arg(long)]
        project: Option<String>,
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
    /// A project's tasks in the order their links let them run: a column per step of the order (the Work screen's
    /// columns), each task with its state and what it runs after; then the longest chain still to finish and the
    /// ready tasks nobody has claimed.
    Graph {
        /// The project's id (`pr1`), or `outside` for the tasks outside projects.
        project: String,
    },
    /// Create a task in a project. Without --assign the scheduler chooses the worker among the project's agents.
    Create {
        /// The title (the first line of the goal).
        title: String,
        /// The project it belongs to (required): its id, such as `pr3`. The project's plan must be approved.
        #[arg(long)]
        project: Option<String>,
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
        /// project's default, else the hub's (2000); the hub refuses a budget under 200.
        #[arg(long)]
        budget: Option<u64>,
        /// Assign it now to this agent (its cid): one of the project's agents.
        #[arg(long)]
        assign: Option<i64>,
    },
    /// Move a task from before projects into a project, with the tasks linked to it (links stay inside a project). A
    /// task never leaves its project once it has one.
    Move {
        /// The task's id.
        id: String,
        /// The project it moves into.
        #[arg(long)]
        project: String,
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

/// The project a task belongs to, or "-" for a task from before projects (`tasks move` puts it in one).
fn project_of(t: &Value) -> String {
    match ctx::s(t, "project") {
        "" => "-".to_string(),
        project => project.to_string(),
    }
}

/// The first line of a goal, the task's title.
pub(super) fn title_of(goal: &str) -> &str {
    goal.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Untitled task")
}

/// One task's timeline: the work_start entries that name the task (in their `origin`, or at the
/// top level, as the web and the desktop read them), and the timeline steps of those works.
/// Entries of the agent's other work are left out.
fn timeline_of(task_id: &str, entries: &[Value]) -> Vec<Value> {
    let names_task = |e: &Value| {
        let data = e.get("data");
        let origin_task = data
            .and_then(|d| d.get("origin"))
            .and_then(|o| o.get("task"))
            .and_then(Value::as_str);
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
            e.get("work")
                .and_then(Value::as_u64)
                .is_some_and(|w| works.contains(&w))
                && TIMELINE_KINDS.iter().any(|k| *k == ctx::s(e, "kind"))
        })
        .cloned()
        .collect()
}

/// A task's state as the Work screen says it: an open task is ready, waiting on the tasks it runs after, or blocked
/// by one that failed or was withdrawn; a claim whose agent waits on an approval needs you.
pub(crate) fn state_word(t: &Value) -> &str {
    let blocked = t
        .get("blocked_by")
        .and_then(Value::as_array)
        .is_some_and(|b| !b.is_empty());
    let ready = t.get("ready").and_then(Value::as_bool).unwrap_or(false);
    match ctx::s(t, "state") {
        "open" if blocked => "blocked",
        "open" if ready => "ready",
        "open" => "waiting",
        "claimed" if t.pointer("/step/stage").and_then(Value::as_str) == Some("approval") => {
            "needs you"
        }
        "claimed" => "working",
        "awaiting_review" => "in review",
        other => other,
    }
}

/// The read model's shape for the tasks outside projects, from the ledger: their links among themselves, and each
/// task's depth (the longest run of links behind it), as `GET /v1/projects/{id}/graph` gives a project's.
fn loose_graph(ledger: &Value) -> Value {
    let loose: Vec<&Value> = array_at(ledger, "tasks")
        .iter()
        .filter(|t| t.get("project").is_none_or(Value::is_null))
        .collect();
    let ids: Vec<&str> = loose.iter().map(|t| ctx::s(t, "id")).collect();
    let after = |t: &Value| -> Vec<String> {
        array_at(t, "after")
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| ids.contains(id))
            .map(str::to_string)
            .collect()
    };
    // Depths by rounds: a task is one deeper than its deepest link; a cycle (the hub refuses them) stops at the count.
    let mut depth: Vec<usize> = vec![0; loose.len()];
    for _ in 0..loose.len() {
        let mut moved = false;
        for (at, t) in loose.iter().enumerate() {
            let deepest = after(t)
                .iter()
                .filter_map(|id| ids.iter().position(|x| x == id))
                .map(|up| depth[up] + 1)
                .max()
                .unwrap_or(0);
            if deepest > depth[at] {
                depth[at] = deepest;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let mut order: Vec<usize> = (0..loose.len()).collect();
    order.sort_by_key(|&at| depth[at]);
    let tasks: Vec<Value> = order
        .iter()
        .map(|&at| {
            let mut t = loose[at].clone();
            t["depth"] = json!(depth[at]);
            t
        })
        .collect();
    let edges: Vec<Value> = loose
        .iter()
        .flat_map(|t| {
            after(t)
                .into_iter()
                .map(move |up| json!([up, ctx::s(t, "id")]))
        })
        .collect();
    json!({ "project": "outside", "tasks": tasks, "edges": edges })
}

/// The graph as lines: a heading per column, a row per task (id, state, role, title, and what it runs after or is
/// blocked by), then the longest chain still to finish and the ready tasks nobody has claimed.
fn graph_lines(g: &Value) -> Vec<String> {
    let tasks = array_at(g, "tasks");
    if tasks.is_empty() {
        let stage = ctx::s(g, "stage");
        return vec![if ctx::s(g, "project") == "outside" {
            "No tasks outside projects.".to_string()
        } else if ["brief", "research", "proposal", "critique", "approval"].contains(&stage) {
            format!(
                "No tasks yet: the project is at {stage}, and its tasks are posted once its plan is approved."
            )
        } else {
            "No tasks yet.".to_string()
        }];
    }
    let edges: Vec<(&str, &str)> = array_at(g, "edges")
        .iter()
        .filter_map(|e| Some((e.get(0)?.as_str()?, e.get(1)?.as_str()?)))
        .collect();
    let cut = |s: &str, n: usize| -> String {
        if s.chars().count() <= n {
            s.to_string()
        } else {
            s.chars().take(n - 1).collect::<String>() + "…"
        }
    };
    let row = |t: &Value| -> [String; 5] {
        let id = ctx::s(t, "id");
        let blocked: Vec<&str> = array_at(t, "blocked_by")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let after: Vec<&str> = edges
            .iter()
            .filter(|(_, to)| *to == id)
            .map(|(from, _)| *from)
            .collect();
        let links = if !blocked.is_empty() {
            format!("blocked by {}", blocked.join(", "))
        } else if !after.is_empty() {
            format!("after {}", after.join(", "))
        } else {
            String::new()
        };
        let role = match ctx::s(t, "role") {
            "" => "any role".to_string(),
            role => role.to_string(),
        };
        [
            id.to_string(),
            state_word(t).to_string(),
            role,
            cut(title_of(ctx::s(t, "goal")), 48),
            links,
        ]
    };
    let rows: Vec<(u64, [String; 5])> = tasks
        .iter()
        .map(|t| (t.get("depth").and_then(Value::as_u64).unwrap_or(0), row(t)))
        .collect();
    let width = |i: usize| {
        rows.iter()
            .map(|(_, r)| r[i].chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths = [width(0), width(1), width(2), width(3)];
    let mut lines = Vec::new();
    let deepest = rows.iter().map(|(d, _)| *d).max().unwrap_or(0);
    for column in 0..=deepest {
        let members: Vec<&[String; 5]> = rows
            .iter()
            .filter(|(d, _)| *d == column)
            .map(|(_, r)| r)
            .collect();
        if members.is_empty() {
            continue;
        }
        lines.push(if column == 0 {
            "Column 1 · runs after nothing".to_string()
        } else {
            format!("Column {}", column + 1)
        });
        for r in members {
            let mut line = String::from("  ");
            for (i, w) in widths.iter().enumerate() {
                line.push_str(&format!("{:<w$}  ", r[i], w = *w));
            }
            line.push_str(&r[4]);
            lines.push(line.trim_end().to_string());
        }
    }
    let chain: Vec<&str> = array_at(g, "longest_chain")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if chain.len() > 1 {
        lines.push(format!(
            "Longest chain still to finish: {} ({} tasks)",
            chain.join(" → "),
            chain.len()
        ));
    }
    let waiting: Vec<String> = array_at(g, "waiting")
        .iter()
        .map(|w| {
            let ids: Vec<&str> = array_at(w, "tasks")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let role = match ctx::s(w, "role") {
                "" => "any role",
                role => role,
            };
            format!("{role} ({})", ids.join(", "))
        })
        .collect();
    if !waiting.is_empty() {
        lines.push(format!("Ready, not claimed: {}", waiting.join(" · ")));
    }
    lines
}

/// What `tasks create` says without `--project`: a task belongs to a project, and how to find or open one.
const NO_PROJECT: &str = "a task belongs to a project: name it with --project <id> (`agentplaza projects list` shows them; `agentplaza projects create` or `agentplaza chat` opens one)";

/// The `POST /v1/tasks` body with only the fields the user gave. An omitted budget
/// is left out, so the hub applies its default (the project's, else the plaza's; the CLI holds no copy of them).
fn create_body(
    project: &str,
    goal: String,
    acceptance: Option<String>,
    role: Option<String>,
    budget: Option<u64>,
    assign: Option<i64>,
) -> Value {
    let mut body = json!({ "project": project, "goal": goal });
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
        Cmd::List { lane, project } => {
            let path = match &project {
                Some(id) => format!("/v1/tasks?project={}", ctx::seg(id)),
                None => "/v1/tasks".to_string(),
            };
            let hub = ctx.client.get(&path)?;
            let tasks: Vec<Value> = array_at(&hub, "tasks")
                .iter()
                .filter(|t| {
                    lane.as_deref()
                        .is_none_or(|l| lane_of(ctx::s(t, "state")) == l)
                })
                .cloned()
                .collect();
            ctx.out(&json!({ "tasks": tasks }), |_| {
                let rows: Vec<Vec<String>> = tasks
                    .iter()
                    .map(|t| {
                        vec![
                            ctx::s(t, "id").to_string(),
                            project_of(t),
                            lane_of(ctx::s(t, "state")).to_string(),
                            ctx::s(t, "state").to_string(),
                            ctx::text(t, "role"),
                            ctx::text(t, "agent_cid"),
                            title_of(ctx::s(t, "goal")).to_string(),
                        ]
                    })
                    .collect();
                ctx::table(
                    &["id", "project", "lane", "state", "role", "agent", "title"],
                    &rows,
                );
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let t = ctx.client.get(&format!("/v1/tasks/{}", ctx::seg(&id)))?;
            let rich_ctx = rich::context_for(ctx);
            ctx.out(&t, |t| {
                ctx::fields(&[
                    ("id", ctx::text(t, "id")),
                    ("project", project_of(t)),
                    ("state", ctx::text(t, "state")),
                    ("role", ctx::text(t, "role")),
                    ("budget", ctx::text(t, "budget")),
                    ("agent", ctx::text(t, "agent_cid")),
                    ("assignee", ctx::text(t, "assignee_id")),
                    ("created", ctx::when(&t["created_at"])),
                    ("updated", ctx::when(&t["updated_at"])),
                ]);
                println!(
                    "\n{}",
                    rich::render(ctx::s(t, "goal"), &rich_ctx, &ctx.rich)
                );
                for (heading, key) in [
                    ("Acceptance", "acceptance"),
                    ("Note", "note"),
                    ("Outcome", "outcome"),
                ] {
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
            let entries = ctx
                .client
                .get(&format!("/v1/agents/{cid}/transcript?last=1000"))?;
            let kept = timeline_of(&id, array(&entries));
            let rich_ctx = rich::context_for(ctx);
            ctx.out(&json!(kept), |_| {
                for e in &kept {
                    let data: String = e
                        .get("data")
                        .map(|d| d.to_string())
                        .unwrap_or_default()
                        .chars()
                        .take(120)
                        .collect();
                    let data = rich::render(&data, &rich_ctx, &ctx.rich);
                    println!(
                        "{:>5}  {:<14}  {}",
                        ctx::text(e, "seq"),
                        ctx::s(e, "kind"),
                        data
                    );
                }
            });
            Ok(())
        }
        Cmd::Graph { project } => {
            let graph = if project == "outside" {
                loose_graph(&ctx.client.get("/v1/tasks")?)
            } else {
                ctx.client
                    .get(&format!("/v1/projects/{}/graph", ctx::seg(&project)))?
            };
            ctx.out(&graph, |g| {
                for line in graph_lines(g) {
                    println!("{line}");
                }
            });
            Ok(())
        }
        Cmd::Create {
            title,
            project,
            details,
            acceptance,
            role,
            budget,
            assign,
        } => {
            let project = project
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .ok_or(NO_PROJECT)?;
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err("a task needs a title".into());
            }
            let details = details
                .filter(|d| !d.trim().is_empty())
                .map(|d| ctx::text_arg(&d))
                .transpose()?;
            let goal = match details {
                Some(d) => format!("{title}\n\n{d}"),
                None => title,
            };
            let acceptance = acceptance
                .filter(|a| !a.trim().is_empty())
                .map(|a| ctx::text_arg(&a))
                .transpose()?;
            let body = create_body(&project, goal, acceptance, role, budget, assign);
            let t = ctx.client.post("/v1/tasks", &body)?;
            ctx.out(&t, |t| {
                println!(
                    "created task {} in project {project} ({})",
                    ctx::s(t, "id"),
                    ctx::s(t, "state")
                )
            });
            Ok(())
        }
        Cmd::Move { id, project } => {
            let moved = ctx.client.post(
                &format!("/v1/tasks/{}/move", ctx::seg(&id)),
                &json!({ "project": project }),
            )?;
            ctx.out(&moved, |m| {
                let ids: Vec<&str> = array_at(m, "moved")
                    .iter()
                    .map(|t| ctx::s(t, "id"))
                    .collect();
                println!(
                    "moved {} into project {}",
                    if ids.is_empty() {
                        id.clone()
                    } else {
                        ids.join(", ")
                    },
                    ctx::s(m, "project")
                );
            });
            Ok(())
        }
        Cmd::Assign { id, cid } => {
            let w = ctx.client.post(
                &format!("/v1/agents/{cid}/assign"),
                &json!({ "task_id": id }),
            )?;
            ctx.out(&w, |w| {
                println!(
                    "assigned task {id} to agent {cid} (work {})",
                    ctx::text(w, "work")
                )
            });
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
        let bare = create_body("pr3", "t".into(), None, None, None, None);
        assert!(bare.get("budget").is_none(), "no budget field: {bare}");
        assert_eq!(bare["goal"], "t");
        assert_eq!(bare["project"], "pr3");
        let budgeted = create_body("pr3", "t".into(), None, None, Some(500), Some(7));
        assert_eq!(budgeted["budget"], 500);
        assert_eq!(budgeted["assignee"], 7);
        assert!(budgeted.get("acceptance").is_none());
    }

    #[test]
    fn create_without_a_project_says_how_to_name_one() {
        // The refusal comes before any request, so no hub is needed.
        let ctx = Ctx {
            client: crate::ctx::Client::new("http://127.0.0.1:9", None, None),
            json: false,
            rich: rich::Mode::default(),
        };
        let create = Cmd::Create {
            title: "Fix the flaky tests".into(),
            project: None,
            details: None,
            acceptance: None,
            role: None,
            budget: None,
            assign: None,
        };
        let refused = run(&ctx, create).unwrap_err();
        assert!(
            refused.starts_with("a task belongs to a project: name it with --project <id>"),
            "{refused}"
        );
    }

    #[test]
    fn a_task_names_its_project_or_none() {
        assert_eq!(project_of(&json!({ "id": "t1", "project": "pr3" })), "pr3");
        assert_eq!(project_of(&json!({ "id": "t1", "project": null })), "-");
    }

    /// A project's graph as `GET /v1/projects/{id}/graph` gives it.
    fn release() -> Value {
        json!({
            "project": "pr1", "stage": "implementation",
            "tasks": [
                { "id": "t1", "goal": "Survey the flaky tests", "role": "qa", "depth": 0, "state": "open", "ready": true, "blocked_by": [] },
                { "id": "t2", "goal": "Fix the double-charge\nmore", "role": "builder", "depth": 0, "state": "claimed", "ready": false, "blocked_by": [],
                  "agent_cid": 7, "step": { "stage": "approval", "tool": "plaza_pay", "since_ms": 0 } },
                { "id": "t3", "goal": "Make the tests deterministic", "role": null, "depth": 1, "state": "open", "ready": false, "blocked_by": [] },
                { "id": "t4", "goal": "Try the migration", "role": "qa", "depth": 1, "state": "open", "ready": false, "blocked_by": ["t9"] }
            ],
            "edges": [["t1", "t3"], ["t2", "t3"], ["t9", "t4"]],
            "frontier": ["t1", "t2"],
            "longest_chain": ["t1", "t3"],
            "waiting": [{ "role": "qa", "tasks": ["t1"] }]
        })
    }

    #[test]
    fn a_graph_prints_a_column_per_step_of_the_order() {
        let lines = graph_lines(&release());
        assert_eq!(
            lines,
            [
                "Column 1 · runs after nothing",
                "  t1  ready      qa        Survey the flaky tests",
                "  t2  needs you  builder   Fix the double-charge",
                "Column 2",
                "  t3  waiting    any role  Make the tests deterministic  after t1, t2",
                "  t4  blocked    qa        Try the migration             blocked by t9",
                "Longest chain still to finish: t1 → t3 (2 tasks)",
                "Ready, not claimed: qa (t1)",
            ]
        );
        assert_eq!(
            graph_lines(&json!({ "project": "pr2", "stage": "critique", "tasks": [] })),
            [
                "No tasks yet: the project is at critique, and its tasks are posted once its plan is approved."
            ]
        );
        assert_eq!(
            graph_lines(&json!({ "project": "outside", "tasks": [] })),
            ["No tasks outside projects."]
        );
    }

    #[test]
    fn the_tasks_outside_projects_get_their_depths_from_their_links() {
        let ledger = json!({ "tasks": [
            { "id": "t1", "goal": "a", "state": "open", "ready": true, "project": null, "after": [] },
            { "id": "t2", "goal": "b", "state": "open", "ready": false, "project": null, "after": ["t1", "t5"] },
            { "id": "t3", "goal": "c", "state": "open", "ready": false, "project": null, "after": ["t2"] },
            { "id": "t5", "goal": "d", "state": "open", "ready": true, "project": "pr1", "after": [] }
        ]});
        let g = loose_graph(&ledger);
        let depths: Vec<(String, u64)> = array_at(&g, "tasks")
            .iter()
            .map(|t| {
                (
                    ctx::s(t, "id").to_string(),
                    t["depth"].as_u64().unwrap_or(99),
                )
            })
            .collect();
        assert_eq!(
            depths,
            [("t1".into(), 0), ("t2".into(), 1), ("t3".into(), 2)]
        );
        assert_eq!(g["edges"], json!([["t1", "t2"], ["t2", "t3"]]));
    }

    #[test]
    fn a_task_says_its_state_as_the_work_screen_does() {
        let graph = release();
        let words: Vec<&str> = array_at(&graph, "tasks").iter().map(state_word).collect();
        assert_eq!(words, ["ready", "needs you", "waiting", "blocked"]);
        assert_eq!(
            state_word(&json!({ "state": "awaiting_review" })),
            "in review"
        );
        assert_eq!(state_word(&json!({ "state": "done" })), "done");
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
        let kept: Vec<u64> = timeline_of("t1", array(&entries))
            .iter()
            .map(|e| e["seq"].as_u64().unwrap_or(0))
            .collect();
        assert_eq!(kept, vec![1, 2, 7, 8]);
        assert!(timeline_of("t9", array(&entries)).is_empty());
    }
}
