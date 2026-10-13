//! Fleet routing: every call that reaches an agent's gateway (vsockd) or a machine's noded goes
//! through here. The hub's own machine is node `n0`, reached over loopback; a joined node is
//! reached through its command queue (`nodes::Store::enqueue`), which its apz-link long-polls.
//! An agent lives on the node whose CID range holds its CID.

use apz_http::{Params, Request, Response, Router};
use serde_json::{json, Value};

use crate::agents::Status;
use crate::nodes::{Node, OFFLINE_AFTER_MS};
use crate::state::App;

/// The hub's own machine.
pub const LOCAL: &str = "n0";
/// How long a remote node has to answer one command.
pub const REMOTE_WAIT_MS: u64 = 30_000;

/// The node that hosts `cid`: `n0` unless a joined node's range holds it.
pub fn node_of(app: &App, cid: u64) -> String {
    app.nodes.owner_of_cid(cid).map_or_else(|| LOCAL.to_string(), |node| node.id)
}

/// A call to the agent gateway that serves `cid` (`/v1/agents/{cid}/send`, `/stop`, …).
pub fn gateway(app: &App, cid: u64, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    let node = node_of(app, cid);
    if node == LOCAL {
        return app.hub.call(method, path, body);
    }
    remote(app, &node, "hub", method, path, body)
}

/// Pushes one message to agent `cid` (`POST /v1/agents/{cid}/send` on its gateway).
pub fn send(app: &App, cid: u64, body: &Value) -> Result<(u16, Value), String> {
    gateway(app, cid, "POST", &format!("/v1/agents/{cid}/send"), Some(body))
}

/// A call to noded on `node`.
pub fn noded(app: &App, node: &str, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    if node == LOCAL {
        return app.noded.call(method, path, body);
    }
    remote(app, node, "noded", method, path, body)
}

/// A call to the noded that runs agent `cid`'s VM.
pub fn noded_for(app: &App, cid: u64, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    noded(app, &node_of(app, cid), method, path, body)
}

/// A fleet call's outcome as a response for the console: the upstream status and body, or 502.
pub fn respond(result: Result<(u16, Value), String>) -> Response {
    match result {
        Ok((status, body)) => Response::json(status, &body),
        Err(err) => Response::error(502, &err),
    }
}

/// Queues the call on a joined node and waits for its result. Refused at once when the node is
/// unknown, revoked or offline, so nothing piles up in a queue no one reads.
fn remote(app: &App, node: &str, target: &str, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    let Some(record) = app.nodes.get(node) else {
        return Err(format!("no node {node}"));
    };
    if record.revoked {
        return Err(format!("node {node} is revoked"));
    }
    if apz_plaza::now_ms().saturating_sub(record.last_seen_ms) >= crate::nodes::OFFLINE_AFTER_MS {
        return Err(format!("node {node} is offline"));
    }
    let id = app.nodes.enqueue(node, target, method, path, body.cloned().unwrap_or(Value::Null));
    match app.nodes.wait_result(&id, REMOTE_WAIT_MS) {
        Some(result) => Ok((result.status, result.body)),
        None => Err(format!("node {node} did not answer {method} {path}")),
    }
}

/// The gateway status (`/v1/status` rows) and noded runs of every node: `n0` asked directly,
/// joined nodes from their last heartbeat. Each item is `(node, status, runs)`; a node that did
/// not answer is left out.
pub fn reports(app: &App) -> Vec<(String, Value, Value)> {
    let mut all = Vec::new();
    let status = app.hub.get("/v1/status").ok().filter(|(code, _)| *code == 200).map(|(_, body)| body);
    let runs = app.noded.get("/v1/runs").ok().filter(|(code, _)| *code == 200).map(|(_, body)| body);
    if status.is_some() || runs.is_some() {
        all.push((LOCAL.to_string(), status.unwrap_or(Value::Null), runs.unwrap_or(Value::Null)));
    }
    let now = apz_plaza::now_ms();
    for node in app.nodes.list() {
        if node.revoked || now.saturating_sub(node.last_seen_ms) >= crate::nodes::OFFLINE_AFTER_MS {
            continue;
        }
        if let Some((status, runs)) = app.nodes.report(&node.id) {
            all.push((node.id, status, runs));
        }
    }
    all
}

