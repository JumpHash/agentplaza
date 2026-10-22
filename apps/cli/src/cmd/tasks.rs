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
    /// One task in full: its goal, acceptance and outcome.
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
    /// A task's review: the newest review run in full (its verdict, the four readings, each step with its reviewer,
    /// the findings), then the earlier rounds.
    Review {
        /// The task's id.
        id: String,
    },
    /// Your ruling on a task's review: `pass`, `rework` (sent back to its worker) or `fail`.
    Rule {
        /// The task's id.
        id: String,
        /// pass, rework or fail.
        #[arg(value_parser = clap::builder::PossibleValuesParser::new(RULINGS))]
        decision: String,
        /// Why (optional).
        #[arg(long)]
        reason: Option<String>,
    },
}

/// The rulings on a review card: pass, send back to the worker, or fail.
const RULINGS: [&str; 3] = ["pass", "rework", "fail"];

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
        "rework" => "sent back",
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

/// A review run's state as people say it: a run sent back to its worker is "sent back".
fn run_state_word(state: &str) -> &str {
    match state {
        "rework" => "sent back",
        other => other,
    }
}

/// A reading's level: a signed number and a word, or "not measured" when no step measured it.
fn level_text(level: Option<&Value>) -> String {
    match level.and_then(Value::as_i64) {
        None => "not measured".to_string(),
        Some(n) => {
            let word = match n {
                ..=-4 => "much worse",
                -3..=-1 => "worse",
                0 => "same",
                1..=3 => "better",
                _ => "much better",
            };
            let sign = if n > 0 { "+" } else { "" };
            format!("{sign}{n} ({word})")
        }
    }
}

/// The findings of a review run, in the order of its steps.
fn findings_of(run: &Value) -> Vec<Value> {
    array_at(run, "nodes")
        .iter()
        .flat_map(|node| array_at(node, "findings").iter().cloned())
        .collect()
}

