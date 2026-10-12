//! `agentplaza rules`: The plaza's laws: what each one allows or forbids, and the infractions the
//! laws stopped.

use clap::Subcommand;
use serde_json::Value;

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The laws, with what each one allows or forbids.
    List {
        /// Only the laws that permit, or only those that forbid.
        #[arg(long, value_parser = ["permit", "forbid"])]
        effect: Option<String>,
    },
    /// One law, explained: what it covers, its wording and why it exists.
    Show {
        /// The law id.
        id: String,
    },
    /// The infractions: actions the laws stopped, newest first.
    Infractions {
        /// How many to fetch.
        #[arg(long, default_value_t = 200)]
        limit: u32,
        /// Only the infractions of this law.
        #[arg(long)]
        law: Option<String>,
    },
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { effect } => {
            let mut v = ctx.client.get("/v1/laws")?;
            if let Some(effect) = effect {
                if let Some(list) = v.as_array_mut() {
                    list.retain(|l| ctx::s(l, "effect") == effect.as_str());
                }
            }
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|l| {
                        vec![
                            ctx::s(l, "id").to_string(),
                            ctx::s(l, "effect").to_string(),
                            ctx::s(l, "severity").to_string(),
                            ctx::text(l, "refusals"),
                            ctx::s(l, "title").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "effect", "severity", "refusals", "title"], &rows);
            });
        }
        Cmd::Show { id } => {
            let v = ctx.client.get("/v1/laws")?;
            let law = array(&v)
                .iter()
                .find(|l| ctx::s(l, "id") == id.as_str())
                .cloned()
                .ok_or_else(|| format!("no law {id}"))?;
            ctx.out(&law, |l| {
                let ops: Vec<String> = array(l.get("ops").unwrap_or(&Value::Null)).iter().map(|o| o.as_str().unwrap_or_default().to_string()).collect();
                let conditions: Vec<String> = array(l.get("conditions").unwrap_or(&Value::Null)).iter().map(|c| c.as_str().unwrap_or_default().to_string()).collect();
                ctx::fields(&[
                    ("id", ctx::text(l, "id")),
                    ("title", ctx::text(l, "title")),
                    ("effect", ctx::text(l, "effect")),
                    ("severity", ctx::text(l, "severity")),
                    ("enforcement", ctx::text(l, "enforcement")),
                    ("applies to", format!("{} -> {}", ctx::s(l, "principal"), ctx::s(l, "resource"))),
                    ("operations", ops.join(", ")),
                    ("statement", ctx::text(l, "statement")),
                    ("in short", ctx::text(l, "explanation")),
                    ("why", ctx::text(l, "rationale")),
                    ("conditions", if conditions.is_empty() { "none".to_string() } else { conditions.join("; ") }),
                    ("file", ctx::text(l, "file")),
                    ("refusals", ctx::text(l, "refusals")),
                ]);
            });
        }
        Cmd::Infractions { limit, law } => {
            let mut v = ctx.client.get(&format!("/v1/infractions?limit={limit}"))?;
            if let Some(law) = law {
                if let Some(list) = v.as_array_mut() {
                    list.retain(|i| ctx::s(i, "law") == law.as_str());
                }
            }
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|i| {
                        vec![
                            ctx::when(&i["at_ms"]),
                            ctx::s(i, "agent_name").to_string(),
                            ctx::s(i, "law").to_string(),
                            ctx::s(i, "tool").to_string(),
                            ctx::s(i, "detail").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["at (ms)", "agent", "law", "tool", "detail"], &rows);
            });
        }
    }
    Ok(())
}
