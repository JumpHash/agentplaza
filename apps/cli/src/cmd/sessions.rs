//! `agentplaza sessions`: the work engine's sessions, each a fresh context working one seat's step, a project's planner
//! or the main orchestrator. The list shows the live sessions first, then the recent ones; `show`, `context` and
//! `transcript` read one session, and `stop` ends it (it hands over, and its seat goes again). A session runs on a VM:
//! its cid is the VM's, which `agents` takes for the VM's own commands.

use std::thread;
use std::time::Duration;

use apz_rich::Ctx as RichCtx;
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};

use crate::cmd::agents::cancelled;
use crate::ctx::{self, Ctx, confirm};
use crate::rich;

/// The states a session can be in (the hub's `state`), in the order of its life.
const STATES: [&str; 10] = [
    "queued",
    "placing",
    "forking",
    "running",
    "paused",
    "compacting",
    "handing_over",
    "ending",
    "ended",
    "failed",
];

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Only the sessions of this task.
    #[arg(long)]
    pub task: Option<String>,
    /// Only the sessions of this project (its id, such as `pr3`).
    #[arg(long)]
    pub project: Option<String>,
    /// Only the sessions in this state.
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(STATES))]
    pub state: Option<String>,
    /// Only the live sessions: not ended or failed.
    #[arg(long)]
    pub live: bool,
    #[command(subcommand)]
    pub action: Option<Action>,
}

#[derive(Subcommand, Debug)]
pub enum Action {
    /// One session in full: its state, seat, task, project, model, node, usage and how it ended.
    Show {
        /// The session's id, `s<n>`.
        id: String,
    },
    /// A session's context: how full its last model call's context was, by part and by tool.
    Context {
        /// The session's id.
        id: String,
    },
    /// A session's transcript: what it did and said, oldest first. `--follow` keeps printing new entries.
    Transcript {
        /// The session's id.
        id: String,
        /// Keep printing the entries as they come.
        #[arg(long)]
        follow: bool,
    },
    /// Stop a session: it ends with its handover, and its seat goes again. Asks to confirm unless --yes.
    Stop {
        /// The session's id.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    match args.action {
        None => list(ctx, &args),
        Some(Action::Show { id }) => show(ctx, &id),
        Some(Action::Context { id }) => context(ctx, &id),
        Some(Action::Transcript { id, follow }) => transcript(ctx, &id, follow),
        Some(Action::Stop { id, yes }) => stop(ctx, &id, yes),
    }
}

/// The items of a hub array (none when the value is not one).
fn items(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

/// Whether a session is still working: any state but `ended` and `failed`.
pub(crate) fn is_live(session: &Value) -> bool {
    !matches!(ctx::s(session, "state"), "ended" | "failed")
}

/// The sessions as the list shows them: the live ones first, each group in the order the hub sent it (newest first).
fn live_first(mut sessions: Vec<Value>) -> Vec<Value> {
    sessions.sort_by_key(|s| !is_live(s));
    sessions
}

/// The list's path: the filters the flags name, `live=true` only when asked for.
fn list_path(project: Option<&str>, task: Option<&str>, state: Option<&str>, live: bool) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(project) = project {
        query.push(format!("project={}", ctx::seg(project)));
    }
    if let Some(task) = task {
        query.push(format!("task={}", ctx::seg(task)));
    }
    if let Some(state) = state {
        query.push(format!("state={}", ctx::seg(state)));
    }
    if live {
        query.push("live=true".to_string());
    }
    if query.is_empty() {
        "/v1/sessions".to_string()
    } else {
        format!("/v1/sessions?{}", query.join("&"))
    }
}

/// A session's state as a word: its state, and why it is paused when it is.
fn state_text(session: &Value) -> String {
    match session.get("paused_by").and_then(Value::as_str) {
        Some(why) => format!("{} ({why})", ctx::s(session, "state")),
        None => ctx::s(session, "state").to_string(),
    }
}

/// A text field, or `-` when it is missing or empty.
fn or_dash(v: &Value, key: &str) -> String {
    match ctx::text(v, key) {
        text if text.is_empty() => "-".to_string(),
        text => text,
    }
}

