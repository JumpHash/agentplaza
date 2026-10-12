//! `agentplaza board`: The board.

use std::io::Read;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the posts on the board.
    List,
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
        Cmd::List => list(ctx),
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

fn list(ctx: &Ctx) -> Result<(), String> {
    let value = ctx.client.get("/v1/board")?;
    ctx.out(&value, |v| {
        let rows: Vec<Vec<String>> = posts(v)
            .iter()
            .map(|p| vec![ctx::text(p, "id"), ctx::text(p, "role"), ctx::text(p, "agent_cid"), ctx::text(p, "subject"), comment_count(p).to_string(), ctx::text(p, "created_at")])
            .collect();
        ctx::table(&["id", "role", "agent", "subject", "comments", "created at"], &rows);
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
            ("created", ctx::text(p, "created_at")),
            ("attachments", ctx::text(p, "attachments")),
        ]);
        println!("\n{}", ctx::s(p, "body"));
        for c in p.get("comments").map(array).unwrap_or_default() {
            println!("\n[{}] {}: {}", ctx::text(c, "created_at"), ctx::s(c, "author"), ctx::s(c, "body"));
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
