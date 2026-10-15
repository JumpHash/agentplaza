//! `agents memory` and `agents proposals`: what an agent has written down, and the agents an
//! orchestrator proposes to spawn (the owner decides them).

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum MemoryAction {
    /// One memory note, with its body.
    Show {
        /// The note's slug (from `agents memory`).
        slug: String,
    },
}

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

/// `agents memory CID`: the agent's notes (`--search` narrows them); `show SLUG` reads one.
pub fn memory(
    ctx: &Ctx,
    cid: u64,
    search: Option<&str>,
    action: Option<MemoryAction>,
) -> Result<(), String> {
    match action {
        None => {
            let v = ctx.client.get(&format!("/v1/agents/{cid}/memory"))?;
            let notes: Vec<Value> = super::items(&v["notes"])
                .iter()
                .filter(|n| matches_search(n, search))
                .cloned()
                .collect();
            ctx.out(&Value::Array(notes.clone()), |_| {
                let rows: Vec<Vec<String>> = notes
                    .iter()
                    .map(|n| {
                        vec![
                            ctx::text(n, "slug"),
                            ctx::text(n, "kind"),
                            ctx::text(n, "title"),
                            list_of(n, "tags"),
                            ctx::when(&n["updated_ms"]),
                        ]
                    })
                    .collect();
                ctx::table(&["slug", "kind", "title", "tags", "updated"], &rows);
            });
            Ok(())
        }
        Some(MemoryAction::Show { slug }) => {
            if search.is_some() {
                return Err("--search narrows the list of notes; it does not go with show".into());
            }
            let doc = ctx
                .client
                .get(&format!("/v1/agents/{cid}/memory/{}", ctx::seg(&slug)))?;
            ctx.out(&doc, |d| {
                ctx::fields(&[
                    ("title", ctx::text(d, "title")),
                    ("kind", ctx::text(d, "kind")),
                    ("updated", ctx::when(&d["updated_ms"])),
                    ("tags", list_of(d, "tags")),
                    ("links", list_of(d, "links")),
                    ("backlinks", list_of(d, "backlinks")),
                ]);
                println!();
                println!("{}", ctx::text(d, "body"));
            });
            Ok(())
        }
    }
}

/// Whether a note matches the `--search` text: a case-insensitive part of its slug, title, kind
/// or one of its tags. No text (or blank text) matches every note.
pub fn matches_search(note: &Value, search: Option<&str>) -> bool {
    let Some(query) = search
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty())
    else {
        return true;
    };
    let hit = |text: &str| text.to_lowercase().contains(&query);
    hit(ctx::s(note, "slug"))
        || hit(ctx::s(note, "title"))
        || hit(ctx::s(note, "kind"))
        || strings_of(note, "tags").iter().any(|tag| hit(tag))
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

/// The strings of an array field, as they are (none when the field is missing).
fn strings_of<'a>(v: &'a Value, key: &str) -> Vec<&'a str> {
    v[key]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The strings of an array field, joined with commas.
fn list_of(v: &Value, key: &str) -> String {
    strings_of(v, key).join(", ")
}
