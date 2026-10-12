//! `agentplaza events`: Stream the plaza's live events, one line each (HH:MM:SS in UTC). The VM
//! console (`serial`) is left out unless `--kinds` names it or `--all` is given.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_http::ws::{self, Limits, Message};
use serde_json::Value;

use crate::ctx::Ctx;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Only these event kinds, comma-separated (for example `agent,approval`).
    #[arg(long, value_delimiter = ',')]
    pub kinds: Vec<String>,
    /// Only events about this agent (its CID).
    #[arg(long)]
    pub agent: Option<u64>,
    /// Include the VM console (`serial`) events, which are noisy.
    #[arg(long)]
    pub all: bool,
}

/// Streams until Ctrl-C. A dropped socket reconnects with backoff (1s doubling to 30s).
pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let mut backoff = Duration::from_secs(1);
    loop {
        if let Err(e) = session(ctx, &args, &mut backoff) {
            eprintln!("{} lost the hub: {e}; reconnecting in {}s", stamp(), backoff.as_secs());
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// One socket: reads frames until it fails. `backoff` resets once the socket is up.
fn session(ctx: &Ctx, args: &Args, backoff: &mut Duration) -> Result<(), String> {
    let mut socket = ws::connect(&ctx.client.target, "/v1/ws", Some("apz.events.v1"), Duration::from_secs(10), Limits::default())
        .map_err(|e| format!("{}: {e}", ctx.client.base()))?;
    *backoff = Duration::from_secs(1);
    eprintln!("{} connected to {}", stamp(), ctx.client.base());
    loop {
        let Some(message) = socket.recv(Duration::from_secs(60)).map_err(|e| e.to_string())? else {
            continue;
        };
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(kind) = frame.get("event").and_then(Value::as_str) else {
            continue;
        };
        let data = frame.get("data").cloned().unwrap_or(Value::Null);
        if !keep(args, kind, &data) {
            continue;
        }
        if ctx.json {
            println!("{}", serde_json::to_string(&frame).unwrap_or_default());
        } else {
            println!("{} {kind} {}", stamp(), summary(&data));
        }
    }
}

/// Whether the event passes `--kinds`, `--agent` and the console filter.
fn keep(args: &Args, kind: &str, data: &Value) -> bool {
    let named = args.kinds.iter().any(|k| k.trim() == kind);
    if !args.kinds.is_empty() && !named {
        return false;
    }
    if kind == "serial" && !named && !args.all {
        return false;
    }
    match args.agent {
        Some(cid) => data.get("cid").and_then(Value::as_u64) == Some(cid),
        None => true,
    }
}

/// The event's scalar fields as `key=value` (at most six), on one line and cut to 160 characters.
fn summary(data: &Value) -> String {
    let text = match data.as_object() {
        Some(map) => map
            .iter()
            .filter(|(_, v)| v.is_string() || v.is_number() || v.is_boolean())
            .take(6)
            .map(|(k, v)| match v {
                Value::String(s) => format!("{k}={s}"),
                other => format!("{k}={other}"),
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => data.to_string(),
    };
    let one_line = text.replace(['\n', '\r'], " ");
    if one_line.chars().count() > 160 { one_line.chars().take(159).collect::<String>() + "…" } else { one_line }
}

/// The current time as HH:MM:SS (UTC).
fn stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) % 86_400;
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}
