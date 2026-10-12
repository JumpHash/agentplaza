//! `agentplaza agents`: Agents: list, inspect, create, talk to, stop, restart, set models.

use std::io::{IsTerminal, Read, Write};
use std::thread;
use std::time::Duration;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The agents, with their status, role, model and queue.
    List {
        /// Only agents of this role (its id, such as "orchestrator").
        #[arg(long)]
        role: Option<String>,
        /// Only agents in this status: working, idle or offline.
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
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { role, status } => list(ctx, role.as_deref(), status.as_deref()),
        Cmd::Show { cid } => show(ctx, cid),
        Cmd::Create { role, count, model } => create(ctx, &role, count, model.as_deref()),
        Cmd::Tell { cid, text } => tell(ctx, cid, &text),
        Cmd::Stop { cid, yes } => {
            if !confirm(yes, &format!("Stop agent {cid}?"))? {
                return cancelled();
            }
            let v = ctx.client.post(&format!("/v1/agents/{cid}/stop"), &json!({}))?;
            ctx.out(&v, |_| println!("stopped agent {cid}"));
            Ok(())
        }
        Cmd::Restart { cid, yes } => {
            if !confirm(yes, &format!("Restart agent {cid}? Its VM restarts."))? {
                return cancelled();
            }
            let v = ctx.client.post(&format!("/v1/agents/{cid}/restart"), &json!({}))?;
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
            let value = if model == "default" { Value::Null } else { Value::String(model.clone()) };
            let v = ctx.client.patch(&format!("/v1/agents/{cid}"), &json!({ "model": value }))?;
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

/// Whether to go ahead with a destructive action: `--yes` says so; otherwise the owner answers
/// `Proceed? [y/N]` on a terminal. Off a terminal the action is refused, never assumed.
pub fn confirm(yes: bool, question: &str) -> Result<bool, String> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!("{question} Not on a terminal: pass --yes to go ahead"));
    }
    print!("{question} Proceed? [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

/// A text flag's value, or stdin for "-". Empty text is refused.
pub fn text_arg(text: &str) -> Result<String, String> {
    let body = if text == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(|e| format!("reading stdin: {e}"))?;
        buf
    } else {
        text.to_string()
    };
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err("the text is empty".into());
    }
    Ok(trimmed.to_string())
}

fn cancelled() -> Result<(), String> {
    println!("cancelled");
    Ok(())
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
        ctx::table(&["cid", "name", "role", "status", "model", "queue", "work"], &rows);
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
            ("agent", format!("{} (cid {})", name_of(a), ctx::text(a, "cid"))),
            ("role", format!("{} ({})", ctx::s(a, "role_label"), ctx::s(a, "role"))),
            ("status", format!("{} · {}", ctx::s(a, "status"), ctx::s(a, "tier"))),
            ("model", format!("{} (from {})", ctx::s(a, "model"), ctx::s(a, "model_source"))),
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
            ("fines", format!("{} ({} total)", ctx::text(fines, "count"), ctx::text(fines, "total"))),
            ("denials", ctx::text(p, "denials")),
            ("memory", format!("{} note(s)", ctx::text(p, "memory_notes"))),
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
    ctx.out(&v, |_| println!("created {count} {role} agent{}", if count == 1 { "" } else { "s" }));
    Ok(())
}

fn tell(ctx: &Ctx, cid: u64, text: &str) -> Result<(), String> {
    let text = text_arg(text)?;
    let v = ctx.client.post(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))?;
    ctx.out(&v, |v| println!("sent to agent {cid} (work {})", ctx::text(v, "work")));
    Ok(())
}

fn logs(ctx: &Ctx, cid: u64, follow: bool) -> Result<(), String> {
    const PAGE: usize = 1000;
    let mut after = 0u64;
    loop {
        let page = ctx.client.get(&format!("/v1/agents/{cid}/transcript?after={after}&limit={PAGE}"))?;
        let entries = page.as_array().cloned().unwrap_or_default();
        for e in &entries {
            if let Some(seq) = e.get("seq").and_then(Value::as_u64) {
                after = after.max(seq);
            }
            print_entry(ctx, e);
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

fn print_entry(ctx: &Ctx, e: &Value) {
    if ctx.json {
        println!("{e}");
        return;
    }
    let data: String = e.get("data").map(|d| d.to_string()).unwrap_or_default().chars().take(160).collect();
    println!("#{} work {} {:<14} {}", ctx::text(e, "seq"), ctx::text(e, "work"), ctx::s(e, "kind"), data);
}
