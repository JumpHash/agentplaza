//! `agentplaza nodes`: The network's nodes: list, one node, join codes, stop and resume (drain and
//! undrain are their older names), rebalance, revoke.

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
    /// Take a node out of service: its agents move to the other machines, one at a time.
    ///
    /// No new agent starts on it. Once it is stopped, its machine can be turned off. `--now`
    /// interrupts the turns in progress (each resumes on its agent's new machine); otherwise a
    /// working agent moves when its turn ends.
    Stop {
        /// The node's id.
        id: String,
        /// Interrupt the turns in progress instead of waiting for them to end.
        #[arg(long)]
        now: bool,
    },
    /// Put a stopped node back in service; agents move onto it once it reports, unless --no-rebalance.
    Resume {
        /// The node's id.
        id: String,
        /// Do not move agents onto the node.
        #[arg(long)]
        no_rebalance: bool,
    },
    /// Spread the idle agents over the machines in service, by memory.
    Rebalance {
        /// Print the moves without making them.
        #[arg(long)]
        dry_run: bool,
    },
    /// The older name of `stop` (without `--now`): its agents move off as their turns end.
    Drain {
        /// The node's id.
        id: String,
    },
    /// The older name of `resume`: put the node back in service.
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
                let mut pairs = vec![
                    ("id", ctx::s(n, "id").to_string()),
                    ("name", ctx::s(n, "name").to_string()),
                    ("host", host(n)),
                    ("state", state(n)),
                    ("address", ctx::s(n, "address").to_string()),
                    ("cids", cids(n)),
                    ("agents", ctx::text(n, "agents")),
                    ("last seen", last_seen(n)),
                    ("machine", machine_line),
                ];
                let waiting = waiting_line(n);
                if !waiting.is_empty() {
                    pairs.push(("waiting", waiting));
                }
                ctx::fields(&pairs);
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
        Cmd::Stop { id, now } => {
            let node = ctx.client.post(
                &format!("/v1/nodes/{}/stop", ctx::seg(&id)),
                &json!({ "now": now }),
            )?;
            ctx.out(&node, |v| {
                println!("{}", stopping_line(&name_or(v, &id), now))
            });
        }
        Cmd::Resume { id, no_rebalance } => {
            let node = ctx.client.post(
                &format!("/v1/nodes/{}/resume", ctx::seg(&id)),
                &json!({ "rebalance": !no_rebalance }),
            )?;
            ctx.out(&node, |v| {
                println!("{} is back in service", name_or(v, &id))
            });
        }
        Cmd::Rebalance { dry_run } => {
            let plan = ctx
                .client
                .post("/v1/nodes/rebalance", &json!({ "dry_run": dry_run }))?;
            ctx.out(&plan, |v| {
                let moves: &[Value] = v["moves"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                println!("{}", rebalance_heading(moves.len(), dry_run));
                if !moves.is_empty() {
                    let rows: Vec<Vec<String>> = moves.iter().map(move_row).collect();
                    ctx::table(&["cid", "from", "to"], &rows);
                }
            });
        }
        Cmd::Drain { id } => {
            let node = ctx.client.post(
                &format!("/v1/nodes/{}/drain", ctx::seg(&id)),
                &json!({ "on": true }),
            )?;
            ctx.out(&node, |v| {
                println!("{}", stopping_line(&name_or(v, &id), false))
            });
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
        state(n),
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

/// The node's state as people read it: revoked; stopping, with the agents still to move; stopped;
/// else online or offline. An older hub sends no `state`: there, draining means stopped.
fn state(n: &Value) -> String {
    let flag = |key: &str| n.get(key).and_then(Value::as_bool).unwrap_or(false);
    let hub = ctx::s(n, "state");
    if flag("revoked") {
        "revoked".to_string()
    } else if hub == "stopping" {
        match n.get("left").and_then(Value::as_u64) {
            Some(left) => format!("stopping ({left} to move)"),
            None => "stopping".to_string(),
        }
    } else if hub == "stopped" || (hub.is_empty() && flag("draining")) {
        "stopped".to_string()
    } else if flag("online") {
        "online".to_string()
    } else {
        "offline".to_string()
    }
}

/// The agents that cannot leave a stopping node yet, as "agent CID: reason" entries joined by
/// "; " ("" when there are none).
fn waiting_line(n: &Value) -> String {
    n.get("waiting")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|w| format!("agent {}: {}", ctx::text(w, "cid"), ctx::s(w, "reason")))
        .collect::<Vec<String>>()
        .join("; ")
}

/// What `stop` (and the older `drain`) says: the node's agents move now, or as their turns end.
fn stopping_line(name: &str, now: bool) -> String {
    let when = if now { "now" } else { "as their turns end" };
    format!("{name} is stopping: its agents move {when}")
}

/// The line above a rebalance's moves: "moving N agents:", "would move N agents:" on a dry run,
/// or that the agents are already spread when there are none.
fn rebalance_heading(moves: usize, dry_run: bool) -> String {
    if moves == 0 {
        return "the agents are already spread over the machines".to_string();
    }
    let verb = if dry_run { "would move" } else { "moving" };
    let agents = if moves == 1 { "agent" } else { "agents" };
    format!("{verb} {moves} {agents}:")
}

/// One move as a table row: the agent's CID, the node it leaves and the node it goes to.
fn move_row(m: &Value) -> Vec<String> {
    vec![
        ctx::text(m, "cid"),
        ctx::text(m, "from"),
        ctx::text(m, "to"),
    ]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revoked_node_is_revoked_whatever_else_it_says() {
        let n = json!({ "revoked": true, "state": "stopping", "left": 2, "draining": true });
        assert_eq!(state(&n), "revoked");
    }

    #[test]
    fn a_stopping_node_says_how_many_agents_are_left_to_move() {
        let n = json!({ "state": "stopping", "left": 3, "draining": true, "online": true });
        assert_eq!(state(&n), "stopping (3 to move)");
        let unknown = json!({ "state": "stopping", "draining": true });
        assert_eq!(state(&unknown), "stopping");
    }

    #[test]
    fn a_stopped_node_is_stopped_even_when_its_machine_is_off() {
        let n = json!({ "state": "stopped", "draining": true, "online": false });
        assert_eq!(state(&n), "stopped");
    }

    #[test]
    fn an_older_hub_reads_draining_as_stopped() {
        let n = json!({ "draining": true, "online": true });
        assert_eq!(state(&n), "stopped");
    }

    #[test]
    fn an_active_node_is_online_or_offline() {
        assert_eq!(
            state(&json!({ "state": "active", "online": true })),
            "online"
        );
        assert_eq!(
            state(&json!({ "state": "active", "online": false })),
            "offline"
        );
        assert_eq!(state(&json!({ "online": true })), "online");
        assert_eq!(state(&json!({})), "offline");
    }

    #[test]
    fn stop_says_when_the_agents_move() {
        assert_eq!(
            stopping_line("box", true),
            "box is stopping: its agents move now"
        );
        assert_eq!(
            stopping_line("box", false),
            "box is stopping: its agents move as their turns end"
        );
    }

    #[test]
    fn the_rebalance_heading_counts_the_moves() {
        assert_eq!(rebalance_heading(3, false), "moving 3 agents:");
        assert_eq!(rebalance_heading(1, false), "moving 1 agent:");
        assert_eq!(rebalance_heading(2, true), "would move 2 agents:");
    }

    #[test]
    fn a_rebalance_with_no_moves_says_the_agents_are_spread() {
        let spread = "the agents are already spread over the machines";
        assert_eq!(rebalance_heading(0, false), spread);
        assert_eq!(rebalance_heading(0, true), spread);
    }

    #[test]
    fn a_move_is_a_cid_from_and_to_row() {
        let m = json!({ "cid": 4, "from": "n1", "to": "n2" });
        assert_eq!(move_row(&m), ["4", "n1", "n2"]);
    }

    #[test]
    fn a_stopping_node_lists_the_agents_that_cannot_leave() {
        let n = json!({ "waiting": [
            { "cid": 4, "reason": "no machine has the memory" },
            { "cid": 9, "reason": "finishing its turn first" }
        ] });
        assert_eq!(
            waiting_line(&n),
            "agent 4: no machine has the memory; agent 9: finishing its turn first"
        );
        assert_eq!(waiting_line(&json!({})), "");
    }
}
