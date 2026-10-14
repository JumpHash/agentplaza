//! `agents permissions`, `infractions`, `sanctions` and `lift`: how an agent is held to the laws.

use clap::Subcommand;
use serde_json::{Value, json};

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

/// The laws an agent has broken, newest first.
pub fn infractions(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let v = ctx.client.get(&format!("/v1/agents/{cid}/infractions"))?;
    ctx.out(&v, |v| {
        let rows: Vec<Vec<String>> = super::items(v)
            .iter()
            .map(|i| {
                vec![
                    ctx::when(&i["at_ms"]),
                    ctx::text(i, "law"),
                    ctx::text(i, "tool"),
                    ctx::text(i, "detail"),
                ]
            })
            .collect();
        ctx::table(&["when", "law", "tool", "detail"], &rows);
    });
    Ok(())
}

/// The sanctions on an agent, and the suspensions they set.
pub fn sanctions(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let v = ctx.client.get(&format!("/v1/sanctions?cid={cid}"))?;
    ctx.out(&v, |v| {
        let rows: Vec<Vec<String>> = super::items(&v["sanctions"])
            .iter()
            .map(|s| {
                vec![
                    ctx::text(s, "id"),
                    ctx::text(s, "law"),
                    ctx::text(s, "sanction"),
                    ctx::text(s, "outcome"),
                    ctx::text(s, "fine"),
                    lifted_text(s),
                ]
            })
            .collect();
        println!("sanctions");
        ctx::table(
            &["id", "law", "sanction", "outcome", "fine", "lifted"],
            &rows,
        );
        let suspensions: Vec<Vec<String>> = super::items(&v["suspensions"])
            .iter()
            .map(|s| {
                vec![
                    ctx::text(s, "id"),
                    ctx::text(s, "tool"),
                    ctx::text(s, "law"),
                    ctx::when(&s["until_ms"]),
                    ctx::text(s, "active"),
                ]
            })
            .collect();
        println!();
        println!("suspensions");
        ctx::table(&["id", "tool", "law", "until", "active"], &suspensions);
    });
    Ok(())
}

/// Lifts an active suspension. The reason is required and goes on the record.
pub fn lift(ctx: &Ctx, id: u64, reason: &str) -> Result<(), String> {
    let reason = super::required_reason(reason)?;
    let v = ctx.client.post(
        &format!("/v1/sanctions/{id}/lift"),
        &json!({ "reason": reason }),
    )?;
    ctx.out(&v, |_| println!("lifted sanction {id}"));
    Ok(())
}

/// "when, why" for a lifted sanction; "-" while it stands.
fn lifted_text(s: &Value) -> String {
    let lifted = &s["lifted"];
    if lifted.is_null() {
        "-".to_string()
    } else {
        format!(
            "{}, {}",
            ctx::when(&lifted["at_ms"]),
            ctx::text(lifted, "why")
        )
    }
}
