//! `agentplaza society`: The society: roles, bonds, reputation, and the agent proposals waiting for
//! the owner.

use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Map, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The agents with their reputation, confidence and wallets, best reputed first.
    List,
    /// The contacts between agents (met, hired, added), with the holder's trust in the peer.
    Bonds,
    /// Rate an agent as the owner: -1 trusts it less, 1 trusts it more.
    Rate {
        /// The agent's cid.
        cid: u64,
        /// A value from -1 to 1.
        #[arg(allow_hyphen_values = true)]
        value: f64,
    },
    /// The roles agents are spawned into, built in and custom.
    Roles,
    /// Save a role from its JSON definition (a file, or - for stdin), as the agent wizard does.
    RoleSave {
        /// The role definition as JSON.
        file: PathBuf,
    },
    /// Delete a custom role.
    RoleDelete {
        /// The role id.
        id: String,
        /// Confirm the deletion (required).
        #[arg(long)]
        yes: bool,
    },
    /// The agent proposals and their states.
    Proposals,
    /// Approve a proposal: spawns its agents, optionally with another count or model.
    Approve {
        /// The proposal id.
        id: String,
        /// Spawn this many instead of the proposed count.
        #[arg(long)]
        count: Option<u64>,
        /// Run them on this model instead of the proposed one.
        #[arg(long)]
        model: Option<String>,
    },
    /// Reject a proposal, with the reason the proposer reads.
    Reject {
        /// The proposal id.
        id: String,
        /// Why it is rejected.
        #[arg(long, default_value = "")]
        reason: String,
    },
}

/// The array at `key` of an object (empty when missing).
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).map(array).unwrap_or(&[])
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// A number field (0 when missing).
fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// The cids with a suspension in force, read from a sanctions register (`GET /v1/sanctions`).
fn suspended_cids(register: &Value) -> BTreeSet<u64> {
    items(register, "suspensions")
        .iter()
        .filter(|s| s.get("active").and_then(Value::as_bool) == Some(true))
        .filter_map(|s| s.get("cid").and_then(Value::as_u64))
        .collect()
}

