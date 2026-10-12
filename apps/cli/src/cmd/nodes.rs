//! `agentplaza nodes`: The network's nodes: list, one node, join codes, drain, revoke.

use std::io::{self, BufRead, IsTerminal, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them: id, host, state, CIDs, agents, last seen.
    List,
    /// One node, with its machine.
    Show {
        /// The node's id.
        id: String,
    },
    /// Make a one-time join code (shown once). On the machine: `agentplaza node join <code>`.
    Code {
        /// What the machine is called here.
        #[arg(long, default_value = "node")]
        label: String,
        /// Minutes the code stays valid.
        #[arg(long, default_value_t = 15)]
        ttl: u64,
    },
    /// Stop new agents on a node (the ones running keep going).
    Drain {
        /// The node's id.
        id: String,
    },
    /// Put a drained node back in service.
    Undrain {
        /// The node's id.
        id: String,
    },
    /// Revoke a node: it leaves the network for good. Asks first on a terminal unless --yes.
    Revoke {
        /// The node's id.
        id: String,
        /// Revoke without asking.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let nodes = ctx.client.get("/v1/nodes")?;
            ctx.out(&nodes, |v| {
                let rows: Vec<Vec<String>> = v.as_array().map(|a| a.iter().map(row).collect()).unwrap_or_default();
                ctx::table(&["id", "host", "state", "cids", "agents", "last seen"], &rows);
            });
        }
        Cmd::Show { id } => {
            let nodes = ctx.client.get("/v1/nodes")?;
            let node = nodes
                .as_array()
                .and_then(|a| a.iter().find(|n| ctx::s(n, "id") == id))
                .cloned()
                .ok_or_else(|| format!("no node {id}"))?;
            // The machine's details need the node online; without them the list's view still shows.
            let machine = ctx.client.get(&format!("/v1/nodes/{}/machine", ctx::seg(&id))).ok();
            let value = json!({ "node": node, "machine": machine });
            ctx.out(&value, |v| {
                let n = &v["node"];
                let m = &v["machine"];
                let machine_line = if m.is_null() {
                    "unknown (the node has not answered)".to_string()
                } else {
                    format!("{} {} cpus, {} MiB, {}", ctx::s(m, "arch"), ctx::text(m, "cpus"), ctx::text(m, "memory_mib"), ctx::s(m, "backend"))
                };
                ctx::fields(&[
                    ("id", ctx::s(n, "id").to_string()),
                    ("name", ctx::s(n, "name").to_string()),
                    ("host", host(n)),
                    ("state", state(n).to_string()),
                    ("address", ctx::s(n, "address").to_string()),
                    ("cids", cids(n)),
                    ("agents", ctx::text(n, "agents")),
                    ("last seen", ago(n)),
                    ("machine", machine_line),
                ]);
            });
        }
        Cmd::Code { label, ttl } => {
            let made = ctx.client.post("/v1/nodes/codes", &json!({ "label": label, "ttl_minutes": ttl }))?;
            ctx.out(&made, |v| {
                let token = ctx::s(v, "token");
                println!("join code for {label}, valid {ttl} minutes (shown once):");
                println!("  {token}");
                println!("on the machine: agentplaza node join {token}");
            });
        }
        Cmd::Drain { id } => {
            let node = ctx.client.post(&format!("/v1/nodes/{}/drain", ctx::seg(&id)), &json!({ "on": true }))?;
            ctx.out(&node, |v| println!("{} is draining", name_or(v, &id)));
        }
        Cmd::Undrain { id } => {
            let node = ctx.client.post(&format!("/v1/nodes/{}/drain", ctx::seg(&id)), &json!({ "on": false }))?;
            ctx.out(&node, |v| println!("{} is back in service", name_or(v, &id)));
        }
        Cmd::Revoke { id, yes } => {
            if !yes && !confirm(&format!("Revoke node {id}? It leaves the network for good."))? {
                println!("not revoked");
                return Ok(());
            }
            let node = ctx.client.post(&format!("/v1/nodes/{}/revoke", ctx::seg(&id)), &json!({}))?;
            ctx.out(&node, |v| println!("{} revoked", name_or(v, &id)));
        }
    }
    Ok(())
}

/// Asks a yes/no question on a terminal (`[y/N]`, default no). Off a terminal it refuses: the
/// caller must pass `--yes`.
pub fn confirm(question: &str) -> Result<bool, String> {
    if !io::stdin().is_terminal() {
        return Err("this asks first on a terminal; pass --yes to go ahead".into());
    }
    print!("{question} Proceed? [y/N] ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn name_or(v: &Value, id: &str) -> String {
    let name = ctx::s(v, "name");
    if name.is_empty() { id.to_string() } else { name.to_string() }
}

fn row(n: &Value) -> Vec<String> {
    vec![ctx::s(n, "id").to_string(), host(n), state(n).to_string(), cids(n), ctx::text(n, "agents"), ago(n)]
}

/// The machine's host name as the node reports it, else the node's name.
fn host(n: &Value) -> String {
    let host = n.get("machine").and_then(|m| m.get("host")).and_then(Value::as_str).unwrap_or("");
    if host.is_empty() { ctx::s(n, "name").to_string() } else { host.to_string() }
}

fn state(n: &Value) -> &'static str {
    let flag = |key: &str| n.get(key).and_then(Value::as_bool).unwrap_or(false);
    if flag("revoked") {
        "revoked"
    } else if flag("draining") {
        "draining"
    } else if flag("online") {
        "online"
    } else {
        "offline"
    }
}

/// The inclusive CID range the node serves (`A-B`), or "-" when it has none.
fn cids(n: &Value) -> String {
    let base = n.get("cid_base").and_then(Value::as_u64).unwrap_or(0);
    let count = n.get("cid_count").and_then(Value::as_u64).unwrap_or(0);
    if count == 0 { "-".to_string() } else { format!("{}-{}", base, base + count - 1) }
}

/// "5s ago", "3m ago", "2h ago", "4d ago" from `last_seen_ms`.
fn ago(n: &Value) -> String {
    let Some(seen) = n.get("last_seen_ms").and_then(Value::as_i64) else {
        return "never".to_string();
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let secs = (now - seen) / 1000;
    match secs {
        s if s < 5 => "just now".to_string(),
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}