/// An agent placed on a machine, for the fleet view: its CID, the node that runs it, and whether
/// it is working now.
pub struct Placed {
    pub cid: u64,
    pub node: String,
    pub working: bool,
}

/// The hub's own noded as it answered: `/v1/machine` and `/v1/machine/stats`, each None when
/// noded did not answer.
pub struct HubReport {
    pub machine: Option<Value>,
    pub stats: Option<Value>,
}

/// The fleet for the Machines view (`GET /v1/fleet`): the hub's own machine (`n0`, from its
/// noded) and every joined node (from its last heartbeat), each with the agents it runs.
pub fn fleet(app: &App) -> Value {
    let answered = |path: &str| app.noded.get(path).ok().filter(|(code, _)| *code == 200).map(|(_, body)| body);
    let hub = HubReport { machine: answered("/v1/machine"), stats: answered("/v1/machine/stats") };
    // Agents that are offline are not running anywhere, so they are not placed.
    let placed: Vec<Placed> = app
        .agents
        .list()
        .into_iter()
        .filter(|agent| agent.status != Status::Offline)
        .map(|agent| Placed { cid: agent.cid, node: node_of(app, agent.cid), working: agent.status == Status::Working })
        .collect();
    assemble(apz_plaza::now_ms(), &hub, &app.nodes.list(), &placed)
}

/// The fleet JSON from its facts (pure): `machines` (the hub first, then every node in order)
/// and `links` (one per node, to `n0`). A node is reachable when it is not revoked and was
/// seen within `OFFLINE_AFTER_MS`. Status: revoked, else draining, else online or offline.
/// The hub's stats come from its noded; a remote node's come from its last heartbeat (`null` until
/// one carries them), since its noded is reached only through the command queue.
pub fn assemble(now_ms: u64, hub: &HubReport, nodes: &[Node], placed: &[Placed]) -> Value {
    let hub_machine = hub.machine.clone().unwrap_or(Value::Null);
    let hub_status = if hub.machine.is_some() { "online" } else { "offline" };
    let hub_name = hub_machine.get("name").and_then(Value::as_str).unwrap_or("hub").to_string();
    let mut machines = vec![machine_row(
        LOCAL,
        &hub_name,
        "hub",
        hub_status,
        &hub_machine,
        placed,
        stats_of(hub.stats.as_ref()),
        now_ms,
    )];
    let mut links = Vec::new();
    for node in nodes {
        let reachable = !node.revoked && now_ms.saturating_sub(node.last_seen_ms) < OFFLINE_AFTER_MS;
        let status = if node.revoked {
            "revoked"
        } else if node.draining {
            "draining"
        } else if reachable {
            "online"
        } else {
            "offline"
        };
        // A remote node's live stats are the ones its last heartbeat carried.
        let stats = node.machine.get("stats").cloned().unwrap_or(Value::Null);
        machines.push(machine_row(&node.id, &node.name, "node", status, &node.machine, placed, stats, node.last_seen_ms));
        links.push(json!({ "a": LOCAL, "b": node.id, "online": reachable }));
    }
    json!({ "machines": machines, "links": links })
}

/// One machine's row of the fleet view. Machine fields are taken from its reported summary;
/// a field the summary lacks is null (`gpus`: empty).
#[allow(clippy::too_many_arguments)]
fn machine_row(
    id: &str,
    name: &str,
    role: &str,
    status: &str,
    machine: &Value,
    placed: &[Placed],
    stats: Value,
    last_seen_ms: u64,
) -> Value {
    let field = |key: &str| machine.get(key).cloned().unwrap_or(Value::Null);
    let here: Vec<&Placed> = placed.iter().filter(|agent| agent.node == id).collect();
    json!({
        "id": id,
        "name": name,
        "role": role,
        "kind": field("kind"),
        "vendor": field("vendor"),
        "model": field("model"),
        "apple": field("apple"),
        "status": status,
        "arch": field("arch"),
        "os": field("os"),
        "backend": field("backend"),
        "cpu": field("cpu"),
        "memory_mib": field("memory_mib"),
        "gpus": machine.get("gpus").cloned().unwrap_or_else(|| json!([])),
        "storage_gib": field("storage_gib"),
        "agents": here.iter().map(|agent| agent.cid).collect::<Vec<u64>>(),
        "working": here.iter().filter(|agent| agent.working).count(),
        "stats": stats,
        "last_seen_ms": last_seen_ms,
    })
}