/// The suspended agents now. The hub answers the suspensions in force on their own (`active=1`), so an old
/// suspension is not missed; a register that cannot be read is said so, and no one is marked.
fn suspended_now(ctx: &Ctx) -> BTreeSet<u64> {
    match ctx.client.get("/v1/sanctions?active=1") {
        Ok(register) => suspended_cids(&register),
        Err(e) => {
            eprintln!("warning: the sanctions register could not be read ({e}), so no agent is marked suspended");
            BTreeSet::new()
        }
    }
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let v = ctx.client.get("/v1/society")?;
            // The mark is for people reading the table; the JSON is the hub's view as it is.
            let suspended = if ctx.json { BTreeSet::new() } else { suspended_now(ctx) };
            ctx.out(&v, |v| {
                let mut agents: Vec<&Value> = items(v, "agents").iter().collect();
                agents.sort_by(|a, b| num(b, "reputation").total_cmp(&num(a, "reputation")));
                let rows: Vec<Vec<String>> = agents
                    .iter()
                    .map(|a| {
                        let cid = a.get("cid").and_then(Value::as_u64);
                        vec![
                            ctx::text(a, "cid"),
                            ctx::s(a, "name").to_string(),
                            ctx::s(a, "role").to_string(),
                            format!("{:.2}", num(a, "reputation")),
                            format!("{:.2}", num(a, "confidence")),
                            ctx::text(a, "wallet"),
                            if cid.is_some_and(|c| suspended.contains(&c)) { "suspended".to_string() } else { String::new() },
                        ]
                    })
                    .collect();
                ctx::table(&["cid", "name", "role", "reputation", "confidence", "wallet", "status"], &rows);
            });
        }
        Cmd::Bonds => {
            let v = ctx.client.get("/v1/society")?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = items(v, "bonds")
                    .iter()
                    .map(|b| {
                        let blocked = b.get("blocked").and_then(Value::as_bool).unwrap_or(false);
                        vec![
                            ctx::text(b, "from"),
                            ctx::text(b, "to"),
                            ctx::s(b, "kind").to_string(),
                            ctx::text(b, "strength"),
                            ctx::text(b, "interactions"),
                            if blocked { "yes".to_string() } else { String::new() },
                            b.get("opinion").map(|o| format!("{:.2}", num(o, "e"))).unwrap_or_default(),
                        ]
                    })
                    .collect();
                ctx::table(&["from", "to", "kind", "strength", "interactions", "blocked", "trust"], &rows);
            });
        }
        Cmd::Rate { cid, value } => {
            if !(-1.0..=1.0).contains(&value) {
                return Err("a rating is a value from -1 to 1".into());
            }
            let v = ctx.client.post(&format!("/v1/agents/{cid}/rating"), &json!({ "value": value }))?;
            ctx.out(&v, |v| println!("rated agent {cid} {value:+.2}; trust now {:.2}", num(v, "e")));
        }
        Cmd::Roles => {
            let v = ctx.client.get("/v1/roles")?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|r| {
                        vec![
                            ctx::s(r, "id").to_string(),
                            ctx::s(r, "title").to_string(),
                            ctx::s(r, "source").to_string(),
                            ctx::text(r, "agents"),
                            r.get("def").map(|d| ctx::text(d, "model")).unwrap_or_default(),
                            r.get("def").map(|d| ctx::text(d, "budget")).unwrap_or_default(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "title", "source", "agents", "model", "budget"], &rows);
            });
        }
        Cmd::RoleSave { file } => {
            let text = if file.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|e| format!("stdin: {e}"))?
            } else {
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?
            };
            let def: Value = serde_json::from_str(&text).map_err(|e| format!("the role is not JSON: {e}"))?;
            let v = ctx.client.post("/v1/roles", &def)?;
            ctx.out(&v, |v| println!("saved role {}", ctx::s(v, "id")));
        }
        Cmd::RoleDelete { id, yes } => {
            if !yes {
                return Err(format!("deleting role {id} needs --yes"));
            }
            let v = ctx.client.delete(&format!("/v1/roles/{}", ctx::seg(&id)))?;
            ctx.out(&v, |_| println!("deleted role {id}"));
        }
        Cmd::Proposals => {
            let v = ctx.client.get("/v1/agent-proposals")?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|p| {
                        vec![
                            ctx::s(p, "id").to_string(),
                            ctx::s(p, "state").to_string(),
                            p.get("role").map(|r| ctx::s(r, "title").to_string()).unwrap_or_default(),
                            ctx::text(p, "count"),
                            ctx::s(p, "model").to_string(),
                            ctx::s(p, "by").to_string(),
                            ctx::s(p, "reason").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "state", "role", "count", "model", "by", "reason"], &rows);
            });
        }
        Cmd::Approve { id, count, model } => {
            let mut edits = Map::new();
            if let Some(count) = count {
                edits.insert("count".into(), json!(count));
            }
            if let Some(model) = model {
                edits.insert("model".into(), json!(model));
            }
            let v = ctx.client.post(&format!("/v1/agent-proposals/{}/approve", ctx::seg(&id)), &Value::Object(edits))?;
            ctx.out(&v, |v| println!("approved proposal {id}; spawned {}", ctx::text(v, "spawned")));
        }
        Cmd::Reject { id, reason } => {
            let v = ctx.client.post(&format!("/v1/agent-proposals/{}/reject", ctx::seg(&id)), &json!({ "reason": reason }))?;
            ctx.out(&v, |_| println!("rejected proposal {id}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_active_suspensions_mark_their_agents() {
        let register = json!({
            "sanctions": [{ "cid": 9 }],
            "suspensions": [
                { "id": 1, "cid": 3, "active": true },
                { "id": 2, "cid": 4, "active": false },
                { "id": 3, "cid": 5 },
                { "id": 4, "cid": 3, "active": true }
            ]
        });
        assert_eq!(suspended_cids(&register), BTreeSet::from([3]));
        assert!(suspended_cids(&json!({})).is_empty());
    }
}
