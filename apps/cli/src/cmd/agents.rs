//! `agentplaza agents`: Agents: list, inspect, create, talk to, stop, restart, set models; cancel
//! their work, retire or wipe them, put them to sleep or wake them, checkpoint, roll back and fork
//! their VMs, drive their harness and loops, set permissions, read their infractions, sanctions and
//! memory, and decide the proposals they make. The submodules hold the larger areas: `lifecycle`,
//! `harness`, `oversight` and `records`.

pub mod harness;
pub mod lifecycle;
pub mod oversight;
pub mod records;

use std::thread;
use std::time::Duration;

use apz_rich::Ctx as RichCtx;
use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm, text_arg};
use crate::rich;

use self::lifecycle::WipeWhat;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The agents, with their status, role, model and queue.
    List {
        /// Only agents of this role (its id, such as "orchestrator").
        #[arg(long)]
        role: Option<String>,
        /// Only agents in this status: working, idle, offline or asleep.
        #[arg(long)]
        status: Option<String>,
    },
    /// One agent: its model, queue, current work and record.
    Show {
        /// The agent's cid (see `agents list`).
        cid: u64,
    },
    /// Create agents of a role (the wizard's spawn).
    Create {
        /// The role id.
        #[arg(long)]
        role: String,
        /// How many to create.
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// The model to run them on, provider/model (default: the role's or the plaza's).
        #[arg(long)]
        model: Option<String>,
    },
    /// Send an agent a message (its chat).
    Tell {
        /// The agent's cid.
        cid: u64,
        /// The message, or "-" to read it from stdin.
        #[arg(long)]
        text: String,
    },
    /// Stop an agent. Asks to confirm unless --yes.
    Stop {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Restart an agent's VM. Asks to confirm unless --yes.
    Restart {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Delete an agent. Asks to confirm unless --yes.
    Delete {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Set an agent's model override: provider/model, or "default" to go back to the role's.
    Model {
        /// The agent's cid.
        cid: u64,
        /// provider/model, or "default".
        model: String,
    },
    /// An agent's transcript: what it did and said, oldest first.
    Logs {
        /// The agent's cid.
        cid: u64,
        /// Keep printing new entries as they come.
        #[arg(long)]
        follow: bool,
    },
    /// Stop the agent's running work. The agent stays on the roster.
    Cancel {
        /// The agent's cid.
        cid: u64,
    },
    /// Retire an agent for good: its VM stops and it leaves the roster. Asks to confirm unless --yes.
    Retire {
        /// The agent's cid.
        cid: u64,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Wipe an agent's workspace, and as asked its memory, transcript or identity. Asks to confirm
    /// unless --yes.
    Wipe {
        /// The agent's cid.
        cid: u64,
        /// Also clear its memory notes.
        #[arg(long)]
        memory: bool,
        /// Also clear its transcript.
        #[arg(long)]
        transcript: bool,
        /// Also forget its name, contacts and reputation.
        #[arg(long)]
        identity: bool,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Put an agent to sleep: its VM pauses with its memory kept, and wakes where it was.
    Sleep {
        /// The agent's cid.
        cid: u64,
    },
    /// Wake a sleeping agent.
    Wake {
        /// The agent's cid.
        cid: u64,
    },
    /// Checkpoint an agent's whole VM (disk and memory), to roll back to later.
    Checkpoint {
        /// The agent's cid.
        cid: u64,
        /// A label for the checkpoint, to find it by in `agents checkpoints`.
        #[arg(long)]
        label: Option<String>,
    },
    /// An agent's VM checkpoints: name, label, age, kind, memory and disk written, and pause.
    Checkpoints {
        /// The agent's cid.
        cid: u64,
    },
    /// Roll an agent's VM back to a checkpoint. A safety checkpoint of its current state is taken
    /// first. Asks to confirm unless --yes.
    Rollback {
        /// The agent's cid.
        cid: u64,
        /// The checkpoint's name (see `agents checkpoints`).
        checkpoint: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Delete a VM checkpoint. Asks to confirm unless --yes.
    CheckpointDelete {
        /// The agent's cid.
        cid: u64,
        /// The checkpoint's name (see `agents checkpoints`).
        checkpoint: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Fork an agent into copies: a checkpoint of its VM is taken, and each copy starts from it.
    Fork {
        /// The agent's cid.
        cid: u64,
        /// How many copies, 1 to 8.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=8))]
        n: u64,
    },
    /// Move an agent's VM to another node. The hub copies it over and reports the bytes moved and the time taken.
    Move {
        /// The agent's cid.
        cid: u64,
        /// The node to move it to, by its id (see `nodes list`).
        node: String,
    },
    /// An agent's harness limits. Add `set`, `compress`, `extend` or `steer` to change them or its
    /// running work.
    Harness {
        /// The agent's cid.
        cid: u64,
        #[command(subcommand)]
        action: Option<harness::Action>,
    },
    /// The loops (prompts run on an agent every so many minutes). Add `start` or `stop` to change them.
    Loops {
        /// Only the loops of this agent.
        #[arg(long)]
        cid: Option<u64>,
        #[command(subcommand)]
        action: Option<harness::LoopAction>,
    },
    /// An agent's permission mode. Add `set` to change it.
    Permissions {
        /// The agent's cid.
        cid: u64,
        #[command(subcommand)]
        action: Option<oversight::PermissionAction>,
    },
    /// The laws an agent has broken, newest first.
    Infractions {
        /// The agent's cid.
        cid: u64,
    },
    /// The sanctions on an agent, and the suspensions they set.
    Sanctions {
        /// The agent's cid.
        cid: u64,
    },
    /// Lift an active suspension, by its sanction id (see `agents sanctions`). The reason is required.
    Lift {
        /// The sanction's id.
        id: u64,
        /// Why it is lifted; kept on the record.
        #[arg(long)]
        reason: String,
    },
    /// An agent's memory notes. `--search` narrows them; add `show SLUG` to read one.
    Memory {
        /// The agent's cid.
        cid: u64,
        /// Only the notes whose slug, title, kind or tag contains this (ignoring case).
        #[arg(long)]
        search: Option<String>,
        #[command(subcommand)]
        action: Option<records::MemoryAction>,
    },
    /// The agent proposals waiting for you. Add `approve` or `reject` to decide one.
    Proposals {
        #[command(subcommand)]
        action: Option<records::ProposalAction>,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Cancel { cid } => lifecycle::cancel(ctx, cid),
        Cmd::Retire { cid, yes } => lifecycle::retire(ctx, cid, yes),
        Cmd::Wipe {
            cid,
            memory,
            transcript,
            identity,
            yes,
        } => lifecycle::wipe(
            ctx,
            cid,
            WipeWhat {
                memory,
                transcript,
                identity,
            },
            yes,
        ),
        Cmd::Sleep { cid } => lifecycle::sleep(ctx, cid),
        Cmd::Wake { cid } => lifecycle::wake(ctx, cid),
        Cmd::Checkpoint { cid, label } => lifecycle::checkpoint(ctx, cid, label.as_deref()),
        Cmd::Checkpoints { cid } => lifecycle::checkpoints(ctx, cid),
        Cmd::Rollback {
            cid,
            checkpoint,
            yes,
        } => lifecycle::rollback(ctx, cid, &checkpoint, yes),
        Cmd::CheckpointDelete {
            cid,
            checkpoint,
            yes,
        } => lifecycle::checkpoint_delete(ctx, cid, &checkpoint, yes),
        Cmd::Fork { cid, n } => lifecycle::fork(ctx, cid, n),
        Cmd::Move { cid, node } => lifecycle::move_to(ctx, cid, &node),
        Cmd::Harness { cid, action } => harness::harness(ctx, cid, action),
        Cmd::Loops { cid, action } => harness::loops(ctx, cid, action),
        Cmd::Permissions { cid, action } => oversight::permissions(ctx, cid, action),
        Cmd::Infractions { cid } => oversight::infractions(ctx, cid),
        Cmd::Sanctions { cid } => oversight::sanctions(ctx, cid),
        Cmd::Lift { id, reason } => oversight::lift(ctx, id, &reason),
        Cmd::Memory {
            cid,
            search,
            action,
        } => records::memory(ctx, cid, search.as_deref(), action),
        Cmd::Proposals { action } => records::proposals(ctx, action),
        Cmd::List { role, status } => list(ctx, role.as_deref(), status.as_deref()),
        Cmd::Show { cid } => show(ctx, cid),
        Cmd::Create { role, count, model } => create(ctx, &role, count, model.as_deref()),
        Cmd::Tell { cid, text } => tell(ctx, cid, &text),
        Cmd::Stop { cid, yes } => {
            if !confirm(yes, &format!("Stop agent {cid}?"))? {
                return cancelled();
            }
            let v = ctx
                .client
                .post(&format!("/v1/agents/{cid}/stop"), &json!({}))?;
            ctx.out(&v, |_| println!("stopped agent {cid}"));
            Ok(())
        }
        Cmd::Restart { cid, yes } => {
            if !confirm(yes, &format!("Restart agent {cid}? Its VM restarts."))? {
                return cancelled();
            }
            let v = ctx
                .client
                .post(&format!("/v1/agents/{cid}/restart"), &json!({}))?;
            ctx.out(&v, |_| println!("restarting agent {cid}"));
            Ok(())
        }
        Cmd::Delete { cid, yes } => {
            if !confirm(yes, &format!("Delete agent {cid}?"))? {
                return cancelled();
            }
            let v = ctx.client.delete(&format!("/v1/agents/{cid}"))?;
            ctx.out(&v, |_| println!("deleted agent {cid}"));
            Ok(())
        }
        Cmd::Model { cid, model } => {
            let value = if model == "default" {
                Value::Null
            } else {
                Value::String(model.clone())
            };
            let v = ctx
                .client
                .patch(&format!("/v1/agents/{cid}"), &json!({ "model": value }))?;
            ctx.out(&v, |_| {
                if model == "default" {
                    println!("agent {cid} uses its default model again");
                } else {
                    println!("agent {cid} now runs {model}");
                }
            });
            Ok(())
        }
        Cmd::Logs { cid, follow } => logs(ctx, cid, follow),
    }
}

fn cancelled() -> Result<(), String> {
    println!("cancelled");
    Ok(())
}

/// The items of a hub array (none when the value is not one).
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// A reason, trimmed. Blank is refused: a lift or a rejection needs one on the record.
fn required_reason(reason: &str) -> Result<String, String> {
    let reason = reason.trim();
    if reason.is_empty() {
        Err("a reason is required: say why".into())
    } else {
        Ok(reason.to_string())
    }
}

fn name_of(a: &Value) -> String {
    let name = ctx::s(a, "name");
    if name.is_empty() {
        format!("agent {}", ctx::text(a, "cid"))
    } else {
        name.to_string()
    }
}

fn work_of(a: &Value) -> String {
    a.get("work")
        .filter(|w| !w.is_null())
        .map(|w| ctx::text(w, "prompt"))
        .unwrap_or_else(|| "-".to_string())
}

fn list(ctx: &Ctx, role: Option<&str>, status: Option<&str>) -> Result<(), String> {
    let all = ctx.client.get("/v1/agents")?;
    let agents: Vec<Value> = all
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| role.map_or(true, |r| ctx::s(a, "role") == r))
        .filter(|a| status.map_or(true, |s| ctx::s(a, "status") == s))
        .collect();
    ctx.out(&Value::Array(agents.clone()), |_| {
        let rows: Vec<Vec<String>> = agents
            .iter()
            .map(|a| {
                vec![
                    ctx::text(a, "cid"),
                    name_of(a),
                    ctx::s(a, "role").to_string(),
                    ctx::s(a, "status").to_string(),
                    ctx::s(a, "model").to_string(),
                    ctx::text(a, "queue"),
                    work_of(a),
                ]
            })
            .collect();
        ctx::table(
            &["cid", "name", "role", "status", "model", "queue", "work"],
            &rows,
        );
    });
    Ok(())
}

fn show(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let agent = ctx.client.get(&format!("/v1/agents/{cid}"))?;
    let profile = ctx.client.get(&format!("/v1/agents/{cid}/profile"))?;
    let both = json!({ "agent": agent, "profile": profile });
    ctx.out(&both, |b| {
        let (a, p) = (&b["agent"], &b["profile"]);
        let tasks = &p["tasks"];
        let fines = &p["fines"];
        ctx::fields(&[
            (
                "agent",
                format!("{} (cid {})", name_of(a), ctx::text(a, "cid")),
            ),
            (
                "role",
                format!("{} ({})", ctx::s(a, "role_label"), ctx::s(a, "role")),
            ),
            (
                "status",
                format!("{} · {}", ctx::s(a, "status"), ctx::s(a, "tier")),
            ),
            (
                "model",
                format!(
                    "{} (from {})",
                    ctx::s(a, "model"),
                    ctx::s(a, "model_source")
                ),
            ),
            ("queue", ctx::text(a, "queue")),
            ("work", work_of(a)),
            ("wallet", ctx::text(p, "wallet")),
            ("earned", ctx::text(p, "earned")),
            ("spent", ctx::text(p, "spent")),
            (
                "tasks",
                format!(
                    "{} posted, {} done, {} verified, {} failed",
                    ctx::text(tasks, "posted"),
                    ctx::text(tasks, "done"),
                    ctx::text(tasks, "verified"),
                    ctx::text(tasks, "failed"),
                ),
            ),
            ("reputation", ctx::text(p, "reputation")),
            ("conduct", format!("{}/100", ctx::text(p, "conduct"))),
            (
                "fines",
                format!(
                    "{} ({} total)",
                    ctx::text(fines, "count"),
                    ctx::text(fines, "total")
                ),
            ),
            ("denials", ctx::text(p, "denials")),
            (
                "memory",
                format!("{} note(s)", ctx::text(p, "memory_notes")),
            ),
        ]);
    });
    Ok(())
}

fn create(ctx: &Ctx, role: &str, count: u32, model: Option<&str>) -> Result<(), String> {
    if count == 0 {
        return Err("--count is at least 1".into());
    }
    let mut body = json!({ "role": role, "count": count });
    if let Some(m) = model {
        body["model"] = json!(m);
    }
    let v = ctx.client.post("/v1/agents", &body)?;
    ctx.out(&v, |_| {
        println!(
            "created {count} {role} agent{}",
            if count == 1 { "" } else { "s" }
        )
    });
    Ok(())
}

fn tell(ctx: &Ctx, cid: u64, text: &str) -> Result<(), String> {
    let text = text_arg(text)?;
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))?;
    ctx.out(&v, |v| {
        println!("sent to agent {cid} (work {})", ctx::text(v, "work"))
    });
    Ok(())
}

fn logs(ctx: &Ctx, cid: u64, follow: bool) -> Result<(), String> {
    const PAGE: usize = 1000;
    let mut after = 0u64;
    let rich_ctx = rich::context_for(ctx);
    loop {
        let page = ctx.client.get(&format!(
            "/v1/agents/{cid}/transcript?after={after}&limit={PAGE}"
        ))?;
        let entries = page.as_array().cloned().unwrap_or_default();
        for e in &entries {
            if let Some(seq) = e.get("seq").and_then(Value::as_u64) {
                after = after.max(seq);
            }
            print_entry(ctx, &rich_ctx, e);
        }
        if entries.len() >= PAGE {
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

    fn entry(kind: &str, data: Value) -> Value {
        json!({ "seq": 1, "at_ms": 0, "cid": 3, "work": 9, "kind": kind, "data": data })
    }

    fn update(item: &str, update: Value) -> Value {
        entry(
            "engine_update",
            json!({ "call": "c4", "item": item, "update": update }),
        )
    }

    fn printed(e: Value) -> Vec<String> {
        engine_lines(&e).expect("an engine entry prints its own lines")
    }

    #[test]
    fn tool_calls_name_their_kind() {
        let with_kind = update(
            "tool_call",
            json!({ "title": "Read src/lib.rs", "kind": "read" }),
        );
        assert_eq!(printed(with_kind), ["    ⋯ Read src/lib.rs [read]"]);
        let bare = update("tool_call", json!({ "title": "Think" }));
        assert_eq!(printed(bare), ["    ⋯ Think"]);
    }

    #[test]
    fn tool_status_shows_a_glyph_and_falls_back_to_the_call_id() {
        let status = |s: &str| {
            printed(update(
                "tool_status",
                json!({ "toolCallId": "t7", "title": "edit", "status": s }),
            ))
        };
        assert_eq!(status("completed"), ["    ✓ edit"]);
        assert_eq!(status("failed"), ["    ✗ edit"]);
        assert_eq!(status("in_progress"), ["    ▸ edit"]);
        assert_eq!(status("pending"), ["    ⋯ edit"]);
        let untitled = update(
            "tool_status",
            json!({ "toolCallId": "t7", "status": "completed" }),
        );
        assert_eq!(printed(untitled), ["    ✓ t7"]);
    }

    #[test]
    fn messages_are_quoted_and_cut_after_twelve_lines() {
        let short = entry(
            "engine_update",
            json!({ "call": "c4", "item": "message", "text": "one\ntwo" }),
        );
        assert_eq!(printed(short), ["    │ one", "    │ two"]);

        let text: String = (1..=14).map(|n| format!("line {n}\n")).collect();
        let long = entry(
            "engine_update",
            json!({ "call": "c4", "item": "message", "text": text }),
        );
        let lines = printed(long);
        assert_eq!(lines.len(), 13, "{lines:?}");
        assert_eq!(lines[0], "    │ line 1");
        assert_eq!(lines[11], "    │ line 12");
        assert_eq!(lines[12], "    │ …");
    }

    #[test]
    fn plans_tick_the_finished_steps() {
        let plan = update(
            "plan",
            json!({ "entries": [
                { "content": "read the spec", "status": "completed" },
                { "content": "write the code", "status": "in_progress" },
            ] }),
        );
        assert_eq!(
            printed(plan),
            [
                "    plan:",
                "      [x] read the spec",
                "      [ ] write the code"
            ]
        );
    }

    #[test]
    fn usage_mode_and_truncation_print_on_one_line() {
        assert_eq!(
            printed(update("usage", json!({ "used": 5 }))),
            ["    usage: {\"used\":5}"]
        );
        assert_eq!(
            printed(update("mode", json!({ "currentModeId": "code" }))),
            ["    mode: code"]
        );
        let cut = entry(
            "engine_update",
            json!({ "call": "c4", "item": "truncated", "after": 400 }),
        );
        assert_eq!(
            printed(cut),
            ["    … (engine stream truncated after 400 entries)"]
        );
    }

    #[test]
    fn engine_requests_and_their_approvals_are_nested_under_the_call() {
        let request = entry(
            "tool_request",
            json!({ "call": "c4.e2", "name": "engine.write", "input": { "title": "write notes.md" } }),
        );
        assert_eq!(printed(request), ["    ? engine.write write notes.md"]);

        let pending = entry(
            "tool_decision",
            json!({ "call": "c4.e2", "decision": "pending", "output": "waiting for the owner's approval (ap-7)" }),
        );
        assert_eq!(
            printed(pending),
            ["    ⏸ waiting for the owner's approval (ap-7)"]
        );

        let allowed = entry(
            "tool_decision",
            json!({ "call": "c4.e2", "decision": "allow", "output": "ok" }),
        );
        assert!(printed(allowed).is_empty());

        let denied = entry(
            "tool_decision",
            json!({ "call": "c4.e2", "decision": "deny", "output": "no" }),
        );
        assert_eq!(printed(denied), ["    ✗ denied: no"]);
    }

    #[test]
    fn other_entries_keep_the_plain_line() {
        // A request from the agent's own tool, not from the engine.
        let own = entry(
            "tool_request",
            json!({ "call": "c4", "name": "plaza_tasks", "input": {} }),
        );
        assert!(engine_lines(&own).is_none());
        let other_tool = entry(
            "tool_request",
            json!({ "call": "c4.e2", "name": "read", "input": {} }),
        );
        assert!(engine_lines(&other_tool).is_none());
        let message = entry("message", json!({ "text": "hello" }));
        assert!(engine_lines(&message).is_none());
        let unknown = update("something_new", json!({}));
        assert!(engine_lines(&unknown).is_none());
    }
}