/// Rows as aligned columns, indented two spaces, two spaces apart (the last column is not padded).
fn aligned(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|row| row.get(c))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let mut line = String::from("  ");
            for (c, cell) in row.iter().enumerate() {
                if c + 1 < row.len() {
                    line.push_str(&format!("{cell:<w$}  ", w = widths[c]));
                } else {
                    line.push_str(cell);
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// A task's review as lines: the newest run in full (its state, round, recipe and verdict, the reasons the decision
/// gave, the four readings, each step with its state and reviewer, the findings), then the earlier rounds.
fn review_lines(reviews: &Value, findings: &[Value]) -> Vec<String> {
    let task = ctx::s(reviews, "task");
    let Some((newest, earlier)) = array_at(reviews, "runs").split_first() else {
        return vec![format!("No review of task {task} yet.")];
    };
    let mut lines = vec![
        format!(
            "Review of {task}: {}, round {} of {}, recipe {}",
            run_state_word(ctx::s(newest, "state")),
            ctx::text(newest, "round"),
            ctx::text(newest, "rounds"),
            ctx::s(newest, "recipe"),
        ),
        ctx::s(newest, "verdict").to_string(),
    ];
    if let Some(outcome) = newest.get("outcome").filter(|o| !o.is_null()) {
        let reasons: Vec<&str> = outcome["reasons"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !reasons.is_empty() {
            lines.push(format!(
                "{} decided {}: {}",
                ctx::s(outcome, "by"),
                ctx::s(outcome, "decision"),
                reasons.join("; ")
            ));
        }
    }
    lines.push(String::new());
    lines.push("Readings".to_string());
    let readings: Vec<Vec<String>> = array_at(newest, "readings")
        .iter()
        .map(|r| vec![ctx::s(r, "name").to_string(), level_text(r.get("level"))])
        .collect();
    lines.extend(aligned(&readings));
    lines.push(String::new());
    lines.push("Steps".to_string());
    let steps: Vec<Vec<String>> = array_at(newest, "nodes")
        .iter()
        .map(|n| {
            let role = match ctx::s(n, "role") {
                "" => "-",
                role => role,
            };
            let reviewer = n
                .get("assignee")
                .and_then(Value::as_u64)
                .map(|cid| format!("agent {cid}"))
                .unwrap_or_default();
            vec![
                ctx::s(n, "id").to_string(),
                ctx::s(n, "kind").to_string(),
                ctx::s(n, "state").to_string(),
                role.to_string(),
                reviewer,
            ]
        })
        .collect();
    lines.extend(aligned(&steps));
    lines.push(String::new());
    lines.push("Findings".to_string());
    if findings.is_empty() {
        lines.push("  none".to_string());
    } else {
        let rows: Vec<Vec<String>> = findings
            .iter()
            .map(|f| {
                let at = match ctx::s(f, "symbol") {
                    "" => ctx::s(f, "node"),
                    symbol => symbol,
                };
                vec![
                    ctx::s(f, "severity").to_string(),
                    at.to_string(),
                    ctx::s(f, "text").to_string(),
                ]
            })
            .collect();
        lines.extend(aligned(&rows));
    }
    if !earlier.is_empty() {
        lines.push(String::new());
        lines.push("Earlier rounds".to_string());
        let rows: Vec<Vec<String>> = earlier
            .iter()
            .map(|r| {
                vec![
                    format!(
                        "round {} of {}",
                        ctx::text(r, "round"),
                        ctx::text(r, "rounds")
                    ),
                    run_state_word(ctx::s(r, "state")).to_string(),
                    ctx::s(r, "verdict").to_string(),
                ]
            })
            .collect();
        lines.extend(aligned(&rows));
    }
    lines
}

/// The approval's answer to a review card: pass approves; rework and fail deny it, and rework sends the task back.
fn ruling_body(decision: &str, reason: Option<&str>) -> Value {
    let mut body = json!({ "approve": decision == "pass" });
    if decision == "rework" {
        body["action"] = json!("rework");
    }
    if let Some(reason) = reason {
        body["reason"] = json!(reason);
    }
    body
}

/// The review card of a task that waits for the owner: a pending approval of kind `review`, the newest if there are
/// several.
fn pending_review_of<'a>(approvals: &'a Value, task: &str) -> Option<&'a Value> {
    array(approvals)
        .iter()
        .filter(|a| {
            ctx::s(a, "kind") == "review"
                && ctx::s(a, "state") == "pending"
                && a.pointer("/detail/task").and_then(Value::as_str) == Some(task)
        })
        .max_by_key(|a| a.get("created_ms").and_then(Value::as_u64).unwrap_or(0))
}

/// What a ruling did to the task, in a sentence.
fn ruling_sentence(task: &str, decision: &str) -> String {
    match decision {
        "pass" => format!("passed the review of task {task}"),
        "rework" => format!("sent task {task} back to its worker"),
        _ => format!("failed the review of task {task}"),
    }
}

/// What `tasks create` says without `--project`: a task belongs to a project, and how to find or open one.
const NO_PROJECT: &str = "a task belongs to a project: name it with --project <id> (`agentplaza projects list` shows them; `agentplaza projects create` opens one)";

/// The `POST /v1/tasks` body with only the fields the user gave.
fn create_body(
    project: &str,
    goal: String,
    acceptance: Option<String>,
    role: Option<String>,
    assign: Option<i64>,
) -> Value {
    let mut body = json!({ "project": project, "goal": goal });
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
            let body = create_body(&project, goal, acceptance, role, assign);
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
        Cmd::Review { id } => {
            let reviews = ctx
                .client
                .get(&format!("/v1/tasks/{}/reviews", ctx::seg(&id)))?;
            // The list has the steps but not their findings: the newest run's own record has them (people only; the
            // raw answer of `--json` is the list).
            let findings = match array_at(&reviews, "runs").first() {
                Some(newest) if !ctx.json => findings_of(
                    &ctx.client
                        .get(&format!("/v1/reviews/{}", ctx::seg(ctx::s(newest, "run"))))?,
                ),
                _ => Vec::new(),
            };
            ctx.out(&reviews, |r| {
                for line in review_lines(r, &findings) {
                    println!("{line}");
                }
            });
            Ok(())
        }
        Cmd::Rule {
            id,
            decision,
            reason,
        } => {
            let approvals = ctx.client.get("/v1/approvals")?;
            let card = pending_review_of(&approvals, &id).ok_or_else(|| {
                format!(
                    "no review of task {id} is waiting for you (`agentplaza tasks review {id}` shows its last one)"
                )
            })?;
            let reason = reason
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty());
            let decided = ctx.client.post(
                &format!("/v1/approvals/{}", ctx::seg(ctx::s(card, "id"))),
                &ruling_body(&decision, reason.as_deref()),
            )?;
            ctx.out(&decided, |_| {
                println!("{}", ruling_sentence(&id, &decision))
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_never_sends_a_budget() {
        let bare = create_body("pr3", "t".into(), None, None, None);
        assert!(bare.get("budget").is_none(), "no budget field: {bare}");
        assert_eq!(bare["goal"], "t");
        assert_eq!(bare["project"], "pr3");
        let assigned = create_body("pr3", "t".into(), None, None, Some(7));
        assert_eq!(assigned["assignee"], 7);
        assert!(assigned.get("acceptance").is_none());
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
                  "agent_cid": 7, "step": { "stage": "approval", "tool": "bash", "since_ms": 0 } },
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
    /// `GET /v1/tasks/t1/reviews` as the hub answers it: two rounds, the newest first.
    const TASK_REVIEWS: &str = r##"{
  "task": "t1",
  "runs": [
    {
      "run": "rv2",
      "task": "t1",
      "change": "w-1",
      "revision": "w-1.r2",
      "recipe": "code@3",
      "round": 2,
      "rounds": 2,
      "kind": "code",
      "state": "passed",
      "verdict": "Every review step passed.",
      "outcome": {
        "decision": "pass",
        "reasons": [
          "fine for now; queues stay small"
        ],
        "replan": [],
        "at_ms": 1792351122547,
        "by": "owner"
      },
      "readings": [
        {
          "name": "what it does",
          "level": null
        },
        {
          "name": "cost to run",
          "level": null
        },
        {
          "name": "risk",
          "level": -3
        },
        {
          "name": "code health",
          "level": null
        }
      ],
      "nodes": [
        {
          "id": "tree",
          "kind": "tool",
          "role": null,
          "state": "passed",
          "assignee": null,
          "parent": null
        },
        {
          "id": "checks",
          "kind": "check",
          "role": "qa",
          "state": "passed",
          "assignee": 303,
          "parent": null
        },
        {
          "id": "function",
          "kind": "agent",
          "role": "reviewer",
          "state": "passed",
          "assignee": 301,
          "parent": null
        },
        {
          "id": "resources",
          "kind": "agent",
          "role": "reviewer",
          "state": "passed",
          "assignee": 301,
          "parent": null
        },
        {
          "id": "risk",
          "kind": "agent",
          "role": "security-reviewer",
          "state": "passed",
          "assignee": 302,
          "parent": null
        },
        {
          "id": "divergence",
          "kind": "tool",
          "role": null,
          "state": "passed",
          "assignee": null,
          "parent": null
        },
        {
          "id": "quality",
          "kind": "agent",
          "role": "qa",
          "state": "passed",
          "assignee": 303,
          "parent": null
        },
        {
          "id": "constraints",
          "kind": "agent",
          "role": "reviewer",
          "state": "passed",
          "assignee": 301,
          "parent": null
        },
        {
          "id": "gate",
          "kind": "gate",
          "role": null,
          "state": "failed",
          "assignee": null,
          "parent": null
        }
      ],
      "created_ms": 1792351118693,
      "updated_ms": 1792351122547
    },
    {
      "run": "rv1",
      "task": "t1",
      "change": "w-1",
      "revision": "w-1.r1",
      "recipe": "code@3",
      "round": 1,
      "rounds": 2,
      "kind": "code",
      "state": "rework",
      "verdict": "quality, gate did not pass; worse on tests -3.",
      "outcome": {
        "decision": "rework",
        "reasons": [
          "tests is -3 (rule: tst <= -3)",
          "quality did not pass"
        ],
        "replan": [],
        "at_ms": 1792351118401,
        "by": "gate"
      },
      "readings": [
        {
          "name": "what it does",
          "level": null
        },
        {
          "name": "cost to run",
          "level": null
        },
        {
          "name": "risk",
          "level": null
        },
        {
          "name": "code health",
          "level": -3
        }
      ],
      "nodes": [
        {
          "id": "tree",
          "kind": "tool",
          "role": null,
          "state": "passed",
          "assignee": null,
          "parent": null
        },
        {
          "id": "checks",
          "kind": "check",
          "role": "qa",
          "state": "passed",
          "assignee": 303,
          "parent": null
        },
        {
          "id": "function",
          "kind": "agent",
          "role": "reviewer",
          "state": "passed",
          "assignee": 301,
          "parent": null
        },
        {
          "id": "resources",
          "kind": "agent",
          "role": "reviewer",
          "state": "passed",
          "assignee": 301,
          "parent": null
        },
        {
          "id": "risk",
          "kind": "agent",
          "role": "security-reviewer",
          "state": "passed",
          "assignee": 302,
          "parent": null
        },
        {
          "id": "divergence",
          "kind": "tool",
          "role": null,
          "state": "passed",
          "assignee": null,
          "parent": null
        },
        {
          "id": "quality",
          "kind": "agent",
          "role": "qa",
          "state": "failed",
          "assignee": 303,
          "parent": null
        },
        {
          "id": "constraints",
          "kind": "agent",
          "role": "reviewer",
          "state": "passed",
          "assignee": 301,
          "parent": null
        },
        {
          "id": "gate",
          "kind": "gate",
          "role": null,
          "state": "failed",
          "assignee": null,
          "parent": null
        }
      ],
      "created_ms": 1792351113919,
      "updated_ms": 1792351118418
    }
  ]
}"##;

    #[test]
    fn a_review_prints_its_newest_run_then_the_earlier_rounds() {
        let reviews: Value = serde_json::from_str(TASK_REVIEWS).unwrap();
        assert_eq!(
            review_lines(&reviews, &[]),
            [
                "Review of t1: passed, round 2 of 2, recipe code@3",
                "Every review step passed.",
                "owner decided pass: fine for now; queues stay small",
                "",
                "Readings",
                "  what it does  not measured",
                "  cost to run   not measured",
                "  risk          -3 (worse)",
                "  code health   not measured",
                "",
                "Steps",
                "  tree         tool   passed  -",
                "  checks       check  passed  qa                 agent 303",
                "  function     agent  passed  reviewer           agent 301",
                "  resources    agent  passed  reviewer           agent 301",
                "  risk         agent  passed  security-reviewer  agent 302",
                "  divergence   tool   passed  -",
                "  quality      agent  passed  qa                 agent 303",
                "  constraints  agent  passed  reviewer           agent 301",
                "  gate         gate   failed  -",
                "",
                "Findings",
                "  none",
                "",
                "Earlier rounds",
                "  round 1 of 2  sent back  quality, gate did not pass; worse on tests -3.",
            ]
        );
    }

    #[test]
    fn findings_name_the_symbol_or_the_step_they_came_from() {
        let run = json!({ "nodes": [
            { "def": { "id": "quality" }, "findings": [
                { "symbol": "src/queue.rs::impl Queue › fn next", "text": "next() has no test", "severity": "major", "node": "quality", "round": 1 }
            ] },
            { "def": { "id": "risk" }, "findings": [
                { "symbol": "", "text": "a secret in the log", "severity": "critical", "node": "risk", "round": 1 }
            ] },
            { "def": { "id": "tree" }, "findings": [] }
        ]});
        let reviews: Value = serde_json::from_str(TASK_REVIEWS).unwrap();
        let lines = review_lines(&reviews, &findings_of(&run));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("  major") && l.ends_with("next() has no test")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("  critical")
                && l.contains("risk")
                && l.ends_with("a secret in the log")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_review_that_has_not_run_says_so() {
        let none = json!({ "task": "t7", "runs": [] });
        assert_eq!(review_lines(&none, &[]), ["No review of task t7 yet."]);
    }

    #[test]
    fn a_ruling_answers_the_review_card_as_the_hub_reads_it() {
        assert_eq!(ruling_body("pass", None), json!({ "approve": true }));
        assert_eq!(
            ruling_body("rework", Some("tests first")),
            json!({ "approve": false, "action": "rework", "reason": "tests first" })
        );
        assert_eq!(ruling_body("fail", None), json!({ "approve": false }));
    }

    #[test]
    fn the_ruling_finds_the_pending_review_card_of_the_task() {
        let approvals = json!([
            { "id": "a0", "kind": "sanction", "state": "pending", "created_ms": 5, "detail": { "task": "t1" } },
            { "id": "a1", "kind": "review", "state": "approved", "created_ms": 6, "detail": { "task": "t1", "run": "rv1" } },
            { "id": "a2", "kind": "review", "state": "pending", "created_ms": 7, "detail": { "task": "t1", "run": "rv2" } },
            { "id": "a3", "kind": "review", "state": "pending", "created_ms": 8, "detail": { "task": "t9", "run": "rv3" } }
        ]);
        assert_eq!(
            pending_review_of(&approvals, "t1").map(|a| ctx::s(a, "id")),
            Some("a2")
        );
        assert!(pending_review_of(&approvals, "t5").is_none());
        assert_eq!(state_word(&json!({ "state": "rework" })), "sent back");
    }
}
