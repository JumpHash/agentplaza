//! `agentplaza overview`: the plaza at a glance: hub, plaza state, nodes, agents, work, spend.

use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

pub fn run(ctx: &Ctx) -> Result<(), String> {
    let overview = ctx.client.get("/v1/overview")?;
    let hub = ctx.client.get("/v1/hub/identity")?;
    let plaza = ctx.client.get("/v1/plaza/state")?;
    let nodes = ctx.client.get("/v1/nodes")?;
    let agents = ctx.client.get("/v1/agents")?;
    let treasury = ctx.client.get("/v1/treasury")?;
    let queued: u64 = agents
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| a.get("queue").and_then(Value::as_u64))
                .sum()
        })
        .unwrap_or(0);
    let report = json!({
        "hub": hub,
        "plaza": plaza,
        "overview": overview,
        "nodes": nodes,
        "queued": queued,
        "spent": treasury.get("spent").cloned().unwrap_or(Value::Null),
        "plaza_budget": treasury.get("plaza_budget").cloned().unwrap_or(Value::Null),
    });
    ctx.out(&report, print);
    Ok(())
}

fn print(r: &Value) {
    let hub = &r["hub"];
    let o = &r["overview"];
    ctx::fields(&[
        (
            "hub",
            format!(
                "{} {} ({})",
                ctx::s(hub, "name"),
                ctx::s(hub, "version"),
                ctx::s(hub, "url")
            ),
        ),
        ("plaza", plaza_line(&r["plaza"])),
        (
            "agents",
            format!(
                "{} total: {} working, {} idle, {} offline; {} queued",
                ctx::text(&o["agents"], "total"),
                ctx::text(&o["agents"], "working"),
                ctx::text(&o["agents"], "idle"),
                ctx::text(&o["agents"], "offline"),
                ctx::text(r, "queued"),
            ),
        ),
        ("tasks", tasks_line(&o["tasks"])),
        (
            "models",
            format!(
                "worker {} · orchestrator {}",
                ctx::s(o, "default_model"),
                ctx::s(o, "orchestrator_model")
            ),
        ),
        (
            "spent",
            format!(
                "{} of a plaza budget of {}",
                ctx::text(r, "spent"),
                ctx::text(r, "plaza_budget")
            ),
        ),
    ]);
    println!();
    let rows: Vec<Vec<String>> = r["nodes"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|n| {
                    vec![
                        ctx::s(n, "name").to_string(),
                        ctx::s(n, "address").to_string(),
                        node_state(n),
                        ctx::text(n, "agents"),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    println!("nodes");
    ctx::table(&["name", "address", "state", "agents"], &rows);
}

fn plaza_line(p: &Value) -> String {
    if p["paused"].as_bool().unwrap_or(false) {
        format!(
            "STOPPED by {}: {} ({} work item(s) cancelled)",
            ctx::s(p, "by"),
            ctx::s(p, "reason"),
            ctx::text(p, "cancelled")
        )
    } else {
        "running".to_string()
    }
}

fn tasks_line(tasks: &Value) -> String {
    let Some(map) = tasks.as_object() else {
        return "none".to_string();
    };
    let mut parts: Vec<String> = map
        .iter()
        .map(|(state, n)| format!("{n} {state}"))
        .collect();
    if parts.is_empty() {
        return "none".to_string();
    }
    parts.sort();
    parts.join(", ")
}

/// As the node list shows it: a stopped node stays "stopped" even with its machine off.
fn node_state(n: &Value) -> String {
    let hub = ctx::s(n, "state");
    if n["revoked"].as_bool().unwrap_or(false) {
        "revoked".to_string()
    } else if hub == "stopping" {
        match n["left"].as_u64() {
            Some(left) => format!("stopping ({left} to move)"),
            None => "stopping".to_string(),
        }
    } else if hub == "stopped" || (hub.is_empty() && n["draining"].as_bool().unwrap_or(false)) {
        "stopped".to_string()
    } else if !n["online"].as_bool().unwrap_or(false) {
        "offline".to_string()
    } else {
        "online".to_string()
    }
}
