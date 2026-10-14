//! `agentplaza approvals`: Approvals waiting for the owner.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};
use crate::rich;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the approvals (`--pending`: only those waiting for you).
    List {
        /// Only the pending ones.
        #[arg(long)]
        pending: bool,
    },
    /// Show one approval, with the tool input it asks about.
    Show {
        /// The approval's id.
        id: String,
    },
    /// Approve a request, optionally with a reason. A sanction escalation takes `--action acknowledge`.
    Approve {
        /// The approval's id.
        id: String,
        /// Why (optional).
        #[arg(long)]
        reason: Option<String>,
        /// For a sanction escalation: `acknowledge` (the default); `lift` and `retire` are denials, so the hub refuses them here.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SANCTION_ACTIONS))]
        action: Option<String>,
    },
    /// Deny a request, optionally with a reason. A sanction escalation takes `--action lift` or `retire`.
    Deny {
        /// The approval's id.
        id: String,
        /// Why (optional).
        #[arg(long)]
        reason: Option<String>,
        /// For a sanction escalation: `lift` lifts an active suspension, `retire` records a retirement (apid does
        /// not retire agents). A bare deny lifts an active suspension and does nothing to any other sanction.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SANCTION_ACTIONS))]
        action: Option<String>,
    },
    /// List the agent proposals waiting for you.
    Proposals,
    /// Approve an agent proposal as it was proposed. Role edits are in the web console.
    ApproveProposal {
        /// The proposal's id.
        id: String,
        /// How many agents to spawn.
        #[arg(long)]
        count: Option<u32>,
        /// The model to run them on.
        #[arg(long)]
        model: Option<String>,
    },
    /// Reject an agent proposal. The reason is required.
    RejectProposal {
        /// The proposal's id.
        id: String,
        /// Why it is rejected.
        #[arg(long)]
        reason: String,
    },
}

