//! `agentplaza nodes`: The network's nodes: list, one node, join codes, drain, revoke.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};

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
    /// The join codes: state (active, used, expired, revoked), when made and when they end, and who used them.
    /// `codes revoke ID` takes an unused code back.
    Codes {
        #[command(subcommand)]
        action: Option<CodesCmd>,
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

#[derive(Subcommand, Debug)]
pub enum CodesCmd {
    /// Revoke a join code nobody has used yet: it can no longer join a machine.
    Revoke {
        /// The code's id (from `agentplaza nodes codes`).
        id: String,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let nodes = ctx.client.get("/v1/nodes")?;
            ctx.out(&nodes, |v| {
                let rows: Vec<Vec<String>> = v
                    .as_array()
                    .map(|a| a.iter().map(row).collect())
                    .unwrap_or_default();
                ctx::table(
                    &["id", "host", "state", "cids", "agents", "last seen"],
                    &rows,
                );
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
            let machine = ctx
                .client
                .get(&format!("/v1/nodes/{}/machine", ctx::seg(&id)))
                .ok();
            let value = json!({ "node": node, "machine": machine });
            ctx.out(&value, |v| {
                let n = &v["node"];
                let m = &v["machine"];
                let machine_line = if m.is_null() {
                    "unknown (the node has not answered)".to_string()
                } else {
                    format!(
                        "{} {} cpus, {} MiB, {}",
                        ctx::s(m, "arch"),
                        ctx::text(m, "cpus"),
                        ctx::text(m, "memory_mib"),
                        ctx::s(m, "backend")
                    )
                };
                ctx::fields(&[
                    ("id", ctx::s(n, "id").to_string()),
                    ("name", ctx::s(n, "name").to_string()),
                    ("host", host(n)),
                    ("state", state(n).to_string()),
                    ("address", ctx::s(n, "address").to_string()),
                    ("cids", cids(n)),
                    ("agents", ctx::text(n, "agents")),
                    ("last seen", last_seen(n)),
                    ("machine", machine_line),
                ]);
            });
        }
        Cmd::Code { label, ttl } => {
            let made = ctx.client.post(
                "/v1/nodes/codes",
                &json!({ "label": label, "ttl_minutes": ttl }),
            )?;
            ctx.out(&made, |v| {
                let token = ctx::s(v, "token");
                println!("join code for {label}, valid {ttl} minutes (shown once):");
                println!("  {token}");
                println!("on the machine: agentplaza node join {token}");
            });
        }
        Cmd::Codes { action: None } => {
            let codes = ctx.client.get("/v1/nodes/codes")?;
            ctx.out(&codes, |v| {
                let rows: Vec<Vec<String>> = v
                    .as_array()
                    .map(|a| a.iter().map(code_row).collect())
                    .unwrap_or_default();
                ctx::table(
                    &["id", "label", "state", "created", "expires", "used"],
                    &rows,
                );
            });
        }
        Cmd::Codes {
            action: Some(CodesCmd::Revoke { id }),
        } => {
            let revoked = ctx
                .client
                .delete(&format!("/v1/nodes/codes/{}", ctx::seg(&id)))?;
            ctx.out(&revoked, |_| println!("join code {id} revoked"));
        }
        Cmd::Drain { id } => {
            let node = ctx.client.post(
                &format!("/v1/nodes/{}/drain", ctx::seg(&id)),
                &json!({ "on": true }),
            )?;
            ctx.out(&node, |v| println!("{} is draining", name_or(v, &id)));
        }
        Cmd::Undrain { id } => {
            let node = ctx.client.post(
                &format!("/v1/nodes/{}/drain", ctx::seg(&id)),
                &json!({ "on": false }),
            )?;
            ctx.out(&node, |v| {
                println!("{} is back in service", name_or(v, &id))
            });
        }
        Cmd::Revoke { id, yes } => {
            if !confirm(
                yes,
                &format!("Revoke node {id}? It leaves the network for good."),
            )? {
                println!("not revoked");
                return Ok(());
            }
            let node = ctx
                .client
                .post(&format!("/v1/nodes/{}/revoke", ctx::seg(&id)), &json!({}))?;
            ctx.out(&node, |v| println!("{} revoked", name_or(v, &id)));
        }
    }
    Ok(())
}

fn name_or(v: &Value, id: &str) -> String {
    let name = ctx::s(v, "name");
    if name.is_empty() {
        id.to_string()
    } else {
        name.to_string()
    }
}

fn row(n: &Value) -> Vec<String> {
    vec![
        ctx::s(n, "id").to_string(),
        host(n),
        state(n).to_string(),
        cids(n),
        ctx::text(n, "agents"),
        last_seen(n),
    ]
}

fn code_row(c: &Value) -> Vec<String> {
    vec![
        ctx::s(c, "id").to_string(),
        ctx::s(c, "label").to_string(),
        ctx::s(c, "state").to_string(),
        c.get("created_ms")
            .map_or_else(|| "-".to_string(), ctx::when),
        c.get("expires_ms")
            .map_or_else(|| "-".to_string(), ctx::when),
        used_by(c),
    ]
}

/// Who used a join code: the machine's name, or "-" while it is unused.
fn used_by(c: &Value) -> String {
    let who = ctx::s(c, "used_by");
    if who.is_empty() {
        "-".to_string()
    } else {
        who.to_string()
    }
}

/// The machine's host name as the node reports it, else the node's name.
fn host(n: &Value) -> String {
    let host = n
        .get("machine")
        .and_then(|m| m.get("host"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if host.is_empty() {
        ctx::s(n, "name").to_string()
    } else {
        host.to_string()
    }
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
    if count == 0 {
        "-".to_string()
    } else {
        format!("{}-{}", base, base + count - 1)
    }
}

/// "just now", "5 min ago" from `last_seen_ms`; "never" when the node has not been seen.
fn last_seen(n: &Value) -> String {
    match n.get("last_seen_ms") {
        None | Some(Value::Null) => "never".to_string(),
        Some(seen) => ctx::ago(seen),
    }
}
