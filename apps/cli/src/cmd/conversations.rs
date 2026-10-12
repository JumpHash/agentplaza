//! `agentplaza conversations`: Conversations.

use std::collections::BTreeMap;

use clap::Subcommand;
use serde_json::Value;

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the conversations, one line per pair of agents.
    List,
    /// Show the messages between two agents, oldest first.
    Show {
        /// The two agent ids, as `3-7`.
        pair: String,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => list(ctx),
        Cmd::Show { pair } => show(ctx, &pair),
    }
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The messages of `GET /v1/conversations` (`{ "conversations": [...] }`).
fn messages(v: &Value) -> &[Value] {
    v.get("conversations").map(array).unwrap_or_default()
}

/// The ordered pair `low-high` of the two agents a message is between.
fn pair_key(m: &Value) -> Option<String> {
    let a = m.get("from_cid")?.as_i64()?;
    let b = m.get("to_cid")?.as_i64()?;
    Some(if a <= b { format!("{a}-{b}") } else { format!("{b}-{a}") })
}

fn at(m: &Value) -> f64 {
    m.get("created_at").and_then(Value::as_f64).unwrap_or(0.0)
}

/// `3-7` or `7-3` as the ordered key.
fn normalize(pair: &str) -> Result<String, String> {
    let ids: Vec<i64> = pair
        .split('-')
        .map(|id| id.trim().parse::<i64>().map_err(|_| format!("`{pair}` is two agent ids, like 3-7")))
        .collect::<Result<_, _>>()?;
    match ids.as_slice() {
        [a, b] => Ok(if a <= b { format!("{a}-{b}") } else { format!("{b}-{a}") }),
        _ => Err(format!("`{pair}` is two agent ids, like 3-7")),
    }
}

fn list(ctx: &Ctx) -> Result<(), String> {
    let value = ctx.client.get("/v1/conversations")?;
    ctx.out(&value, |v| {
        // Per pair: how many messages, and the newest one.
        let mut pairs: BTreeMap<String, (usize, &Value)> = BTreeMap::new();
        for m in messages(v) {
            let Some(key) = pair_key(m) else { continue };
            let entry = pairs.entry(key).or_insert((0, m));
            entry.0 += 1;
            if at(m) >= at(entry.1) {
                entry.1 = m;
            }
        }
        let rows: Vec<Vec<String>> = pairs
            .iter()
            .map(|(key, (count, last))| {
                vec![
                    key.clone(),
                    count.to_string(),
                    format!("{} {}", ctx::s(last, "from_role"), ctx::text(last, "from_cid")),
                    format!("{} {}", ctx::s(last, "to_role"), ctx::text(last, "to_cid")),
                    ctx::when(&last["created_at"]),
                    ctx::text(last, "text"),
                ]
            })
            .collect();
        ctx::table(&["pair", "messages", "from", "to", "last at", "last message"], &rows);
    });
    Ok(())
}

fn show(ctx: &Ctx, pair: &str) -> Result<(), String> {
    let key = normalize(pair)?;
    let value = ctx.client.get("/v1/conversations")?;
    let mut thread: Vec<Value> = messages(&value).iter().filter(|m| pair_key(m).as_deref() == Some(key.as_str())).cloned().collect();
    thread.sort_by(|x, y| at(x).total_cmp(&at(y)));
    let thread = Value::Array(thread);
    ctx.out(&thread, |v| {
        if array(v).is_empty() {
            println!("(no messages between {key})");
        }
        for m in array(v) {
            let delivered = if m.get("delivered").and_then(Value::as_bool) == Some(false) { " (not delivered yet)" } else { "" };
            println!(
                "[{}] {} {} -> {} {}{delivered}\n{}\n",
                ctx::when(&m["created_at"]),
                ctx::s(m, "from_role"),
                ctx::text(m, "from_cid"),
                ctx::s(m, "to_role"),
                ctx::text(m, "to_cid"),
                ctx::s(m, "text"),
            );
        }
    });
    Ok(())
}