/// A count field (0 when missing).
fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn list(ctx: &Ctx, args: &Args) -> Result<(), String> {
    let path = list_path(
        args.project.as_deref(),
        args.task.as_deref(),
        args.state.as_deref(),
        args.live,
    );
    let sessions = live_first(items(&ctx.client.get(&path)?));
    ctx.out(&Value::Array(sessions.clone()), |_| {
        let rows: Vec<Vec<String>> = sessions
            .iter()
            .map(|s| {
                vec![
                    ctx::text(s, "id"),
                    state_text(s),
                    or_dash(s, "seat"),
                    or_dash(s, "task"),
                    or_dash(s, "project"),
                    or_dash(s, "model"),
                    or_dash(s, "cid"),
                    cost_text(s),
                ]
            })
            .collect();
        ctx::table(
            &[
                "session", "state", "seat", "task", "project", "model", "vm", "cost",
            ],
            &rows,
        );
    });
    Ok(())
}

/// What a session has cost, in dollars.
pub(crate) fn cost_text(session: &Value) -> String {
    let micros = session
        .pointer("/usage/cost_micros")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    crate::cmd::budget::dollars(micros)
}

/// One session as `sessions show` lines.
fn session_lines(s: &Value) -> Vec<(&'static str, String)> {
    let usage = s.get("usage").unwrap_or(&Value::Null);
    let mut lines = vec![
        ("session", ctx::text(s, "id")),
        ("state", state_text(s)),
        ("seat", or_dash(s, "seat")),
        ("role", or_dash(s, "role")),
        ("task", or_dash(s, "task")),
        ("project", or_dash(s, "project")),
        ("model", or_dash(s, "model")),
        ("node", or_dash(s, "node")),
        ("vm", or_dash(s, "cid")),
        ("attempt", ctx::text(s, "attempt")),
        (
            "usage",
            format!(
                "{} in · {} out · {} cached tokens · {} · {} model calls · {} tool calls",
                count(usage, "tokens_in"),
                count(usage, "tokens_out"),
                count(usage, "tokens_cached"),
                cost_text(s),
                count(usage, "model_calls"),
                count(usage, "tool_calls"),
            ),
        ),
    ];
    if let Some(verdict) = s.get("verdict").and_then(Value::as_bool) {
        lines.push((
            "verdict",
            if verdict { "passed" } else { "failed" }.to_string(),
        ));
    }
    lines.push(("outcome", or_dash(s, "outcome")));
    lines.push(("handover", or_dash(s, "handover")));
    lines
}

fn show(ctx: &Ctx, id: &str) -> Result<(), String> {
    let session = ctx.client.get(&format!("/v1/sessions/{}", ctx::seg(id)))?;
    ctx.out(&session, |s| ctx::fields(&session_lines(s)));
    Ok(())
}

/// A session's context as `sessions context` lines: its model and window, what its last call sent, and its calls.
fn context_lines(view: &Value) -> Vec<(&'static str, String)> {
    let window = match view.get("window").and_then(Value::as_u64) {
        Some(window) => format!("{window} tokens"),
        None => "-".to_string(),
    };
    let fill = match view.get("fill").and_then(Value::as_f64) {
        Some(fill) => format!(" ({:.0}% of the window)", fill * 100.0),
        None => String::new(),
    };
    vec![
        ("session", ctx::text(view, "session")),
        ("model", or_dash(view, "model")),
        ("window", window),
        ("used", format!("{} tokens{fill}", count(view, "used"))),
        ("calls", count(view, "calls").to_string()),
        ("compactions", count(view, "compactions").to_string()),
    ]
}

fn context(ctx: &Ctx, id: &str) -> Result<(), String> {
    let view = ctx
        .client
        .get(&format!("/v1/sessions/{}/context", ctx::seg(id)))?;
    ctx.out(&view, |v| {
        ctx::fields(&context_lines(v));
        let parts: Vec<Vec<String>> = v
            .get("parts")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|p| {
                vec![
                    ctx::s(p, "part").to_string(),
                    count(p, "tokens").to_string(),
                ]
            })
            .collect();
        if !parts.is_empty() {
            println!();
            ctx::table(&["part", "tokens"], &parts);
        }
        let tools: Vec<Vec<String>> = v
            .get("tools")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|t| {
                vec![
                    ctx::s(t, "tool").to_string(),
                    count(t, "calls").to_string(),
                    count(t, "errors").to_string(),
                    count(t, "result_tokens").to_string(),
                ]
            })
            .collect();
        if !tools.is_empty() {
            println!();
            ctx::table(&["tool", "calls", "errors", "result tokens"], &tools);
        }
    });
    Ok(())
}