/// The live stats of a machine from its noded `/v1/machine/stats` body: CPU percent and memory used.
fn stats_of(body: Option<&Value>) -> Value {
    match body {
        Some(body) => json!({
            "cpu_pct": body.get("cpu_percent").cloned().unwrap_or(Value::Null),
            "mem_used_mib": body.get("mem_used_mib").cloned().unwrap_or(Value::Null),
        }),
        None => Value::Null,
    }
}

/// A node's machine and runs for the console: `GET /v1/nodes/{id}/machine`, `/runs`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/fleet", |app, _, _| Response::json(200, &fleet(app)))
        .get("/v1/nodes/{id}/machine", |app, _, p| respond(noded(app, p.get("id"), "GET", "/v1/machine", None)))
        .get("/v1/nodes/{id}/stats", |app, _, p| respond(noded(app, p.get("id"), "GET", "/v1/machine/stats", None)))
        .get("/v1/nodes/{id}/runs", |app, _, p| respond(noded(app, p.get("id"), "GET", "/v1/runs", None)))
        .post("/v1/nodes/{id}/runs", spawn_on)
}

/// `POST /v1/nodes/{id}/runs`: a raw run on that node (the console's machine view).
fn spawn_on(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Value = if req.body.is_empty() { json!({}) } else {
        match serde_json::from_slice(&req.body) {
            Ok(body) => body,
            Err(_) => return Response::error(400, "the body must be JSON"),
        }
    };
    respond(noded(app, p.get("id"), "POST", "/v1/runs", Some(&body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn node(id: &str, name: &str, last_seen_ms: u64, machine: Value, draining: bool, revoked: bool) -> Node {
        Node {
            id: id.into(),
            name: name.into(),
            public_key: String::new(),
            cid_base: 1000,
            cid_count: 100,
            joined_ms: 0,
            last_seen_ms,
            machine,
            draining,
            revoked,
            address: String::new(),
        }
    }

    fn placed(cid: u64, node: &str, working: bool) -> Placed {
        Placed { cid, node: node.into(), working }
    }

    fn hub() -> HubReport {
        HubReport {
            machine: Some(json!({
                "name": "zarathos-pc", "kind": "workstation", "arch": "x86_64", "os": "linux", "backend": "kvm",
                "cpu": { "model": "AMD Ryzen 9 5900X", "cores": 12, "threads": 24 },
                "memory_mib": 128320,
                "gpus": [{ "name": "NVIDIA GeForce RTX 3090", "vendor": "NVIDIA", "memory_mib": 24576 }],
                "storage_gib": 1863,
            })),
            stats: Some(json!({ "cpu_percent": 7.5, "mem_used_mib": 18000, "load": [0.1, 0.2, 0.3] })),
        }
    }

    #[test]
    fn fleet_lists_the_hub_then_each_node_with_its_status_and_agents() {
        let nodes = [
            node(
                "n3",
                "pi-kitchen",
                NOW - 5_000,
                json!({
                    "kind": "edge", "os": "linux", "arch": "aarch64", "storage_gib": 58,
                    "vendor": "raspberrypi", "model": "Raspberry Pi 5 Model B Rev 1.0", "apple": null,
                    "stats": { "cpu_pct": 3.2, "mem_used_mib": 900 },
                }),
                false,
                false,
            ),
            node("n4", "old-laptop", NOW - 5_000, json!({ "kind": "laptop" }), true, false),
            node("n5", "silent", NOW - OFFLINE_AFTER_MS - 1, json!({}), false, false),
            node("n6", "banned", NOW, json!({}), false, true),
        ];
        let agents = [placed(200, "n0", true), placed(201, "n0", false), placed(1000, "n3", false)];
        let view = assemble(NOW, &hub(), &nodes, &agents);

        let machines = view["machines"].as_array().unwrap();
        assert_eq!(machines.len(), 5);
        let hub_row = &machines[0];
        assert_eq!(hub_row["id"], "n0");
        assert_eq!(hub_row["role"], "hub");
        assert_eq!(hub_row["name"], "zarathos-pc");
        assert_eq!(hub_row["status"], "online");
        assert_eq!(hub_row["agents"], json!([200, 201]));
        assert_eq!(hub_row["working"], 1);
        assert_eq!(hub_row["stats"], json!({ "cpu_pct": 7.5, "mem_used_mib": 18000 }));
        assert_eq!(hub_row["cpu"]["threads"], 24);
        assert_eq!(hub_row["gpus"][0]["vendor"], "NVIDIA");
        assert_eq!(hub_row["vendor"], Value::Null);
        assert_eq!(hub_row["model"], Value::Null);
        assert_eq!(hub_row["apple"], Value::Null);

        let statuses: Vec<(&str, &str)> = machines[1..]
            .iter()
            .map(|row| (row["id"].as_str().unwrap(), row["status"].as_str().unwrap()))
            .collect();
        assert_eq!(statuses, [("n3", "online"), ("n4", "draining"), ("n5", "offline"), ("n6", "revoked")]);

        let pi = &machines[1];
        assert_eq!(pi["role"], "node");
        assert_eq!(pi["name"], "pi-kitchen");
        assert_eq!(pi["kind"], "edge");
        assert_eq!(pi["agents"], json!([1000]));
        assert_eq!(pi["working"], 0);
        assert_eq!(pi["stats"], json!({ "cpu_pct": 3.2, "mem_used_mib": 900 }));
        assert_eq!(pi["vendor"], "raspberrypi");
        assert_eq!(pi["model"], "Raspberry Pi 5 Model B Rev 1.0");
        assert_eq!(pi["apple"], Value::Null);
        assert_eq!(pi["gpus"], json!([]));
        assert_eq!(pi["cpu"], Value::Null);
        assert_eq!(pi["last_seen_ms"], NOW - 5_000);

        let links = view["links"].as_array().unwrap();
        let online: Vec<(&str, bool)> = links.iter().map(|l| (l["b"].as_str().unwrap(), l["online"].as_bool().unwrap())).collect();
        assert_eq!(online, [("n3", true), ("n4", true), ("n5", false), ("n6", false)]);
        assert!(links.iter().all(|l| l["a"] == "n0"));
    }

    #[test]
    fn a_hub_whose_noded_is_down_is_offline_without_stats() {
        let down = HubReport { machine: None, stats: None };
        let view = assemble(NOW, &down, &[], &[]);
        let hub_row = &view["machines"][0];
        assert_eq!(hub_row["status"], "offline");
        assert_eq!(hub_row["name"], "hub");
        assert_eq!(hub_row["kind"], Value::Null);
        assert_eq!(hub_row["stats"], Value::Null);
        assert_eq!(view["links"], json!([]));
    }

    #[test]
    fn a_mac_hub_reports_its_maker_model_and_apple_details() {
        let apple = json!({
            "chip": "Apple M2 Ultra", "p_cores": 16, "e_cores": 8, "gpu_cores": 76,
            "neural_cores": 32, "unified_memory_gib": 192,
        });
        let mac = HubReport {
            machine: Some(json!({
                "name": "studio", "kind": "workstation", "vendor": "apple", "model": "Mac Studio (M2 Ultra)", "apple": apple,
            })),
            stats: None,
        };
        let view = assemble(NOW, &mac, &[], &[]);
        let hub_row = &view["machines"][0];
        assert_eq!(hub_row["vendor"], "apple");
        assert_eq!(hub_row["model"], "Mac Studio (M2 Ultra)");
        assert_eq!(hub_row["apple"]["gpu_cores"], 76);
        assert_eq!(hub_row["apple"]["unified_memory_gib"], 192);
        assert_eq!(hub_row["stats"], Value::Null);
    }
}
