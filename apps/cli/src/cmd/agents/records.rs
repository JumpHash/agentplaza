//! `agents proposals`: the agents an orchestrator proposes to spawn (the owner decides them).

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum ProposalAction {
    /// Approve a proposal as it was proposed.
    Approve {
        /// The proposal's id.
        id: String,
    },
    /// Reject a proposal. The reason is required.
    Reject {
        /// The proposal's id.
        id: String,
        /// Why it is rejected.
        #[arg(long)]
        reason: String,
    },
}

/// `agents proposals`: the proposals waiting for the owner; `approve` or `reject` one.
pub fn proposals(ctx: &Ctx, action: Option<ProposalAction>) -> Result<(), String> {
    match action {
        None => {
            let v = ctx.client.get("/v1/agent-proposals")?;
            let pending: Vec<Value> = super::items(&v)
                .iter()
                .filter(|p| ctx::s(p, "state") == "pending")
                .cloned()
                .collect();
            ctx.out(&Value::Array(pending.clone()), |_| {
                let rows: Vec<Vec<String>> = pending
                    .iter()
                    .map(|p| {
                        vec![
                            ctx::text(p, "id"),
                            role_name(p),
                            ctx::text(p, "count"),
                            ctx::text(p, "model"),
                            ctx::text(p, "by"),
                            ctx::when(&p["created_ms"]),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "role", "count", "model", "by", "created"], &rows);
            });
            Ok(())
        }
        Some(ProposalAction::Approve { id }) => {
            let v = ctx.client.post(
                &format!("/v1/agent-proposals/{}/approve", ctx::seg(&id)),
                &json!({}),
            )?;
            ctx.out(&v, |_| println!("approved proposal {id}"));
            Ok(())
        }
        Some(ProposalAction::Reject { id, reason }) => {
            let reason = super::required_reason(&reason)?;
            let v = ctx.client.post(
                &format!("/v1/agent-proposals/{}/reject", ctx::seg(&id)),
                &json!({ "reason": reason }),
            )?;
            ctx.out(&v, |_| println!("rejected proposal {id}"));
            Ok(())
        }
    }
}

/// The role's title, or its id when it has none.
fn role_name(p: &Value) -> String {
    let role = &p["role"];
    let title = ctx::s(role, "title");
    if title.is_empty() {
        ctx::s(role, "id").to_string()
    } else {
        title.to_string()
    }
}