fn stop(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!("Stop session {id}? It hands over, and its seat goes again."),
    )? {
        return cancelled();
    }
    let session = ctx
        .client
        .post(&format!("/v1/sessions/{}/stop", ctx::seg(id)), &json!({}))?;
    ctx.out(&session, |_| {
        println!("stopped session {id}: its seat goes again")
    });
    Ok(())
}

/// A session's transcript, oldest first, a page of a thousand at a time; with `follow`, new entries as they come.
fn transcript(ctx: &Ctx, id: &str, follow: bool) -> Result<(), String> {
    const PAGE: usize = 1000;
    let mut after = 0u64;
    let rich_ctx = rich::context_for(ctx);
    loop {
        let page = items(&ctx.client.get(&format!(
            "/v1/sessions/{}/transcript?after={after}&limit={PAGE}",
            ctx::seg(id)
        ))?);
        for e in &page {
            if let Some(seq) = e.get("seq").and_then(Value::as_u64) {
                after = after.max(seq);
            }
            print_entry(ctx, &rich_ctx, e);
        }
        if page.len() >= PAGE {
            continue;
        }
        if !follow {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn print_entry(ctx: &Ctx, rich_ctx: &RichCtx, e: &Value) {
    if ctx.json {
        println!("{e}");
        return;
    }
    if let Some(lines) = engine_lines(e) {
        for line in lines {
            println!("{line}");
        }
        return;
    }
    let data: String = e
        .get("data")
        .map(|d| d.to_string())
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect();
    let data = rich::render(&data, rich_ctx, &ctx.rich);
    println!(
        "#{} work {} {:<14} {}",
        ctx::text(e, "seq"),
        ctx::text(e, "work"),
        ctx::s(e, "kind"),
        data
    );
}

/// How many lines of an agent message the transcript prints before it cuts to `│ …`.
const MESSAGE_LINES: usize = 12;

/// The lines one coding-engine entry prints, or `None` for an entry that prints as a plain line.
/// The entries are the ones a `code_engine` call produced: an `engine_update` from its ACP stream,
/// and the approvals of the tools the engine itself asked for (their call ids carry `.e`). They
/// are indented so they read as part of that call. An `allow` decision prints nothing.
fn engine_lines(e: &Value) -> Option<Vec<String>> {
    let kind = e.get("kind").and_then(Value::as_str)?;
    let data = e.get("data")?;
    let nested = data
        .get("call")
        .and_then(Value::as_str)
        .is_some_and(|call| call.contains(".e"));
    match kind {
        "engine_update" => engine_update_lines(data),
        "tool_request" if nested => {
            let name = data.get("name").and_then(Value::as_str)?;
            if !name.starts_with("engine.") {
                return None;
            }
            Some(vec![
                match data.pointer("/input/title").and_then(Value::as_str) {
                    Some(title) => format!("    ? {name} {title}"),
                    None => format!("    ? {name}"),
                },
            ])
        }
        "tool_decision" if nested => {
            let output = data.get("output").and_then(Value::as_str).unwrap_or("");
            match data.get("decision").and_then(Value::as_str)? {
                // The output already reads "waiting for the owner's approval (<id>)".
                "pending" => Some(vec![format!("    ⏸ {output}")]),
                "deny" => Some(vec![format!("    ✗ denied: {output}")]),
                _ => Some(Vec::new()),
            }
        }
        _ => None,
    }
}

/// The lines of one ACP session update the engine's stream forwarded, by its `item`.
fn engine_update_lines(data: &Value) -> Option<Vec<String>> {
    let update = data.get("update");
    let field = |key: &str| {
        update
            .and_then(|u| u.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    let lines = match data.get("item").and_then(Value::as_str)? {
        "tool_call" => {
            let (title, kind) = (field("title"), field("kind"));
            vec![if kind.is_empty() {
                format!("    ⋯ {title}")
            } else {
                format!("    ⋯ {title} [{kind}]")
            }]
        }
        "tool_status" => {
            let glyph = match field("status") {
                "completed" => "✓",
                "failed" => "✗",
                "in_progress" => "▸",
                _ => "⋯",
            };
            let label = [field("title"), field("toolCallId")]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or("");
            vec![format!("    {glyph} {label}")]
        }
        "message" => {
            let text = data.get("text").and_then(Value::as_str).unwrap_or("");
            let body: Vec<&str> = text.lines().collect();
            let mut out: Vec<String> = body
                .iter()
                .take(MESSAGE_LINES)
                .map(|line| format!("    │ {line}"))
                .collect();
            if body.len() > MESSAGE_LINES {
                out.push("    │ …".to_string());
            }
            out
        }
        "plan" => {
            let mut out = vec!["    plan:".to_string()];
            let entries = update
                .and_then(|u| u.get("entries"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            for entry in entries {
                let done = entry.get("status").and_then(Value::as_str) == Some("completed");
                let content = entry.get("content").and_then(Value::as_str).unwrap_or("");
                out.push(format!(
                    "      [{}] {content}",
                    if done { "x" } else { " " }
                ));
            }
            out
        }
        "usage" => vec![format!("    usage: {}", update.unwrap_or(&Value::Null))],
        "mode" => vec![format!("    mode: {}", field("currentModeId"))],
        "truncated" => {
            let after = data
                .get("after")
                .map_or_else(|| "?".to_string(), Value::to_string);
            vec![format!(
                "    … (engine stream truncated after {after} entries)"
            )]
        }
        _ => return None,
    };
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_asks_for_the_filters_the_flags_name() {
        assert_eq!(list_path(None, None, None, false), "/v1/sessions");
        assert_eq!(
            list_path(Some("pr3"), None, None, true),
            "/v1/sessions?project=pr3&live=true"
        );
        assert_eq!(
            list_path(None, Some("t4"), Some("paused"), false),
            "/v1/sessions?task=t4&state=paused"
        );
    }

    #[test]
    fn the_live_sessions_list_first_and_each_group_keeps_its_order() {
        let sessions = vec![
            json!({ "id": "s9", "state": "ended" }),
            json!({ "id": "s8", "state": "running" }),
            json!({ "id": "s7", "state": "failed" }),
            json!({ "id": "s6", "state": "paused" }),
        ];
        let ordered = live_first(sessions);
        let ids: Vec<&str> = ordered.iter().map(|s| ctx::s(s, "id")).collect();
        assert_eq!(ids, ["s8", "s6", "s9", "s7"]);
    }

    #[test]
    fn a_paused_session_says_why_and_a_seatless_one_shows_a_dash() {
        let paused = json!({ "state": "paused", "paused_by": "budget", "seat": null });
        assert_eq!(state_text(&paused), "paused (budget)");
        assert_eq!(
            state_text(&json!({ "state": "running", "paused_by": null })),
            "running"
        );
        assert_eq!(or_dash(&paused, "seat"), "-");
    }

    #[test]
    fn a_session_shows_its_usage_and_its_verdict_once_given() {
        let s = json!({
            "id": "s3", "state": "ended", "seat": "reviewer:1", "role": "reviewer", "task": "t4",
            "project": "pr3", "model": "anthropic/x", "node": "n1", "cid": 7, "attempt": 2,
            "verdict": true, "outcome": "passed", "handover": null,
            "usage": { "tokens_in": 1200, "tokens_out": 300, "tokens_cached": 50, "cost_micros": 420_000, "model_calls": 4, "tool_calls": 9 }
        });
        let lines = session_lines(&s);
        assert!(lines.contains(&("verdict", "passed".to_string())));
        assert!(
            lines.contains(&(
                "usage",
                "1200 in · 300 out · 50 cached tokens · $0.42 · 4 model calls · 9 tool calls"
                    .to_string()
            ))
        );
        assert!(lines.contains(&("handover", "-".to_string())));
        let running = session_lines(&json!({ "id": "s4", "state": "running", "usage": {} }));
        assert!(
            !running.iter().any(|(k, _)| *k == "verdict"),
            "no verdict before one is given"
        );
    }

    #[test]
    fn a_context_shows_its_window_and_how_full_it_is() {
        let view = json!({ "session": "s3", "model": "m", "window": 200_000, "used": 41_200, "fill": 0.206, "calls": 5, "compactions": 1 });
        let lines = context_lines(&view);
        assert!(lines.contains(&("window", "200000 tokens".to_string())));
        assert!(lines.contains(&("used", "41200 tokens (21% of the window)".to_string())));
        let no_call = context_lines(&json!({ "session": "s4", "used": 0, "parts": [] }));
        assert!(no_call.contains(&("window", "-".to_string())));
        assert!(no_call.contains(&("used", "0 tokens".to_string())));
    }
}
