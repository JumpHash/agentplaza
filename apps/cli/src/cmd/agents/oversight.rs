//! `agents permissions`: an agent's permission mode, which `set` changes.

use clap::Subcommand;
use serde_json::json;

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum PermissionAction {
    /// Set the mode: autonomous (acts on its own), ask (asks you first) or read_only.
    Set {
        /// The mode.
        #[arg(value_parser = ["autonomous", "ask", "read_only"])]
        mode: String,
    },
}

/// `agents permissions CID`: the agent's mode; `set` changes it.
pub fn permissions(ctx: &Ctx, cid: u64, action: Option<PermissionAction>) -> Result<(), String> {
    match action {
        None => {
            let v = ctx.client.get(&format!("/v1/agents/{cid}/permissions"))?;
            ctx.out(&v, |v| {
                ctx::fields(&[("agent", cid.to_string()), ("mode", ctx::text(v, "mode"))])
            });
        }
        Some(PermissionAction::Set { mode }) => {
            let v = ctx.client.put(
                &format!("/v1/agents/{cid}/permissions"),
                &json!({ "mode": mode }),
            )?;
            ctx.out(&v, |v| {
                println!("agent {cid} now runs in {} mode", ctx::text(v, "mode"))
            });
        }
    }
    Ok(())
}
