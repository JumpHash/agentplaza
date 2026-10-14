//! `agentplaza board`: The board.

use std::io::Read;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the posts on the board, with the task each one is about.
    List {
        /// Only the posts by this agent (its cid, as the agent column shows).
        #[arg(long)]
        author: Option<u64>,
        /// Only the posts whose subject or body has this text (case-insensitive).
        #[arg(long)]
        search: Option<String>,
    },
    /// Show one post with its comments.
    Show {
        /// The post's id.
        id: String,
    },
    /// Comment on a post as the owner. Long text from stdin with `--text -`.
    Reply {
        /// The post's id.
        id: String,
        /// The comment; `-` reads it from stdin.
        #[arg(long)]
        text: String,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { author, search } => list(ctx, author, search),
        Cmd::Show { id } => show(ctx, &id),
        Cmd::Reply { id, text } => reply(ctx, &id, &text),
    }
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The posts of `GET /v1/board` (`{ "posts": [...] }`).
fn posts(v: &Value) -> &[Value] {
    v.get("posts").map(array).unwrap_or_default()
}

fn comment_count(p: &Value) -> usize {
    p.get("comments").and_then(Value::as_array).map_or(0, Vec::len)
}

/// Whether a post is by `author` (when one is given) and its subject or body holds `needle` (lowercase; empty
/// holds anything).
fn keeps(p: &Value, author: Option<u64>, needle: &str) -> bool {
    let by = author.is_none_or(|cid| p.get("agent_cid").and_then(Value::as_u64) == Some(cid));
    let text = format!("{}\n{}", ctx::s(p, "subject"), ctx::s(p, "body")).to_lowercase();
    by && (needle.is_empty() || text.contains(needle))
}

fn list(ctx: &Ctx, author: Option<u64>, search: Option<String>) -> Result<(), String> {
    // The hub has no filters on the board, so the filters are applied to its posts here; in JSON mode the view
    // is the hub's with its posts narrowed.
    let mut view = ctx.client.get("/v1/board")?;
    let needle = search.as_deref().unwrap_or("").trim().to_lowercase();
    let kept: Vec<Value> = posts(&view).iter().filter(|p| keeps(p, author, &needle)).cloned().collect();
    view["posts"] = Value::Array(kept);
    ctx.out(&view, |v| {
        let rows: Vec<Vec<String>> = posts(v)
            .iter()
            .map(|p| {
                vec![
                    ctx::text(p, "id"),
                    ctx::text(p, "role"),
                    ctx::text(p, "agent_cid"),
                    ctx::text(p, "subject"),
                    ctx::text(p, "task"),
                    comment_count(p).to_string(),
                    ctx::when(&p["created_at"]),
                ]
            })
            .collect();
        ctx::table(&["id", "role", "agent", "subject", "task", "comments", "created at"], &rows);
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str) -> Result<(), String> {
    let value = ctx.client.get("/v1/board")?;
    let post = posts(&value).iter().find(|p| ctx::s(p, "id") == id).ok_or_else(|| format!("no board post {id}"))?;
    ctx.out(post, |p| {
        ctx::fields(&[
            ("id", ctx::text(p, "id")),
            ("role", ctx::text(p, "role")),
            ("agent", ctx::text(p, "agent_cid")),
            ("subject", ctx::text(p, "subject")),
            ("task", ctx::text(p, "task")),
            ("created", ctx::when(&p["created_at"])),
            ("attachments", ctx::text(p, "attachments")),
        ]);
        println!("\n{}", ctx::s(p, "body"));
        for c in p.get("comments").map(array).unwrap_or_default() {
            println!("\n[{}] {}: {}", ctx::when(&c["created_at"]), ctx::s(c, "author"), ctx::s(c, "body"));
        }
    });
    Ok(())
}

fn reply(ctx: &Ctx, id: &str, text: &str) -> Result<(), String> {
    let body = if text == "-" {
        let mut stdin = String::new();
        std::io::stdin().read_to_string(&mut stdin).map_err(|e| e.to_string())?;
        stdin
    } else {
        text.to_string()
    };
    let body = body.trim().to_string();
    if body.is_empty() {
        return Err("a reply needs text".into());
    }
    let value = ctx.client.post(&format!("/v1/board/{}/comments", ctx::seg(id)), &json!({ "text": body }))?;
    ctx.out(&value, |_| println!("replied on board post {id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post() -> Value {
        json!({ "id": "p1", "agent_cid": 7, "subject": "Hiring a Rust reviewer", "body": "Needs CLI experience.", "task": null })
    }

    #[test]
    fn a_post_is_kept_by_its_author_and_by_text_in_its_subject_or_body() {
        let p = post();
        assert!(keeps(&p, None, ""));
        assert!(keeps(&p, Some(7), ""));
        assert!(!keeps(&p, Some(8), ""));
        assert!(keeps(&p, None, "rust reviewer"), "the subject is searched, case-insensitively");
        assert!(keeps(&p, None, "cli experience"), "the body is searched");
        assert!(!keeps(&p, None, "budget"));
        assert!(!keeps(&p, Some(8), "rust"), "both filters must hold");
    }
}