/// The decisions a sanction escalation takes (the hub's `sanctions::settle`).
const SANCTION_ACTIONS: [&str; 3] = ["acknowledge", "lift", "retire"];

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { pending } => list(ctx, pending),
        Cmd::Show { id } => show(ctx, &id),
        Cmd::Approve { id, reason, action } => decide(ctx, &id, true, reason, action),
        Cmd::Deny { id, reason, action } => decide(ctx, &id, false, reason, action),
        Cmd::Proposals => proposals(ctx),
        Cmd::ApproveProposal { id, count, model } => approve_proposal(ctx, &id, count, model),
        Cmd::RejectProposal { id, reason } => reject_proposal(ctx, &id, reason),
    }
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn list(ctx: &Ctx, pending: bool) -> Result<(), String> {
    let value = ctx.client.get("/v1/approvals")?;
    let shown: Vec<Value> = array(&value)
        .iter()
        .filter(|a| !pending || ctx::s(a, "state") == "pending")
        .cloned()
        .collect();
    let shown = Value::Array(shown);
    ctx.out(&shown, |shown| {
        let rows: Vec<Vec<String>> = array(shown)
            .iter()
            .map(|a| {
                vec![
                    ctx::text(a, "id"),
                    ctx::text(a, "cid"),
                    ctx::text(a, "tool"),
                    ctx::text(a, "state"),
                    ctx::text(a, "reason"),
                ]
            })
            .collect();
        ctx::table(&["id", "agent", "tool", "state", "reason"], &rows);
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str) -> Result<(), String> {
    let value = ctx.client.get("/v1/approvals")?;
    let approval = array(&value)
        .iter()
        .find(|a| ctx::s(a, "id") == id)
        .ok_or_else(|| format!("no approval {id}"))?;
    let rich_ctx = rich::context_for(ctx);
    ctx.out(approval, |a| {
        ctx::fields(&[
            ("id", ctx::text(a, "id")),
            ("agent", ctx::text(a, "cid")),
            ("tool", ctx::text(a, "tool")),
            ("state", ctx::text(a, "state")),
            ("created", ctx::when(&a["created_ms"])),
            (
                "reason",
                rich::render(&ctx::text(a, "reason"), &rich_ctx, &ctx.rich),
            ),
            (
                "input",
                serde_json::to_string_pretty(a.get("input").unwrap_or(&Value::Null))
                    .unwrap_or_default(),
            ),
        ]);
    });
    Ok(())
}

fn decide(
    ctx: &Ctx,
    id: &str,
    approve: bool,
    reason: Option<String>,
    action: Option<String>,
) -> Result<(), String> {
    let mut body = json!({ "approve": approve });
    if let Some(reason) = reason
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
    {
        body["reason"] = json!(reason);
    }
    if let Some(action) = action {
        body["action"] = json!(action);
    }
    let value = ctx
        .client
        .post(&format!("/v1/approvals/{}", ctx::seg(id)), &body)?;
    ctx.out(&value, |_| {
        println!(
            "{} approval {id}",
            if approve { "approved" } else { "denied" }
        )
    });
    // A bare deny lifts an active suspension and does nothing to any other sanction.
    if !approve && body.get("action").is_none() && is_sanction(ctx, id) {
        eprintln!(
            "note: approval {id} is a sanction escalation. A bare deny lifts it only when it is an active suspension; \
             on any other sanction it has no effect. Use --action lift for a suspension, or --action retire to record a retirement."
        );
    }
    Ok(())
}

/// Whether the approval `id` is a sanction escalation (best effort: false when the queue cannot be read).
fn is_sanction(ctx: &Ctx, id: &str) -> bool {
    ctx.client.get("/v1/approvals").ok().is_some_and(|value| {
        array(&value)
            .iter()
            .any(|a| ctx::s(a, "id") == id && ctx::s(a, "kind") == "sanction")
    })
}

/// The role's title, or its id when it has none.
fn role_name(p: &Value) -> String {
    let role = p.get("role").unwrap_or(&Value::Null);
    let title = ctx::s(role, "title");
    if title.is_empty() {
        ctx::s(role, "id").to_string()
    } else {
        title.to_string()
    }
}

fn proposals(ctx: &Ctx) -> Result<(), String> {
    let value = ctx.client.get("/v1/agent-proposals")?;
    ctx.out(&value, |v| {
        let rows: Vec<Vec<String>> = array(v)
            .iter()
            .map(|p| {
                vec![
                    ctx::text(p, "id"),
                    role_name(p),
                    ctx::text(p, "count"),
                    ctx::text(p, "model"),
                    ctx::text(p, "state"),
                    ctx::text(p, "by"),
                ]
            })
            .collect();
        ctx::table(&["id", "role", "count", "model", "state", "by"], &rows);
    });
    Ok(())
}

fn approve_proposal(
    ctx: &Ctx,
    id: &str,
    count: Option<u32>,
    model: Option<String>,
) -> Result<(), String> {
    let mut body = json!({});
    if let Some(count) = count {
        body["count"] = json!(count);
    }
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        body["model"] = json!(model.trim());
    }
    let value = ctx.client.post(
        &format!("/v1/agent-proposals/{}/approve", ctx::seg(id)),
        &body,
    )?;
    ctx.out(&value, |_| println!("approved proposal {id}"));
    Ok(())
}

fn reject_proposal(ctx: &Ctx, id: &str, reason: String) -> Result<(), String> {
    let reason = reason.trim().to_string();
    if reason.is_empty() {
        return Err("a rejection needs a reason".into());
    }
    let value = ctx.client.post(
        &format!("/v1/agent-proposals/{}/reject", ctx::seg(id)),
        &json!({ "reason": reason }),
    )?;
    ctx.out(&value, |_| println!("rejected proposal {id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Just the `approvals` command, so the flags parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[test]
    fn a_sanction_action_is_one_of_the_three_decisions() {
        for action in SANCTION_ACTIONS {
            for verb in ["approve", "deny"] {
                assert!(
                    Probe::try_parse_from(["approvals", verb, "4", "--action", action]).is_ok(),
                    "{verb} {action}"
                );
            }
        }
        assert!(Probe::try_parse_from(["approvals", "deny", "4", "--action", "fire"]).is_err());
    }
}
