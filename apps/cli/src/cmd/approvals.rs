//! `agentplaza approvals`: Approvals waiting for the owner.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

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
    /// Approve a request, optionally with a reason.
    Approve {
        /// The approval's id.
        id: String,
        /// Why (optional).
        #[arg(long)]
        reason: Option<String>,
    },
    /// Deny a request, optionally with a reason.
    Deny {
        /// The approval's id.
        id: String,
        /// Why (optional).
        #[arg(long)]
        reason: Option<String>,
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

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { pending } => list(ctx, pending),
        Cmd::Show { id } => show(ctx, &id),
        Cmd::Approve { id, reason } => decide(ctx, &id, true, reason),
        Cmd::Deny { id, reason } => decide(ctx, &id, false, reason),
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
    let shown: Vec<&Value> = array(&value).iter().filter(|a| !pending || ctx::s(a, "state") == "pending").collect();
    ctx.out(&value, |_| {
        let rows: Vec<Vec<String>> = shown
            .iter()
            .map(|a| vec![ctx::text(a, "id"), ctx::text(a, "cid"), ctx::text(a, "tool"), ctx::text(a, "state"), ctx::text(a, "reason")])
            .collect();
        ctx::table(&["id", "agent", "tool", "state", "reason"], &rows);
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str) -> Result<(), String> {
    let value = ctx.client.get("/v1/approvals")?;
    let approval = array(&value).iter().find(|a| ctx::s(a, "id") == id).ok_or_else(|| format!("no approval {id}"))?;
    ctx.out(approval, |a| {
        ctx::fields(&[
            ("id", ctx::text(a, "id")),
            ("agent", ctx::text(a, "cid")),
            ("tool", ctx::text(a, "tool")),
            ("state", ctx::text(a, "state")),
            ("created", ctx::when(&a["created_ms"])),
            ("reason", ctx::text(a, "reason")),
            ("input", serde_json::to_string_pretty(a.get("input").unwrap_or(&Value::Null)).unwrap_or_default()),
        ]);
    });
    Ok(())
}

fn decide(ctx: &Ctx, id: &str, approve: bool, reason: Option<String>) -> Result<(), String> {
    let mut body = json!({ "approve": approve });
    if let Some(reason) = reason.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()) {
        body["reason"] = json!(reason);
    }
    let value = ctx.client.post(&format!("/v1/approvals/{}", ctx::seg(id)), &body)?;
    ctx.out(&value, |_| println!("{} approval {id}", if approve { "approved" } else { "denied" }));
    Ok(())
}

/// The role's title, or its id when it has none.
fn role_name(p: &Value) -> String {
    let role = p.get("role").unwrap_or(&Value::Null);
    let title = ctx::s(role, "title");
    if title.is_empty() { ctx::s(role, "id").to_string() } else { title.to_string() }
}

fn proposals(ctx: &Ctx) -> Result<(), String> {
    let value = ctx.client.get("/v1/agent-proposals")?;
    ctx.out(&value, |v| {
        let rows: Vec<Vec<String>> = array(v)
            .iter()
            .map(|p| vec![ctx::text(p, "id"), role_name(p), ctx::text(p, "count"), ctx::text(p, "model"), ctx::text(p, "state"), ctx::text(p, "by")])
            .collect();
        ctx::table(&["id", "role", "count", "model", "state", "by"], &rows);
    });
    Ok(())
}

fn approve_proposal(ctx: &Ctx, id: &str, count: Option<u32>, model: Option<String>) -> Result<(), String> {
    let mut body = json!({});
    if let Some(count) = count {
        body["count"] = json!(count);
    }
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        body["model"] = json!(model.trim());
    }
    let value = ctx.client.post(&format!("/v1/agent-proposals/{}/approve", ctx::seg(id)), &body)?;
    ctx.out(&value, |_| println!("approved proposal {id}"));
    Ok(())
}

fn reject_proposal(ctx: &Ctx, id: &str, reason: String) -> Result<(), String> {
    let reason = reason.trim().to_string();
    if reason.is_empty() {
        return Err("a rejection needs a reason".into());
    }
    let value = ctx.client.post(&format!("/v1/agent-proposals/{}/reject", ctx::seg(id)), &json!({ "reason": reason }))?;
    ctx.out(&value, |_| println!("rejected proposal {id}"));
    Ok(())
}
