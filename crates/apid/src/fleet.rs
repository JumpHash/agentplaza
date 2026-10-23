//! Fleet routing: every call that reaches an agent's gateway (vsockd) or a machine's noded goes
//! through here. The hub's own machine is node `n0`, reached over loopback; a joined node is
//! reached through its command queue (`nodes::Store::enqueue`), which its apz-link long-polls.
//! An agent lives on the node whose CID range holds its CID.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use crate::agents::Status;
use crate::nodes::{Machine, Node, OFFLINE_AFTER_MS};
use crate::state::App;

/// The hub's own machine.
pub const LOCAL: &str = "n0";
/// How long a remote node has to answer one command.
pub const REMOTE_WAIT_MS: u64 = 30_000;

/// The node that hosts `cid`: where it was moved to, else the node whose CID range holds it, else `n0`.
pub fn node_of(app: &App, cid: u64) -> String {
    node_for(
        app.nodes.override_of(cid),
        app.nodes.owner_of_cid(cid).map(|node| node.id),
    )
}

/// The host of a CID from what the store knows of it (pure): a recorded move wins over the CID
/// ranges, and a CID in no range is on `n0`.
pub fn node_for(moved_to: Option<String>, range_owner: Option<String>) -> String {
    moved_to
        .or(range_owner)
        .unwrap_or_else(|| LOCAL.to_string())
}

/// A call to the agent gateway that serves `cid` (`/v1/agents/{cid}/send`, `/stop`, …).
pub fn gateway(
    app: &App,
    cid: u64,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), String> {
    hub_on(app, &node_of(app, cid), method, path, body)
}

/// A call to the agent gateway (vsockd) of `node`, whichever node that is. A move serves a CID on
/// its target before the placement points there, so this takes the node itself.
pub fn hub_on(
    app: &App,
    node: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), String> {
    if node == LOCAL {
        return app.hub.call(method, path, body);
    }
    remote(app, node, "hub", method, path, body)
}

/// Pushes one message to agent `cid` (`POST /v1/agents/{cid}/send` on its gateway).
pub fn send(app: &App, cid: u64, body: &Value) -> Result<(u16, Value), String> {
    gateway(
        app,
        cid,
        "POST",
        &format!("/v1/agents/{cid}/send"),
        Some(body),
    )
}

/// A call to noded on `node`.
pub fn noded(
    app: &App,
    node: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), String> {
    let answer = if node == LOCAL {
        app.noded.call(method, path, body)
    } else {
        remote(app, node, "noded", method, path, body)
    };
    // Chunk data through the hub: a relayed file read or write is payload (the transfers of plan 17.5 send none).
    if is_file_data(method, path) {
        let sent = body.map_or(0, |b| b.to_string().len());
        let got = answer
            .as_ref()
            .map_or(0, |(_, reply)| reply.to_string().len());
        crate::transfer::note_payload(sent + got);
    }
    answer
}

/// Whether a noded call moves file bytes: an export's chunk read, or an import's chunk write.
fn is_file_data(method: &str, path: &str) -> bool {
    (method == "GET" && path.starts_with("/v1/exports/"))
        || (method == "POST"
            && path.starts_with("/v1/imports/")
            && !path.ends_with("/finish")
            && path != "/v1/imports/from-transfer")
}

/// A call to the noded that runs agent `cid`'s VM.
pub fn noded_for(
    app: &App,
    cid: u64,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), String> {
    noded(app, &node_of(app, cid), method, path, body)
}

/// A fleet call's outcome as a response for the console: the upstream status and body, or the
/// status `status_of` gives the error.
pub fn respond(result: Result<(u16, Value), String>) -> Response {
    match result {
        Ok((status, body)) => Response::json(status, &body),
        Err(err) => Response::error(status_of(&err), &err),
    }
}

/// The HTTP status for a fleet error built by `remote`: 404 for an unknown node, 409 for a revoked
/// one, 503 for an offline one, 504 for one that did not answer, and 502 for anything else.
pub fn status_of(err: &str) -> u16 {
    if err.starts_with("no node ") {
        404
    } else if err.starts_with("node ") && err.contains(" did not answer ") {
        504
    } else if err.starts_with("node ") && err.ends_with(" is revoked") {
        409
    } else if err.starts_with("node ") && err.ends_with(" is offline") {
        503
    } else {
        502
    }
}

/// The error for a node the store does not know (`status_of`: 404).
fn unknown_node(node: &str) -> String {
    format!("no node {node}")
}

/// The error for a revoked node (`status_of`: 409).
fn revoked_node(node: &str) -> String {
    format!("node {node} is revoked")
}

/// The error for a node whose last heartbeat is too old (`status_of`: 503).
fn offline_node(node: &str) -> String {
    format!("node {node} is offline")
}

/// The error for a node that did not answer a queued call in time (`status_of`: 504).
fn no_answer(node: &str, method: &str, path: &str) -> String {
    format!("node {node} did not answer {method} {path}")
}

/// Queues the call on a joined node and waits for its result. Refused at once when the node is
/// unknown, revoked or offline, so nothing piles up in a queue no one reads.
fn remote(
    app: &App,
    node: &str,
    target: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value), String> {
    let Some(record) = app.nodes.get(node) else {
        return Err(unknown_node(node));
    };
    if record.revoked {
        return Err(revoked_node(node));
    }
    if apz_plaza::now_ms().saturating_sub(record.last_seen_ms) >= crate::nodes::OFFLINE_AFTER_MS {
        return Err(offline_node(node));
    }
    let id = app.nodes.enqueue(
        node,
        target,
        method,
        path,
        body.cloned().unwrap_or(Value::Null),
    );
    match app.nodes.wait_result(&id, REMOTE_WAIT_MS) {
        Some(result) => Ok((result.status, result.body)),
        None => Err(no_answer(node, method, path)),
    }
}

/// The gateway status (`/v1/status` rows) and noded runs of every node: `n0` asked directly,
/// joined nodes from their last heartbeat. Each item is `(node, status, runs)`; a node that did
/// not answer is left out.
pub fn reports(app: &App) -> Vec<(String, Value, Value)> {
    let mut all = Vec::new();
    let status = app
        .hub
        .get("/v1/status")
        .ok()
        .filter(|(code, _)| *code == 200)
        .map(|(_, body)| body);
    let runs = app
        .noded
        .get("/v1/runs")
        .ok()
        .filter(|(code, _)| *code == 200)
        .map(|(_, body)| body);
    if status.is_some() || runs.is_some() {
        all.push((
            LOCAL.to_string(),
            status.unwrap_or(Value::Null),
            runs.unwrap_or(Value::Null),
        ));
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
/// noded did not answer; and whether the owner stopped `n0` (`mover`).
pub struct HubReport {
    pub machine: Option<Value>,
    pub stats: Option<Value>,
    pub stopped: bool,
}

/// The fleet for the Machines view (`GET /v1/fleet`): the hub's own machine (`n0`, from its
/// noded) and every joined node (from its last heartbeat), each with the agents it runs.
pub fn fleet(app: &App) -> Value {
    // The latest probe round (probes.rs), not a live call per request.
    let probed = crate::probes::latest(app);
    let hub = HubReport {
        machine: probed.machine,
        stats: probed.stats,
        stopped: app.nodes.is_stopped(LOCAL),
    };
    // Agents that are offline are not running anywhere, so they are not placed.
    let placed: Vec<Placed> = app
        .agents
        .list()
        .into_iter()
        .filter(|agent| agent.status != Status::Offline)
        .map(|agent| Placed {
            cid: agent.cid,
            node: node_of(app, agent.cid),
            working: agent.status == Status::Working,
        })
        .collect();
    let now = apz_plaza::now_ms();
    let mut view = assemble(now, &hub, &app.nodes.list(), &placed);
    // The enrolled workstations, from their views as the registry serialises them.
    let stations = serde_json::to_value(app.stations.views(now)).unwrap_or(Value::Null);
    let (machines, links) = station_rows(now, &stations);
    if let Some(list) = view["machines"].as_array_mut() {
        list.extend(machines);
    }
    if let Some(list) = view["links"].as_array_mut() {
        list.extend(links);
    }
    view
}

/// The enrolled workstations as fleet machines and links (pure), from `stations`: the station views as
/// the registry serialises them (a JSON array, `StationView` in `stations::registry`, not nameable here).
/// A revoked station is left out. Each station is one `kind: "workstation"` row whose `agents` are the
/// agents of its opening or active leases, and one link to `n0`, online when its link is.
pub fn station_rows(_now_ms: u64, stations: &Value) -> (Vec<Value>, Vec<Value>) {
    let mut machines = Vec::new();
    let mut links = Vec::new();
    for station in stations.as_array().into_iter().flatten() {
        if station["status"] == "revoked" {
            continue;
        }
        let facts = &station["facts"];
        let os = facts["os"].clone();
        let form = facts["form"].clone();
        let cpus = facts["cpus"].clone();
        let vendor = (os == "macos").then_some("apple");
        let leases = station["leases"].as_array().cloned().unwrap_or_default();
        let mut agents: Vec<Value> = Vec::new();
        let mut active = 0_usize;
        for lease in &leases {
            let state = lease["state"].as_str();
            if state == Some("active") {
                active += 1;
            }
            if matches!(state, Some("opening" | "active")) && !agents.contains(&lease["agent"]) {
                agents.push(lease["agent"].clone());
            }
        }
        let gpus: Vec<Value> = facts["gpus"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|gpu| {
                let vram = gpu["vram_mb"].as_u64().unwrap_or(0);
                let memory = (vram > 0).then_some(vram);
                json!({ "name": gpu["name"], "vendor": gpu["vendor"], "memory_mib": memory })
            })
            .collect();
        let account = &facts["account"];
        machines.push(json!({
            "id": station["id"],
            "name": station["name"],
            "role": "station",
            "kind": "workstation",
            "vendor": vendor,
            "model": null,
            "apple": null,
            "status": station["status"],
            "arch": facts["arch"],
            "os": os,
            "backend": "",
            "cpu": { "model": "", "cores": cpus, "threads": cpus },
            "memory_mib": facts["ram_mb"],
            "gpus": gpus,
            "storage_gib": 0,
            "agents": agents,
            "working": active,
            "stats": null,
            "last_seen_ms": station["last_seen_ms"],
            "station": {
                "form": form,
                "os": os,
                "leases": leases.len(),
                "active": active,
                "account": account["mode"],
                "account_name": account["name"],
            },
        }));
        links.push(json!({ "a": LOCAL, "b": station["id"], "online": station["online"] }));
    }
    (machines, links)
}

/// The fleet JSON from its facts (pure): `machines` (the hub first, then every node in order)
/// and `links` (one per node, to `n0`). A node is reachable when it is not revoked and was
/// seen within `OFFLINE_AFTER_MS`. Status: revoked, else draining (stopped by the owner, `n0` too),
/// else online or offline.
/// The hub's stats come from its noded; a remote node's come from its last heartbeat (`null` until
/// one carries them), since its noded is reached only through the command queue.
pub fn assemble(now_ms: u64, hub: &HubReport, nodes: &[Node], placed: &[Placed]) -> Value {
    let hub_machine: Machine = hub
        .machine
        .clone()
        .and_then(|view| serde_json::from_value(view).ok())
        .unwrap_or_default();
    let hub_status = if hub.stopped {
        "draining"
    } else if hub.machine.is_some() {
        "online"
    } else {
        "offline"
    };
    let hub_name = hub_machine
        .name
        .clone()
        .unwrap_or_else(|| "hub".to_string());
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
        let reachable =
            !node.revoked && now_ms.saturating_sub(node.last_seen_ms) < OFFLINE_AFTER_MS;
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
        let stats = json!(node.machine.stats);
        machines.push(machine_row(
            &node.id,
            &node.name,
            "node",
            status,
            &node.machine,
            placed,
            stats,
            node.last_seen_ms,
        ));
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
    machine: &Machine,
    placed: &[Placed],
    stats: Value,
    last_seen_ms: u64,
) -> Value {
    let here: Vec<&Placed> = placed.iter().filter(|agent| agent.node == id).collect();
    json!({
        "id": id,
        "name": name,
        "role": role,
        "kind": machine.kind,
        "vendor": machine.vendor,
        "model": machine.model,
        "apple": machine.apple,
        "status": status,
        "arch": machine.arch,
        "os": machine.os,
        "backend": machine.backend,
        "cpu": machine.cpu,
        "memory_mib": machine.memory_mib,
        "gpus": machine.gpus,
        "storage_gib": machine.storage_gib,
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
        .doc(RouteDoc::new("List the machines and the agents on them"))
        .get("/v1/nodes/{id}/machine", |app, _, p| {
            respond(noded(app, p.get("id"), "GET", "/v1/machine", None))
        })
        .doc(RouteDoc::new("Read a machine's summary").errors(&[502]))
        .get("/v1/nodes/{id}/stats", |app, _, p| {
            respond(noded(app, p.get("id"), "GET", "/v1/machine/stats", None))
        })
        .doc(RouteDoc::new("Read a machine's live stats").errors(&[502]))
        .get("/v1/nodes/{id}/runs", |app, _, p| {
            respond(noded(app, p.get("id"), "GET", "/v1/runs", None))
        })
        .doc(RouteDoc::new("List a machine's runs").errors(&[502]))
        .post("/v1/nodes/{id}/runs", spawn_on)
        .doc(RouteDoc::new("Start a run on a machine").errors(&[400, 502]))
}

/// `POST /v1/nodes/{id}/runs`: a raw run on that node (the console's machine view).
fn spawn_on(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Value = if req.body.is_empty() {
        json!({})
    } else {
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

    fn node(
        id: &str,
        name: &str,
        last_seen_ms: u64,
        machine: Value,
        draining: bool,
        revoked: bool,
    ) -> Node {
        Node {
            id: id.into(),
            name: name.into(),
            public_key: String::new(),
            cid_base: 1000,
            cid_count: 100,
            joined_ms: 0,
            last_seen_ms,
            machine: serde_json::from_value(machine).unwrap_or_default(),
            draining,
            revoked,
            address: String::new(),
        }
    }

    fn placed(cid: u64, node: &str, working: bool) -> Placed {
        Placed {
            cid,
            node: node.into(),
            working,
        }
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
            stats: Some(
                json!({ "cpu_percent": 7.5, "mem_used_mib": 18000, "load": [0.1, 0.2, 0.3] }),
            ),
            stopped: false,
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
            node(
                "n4",
                "old-laptop",
                NOW - 5_000,
                json!({ "kind": "laptop" }),
                true,
                false,
            ),
            node(
                "n5",
                "silent",
                NOW - OFFLINE_AFTER_MS - 1,
                json!({}),
                false,
                false,
            ),
            node("n6", "banned", NOW, json!({}), false, true),
        ];
        let agents = [
            placed(200, "n0", true),
            placed(201, "n0", false),
            placed(1000, "n3", false),
        ];
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
        assert_eq!(
            hub_row["stats"],
            json!({ "cpu_pct": 7.5, "mem_used_mib": 18000 })
        );
        assert_eq!(hub_row["cpu"]["threads"], 24);
        assert_eq!(hub_row["gpus"][0]["vendor"], "NVIDIA");
        assert_eq!(hub_row["vendor"], Value::Null);
        assert_eq!(hub_row["model"], Value::Null);
        assert_eq!(hub_row["apple"], Value::Null);

        let statuses: Vec<(&str, &str)> = machines[1..]
            .iter()
            .map(|row| (row["id"].as_str().unwrap(), row["status"].as_str().unwrap()))
            .collect();
        assert_eq!(
            statuses,
            [
                ("n3", "online"),
                ("n4", "draining"),
                ("n5", "offline"),
                ("n6", "revoked")
            ]
        );

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
        let online: Vec<(&str, bool)> = links
            .iter()
            .map(|l| (l["b"].as_str().unwrap(), l["online"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            online,
            [("n3", true), ("n4", true), ("n5", false), ("n6", false)]
        );
        assert!(links.iter().all(|l| l["a"] == "n0"));
    }

    #[test]
    fn a_hub_whose_noded_is_down_is_offline_without_stats() {
        let down = HubReport {
            machine: None,
            stats: None,
            stopped: false,
        };
        let view = assemble(NOW, &down, &[], &[]);
        let hub_row = &view["machines"][0];
        assert_eq!(hub_row["status"], "offline");
        let stopped = HubReport {
            stopped: true,
            ..hub()
        };
        assert_eq!(
            assemble(NOW, &stopped, &[], &[])["machines"][0]["status"],
            "draining",
            "a stopped hub machine is out of service like a stopped node"
        );
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
            stopped: false,
        };
        let view = assemble(NOW, &mac, &[], &[]);
        let hub_row = &view["machines"][0];
        assert_eq!(hub_row["vendor"], "apple");
        assert_eq!(hub_row["model"], "Mac Studio (M2 Ultra)");
        assert_eq!(hub_row["apple"]["gpu_cores"], 76);
        assert_eq!(hub_row["apple"]["unified_memory_gib"], 192);
        assert_eq!(hub_row["stats"], Value::Null);
    }

    /// A station's view as the registry serialises it, with the given leases (JSON).
    fn station_json(id: &str, status: &str, leases: Value) -> Value {
        let online = status == "online";
        json!({
            "id": id, "name": "front", "status": status, "online": online, "draining": false,
            "revoked": status == "revoked", "version": "0.1.0", "code": "j1", "joined_ms": 0,
            "last_seen_ms": NOW - 2_000, "address": "",
            "facts": {
                "os": "macos", "os_version": "15.0", "arch": "arm64", "hostname": "front",
                "form": "laptop", "cpus": 10, "ram_mb": 32768,
                "gpus": [{ "vendor": "apple", "name": "Apple M3", "vram_mb": 0 }],
                "displays": [], "account": { "mode": "owner", "name": "zarathos" },
                "apps": 12, "virtual_displays": false, "ui_tree": true, "machine_id": null,
            },
            "settings": {}, "leases": leases,
        })
    }

    /// A lease view as the registry serialises it (its lease fields, flattened, and its queue position).
    fn lease_json(agent: u64, state: &str) -> Value {
        json!({
            "id": format!("l{agent}"), "station": "w1", "agent": agent, "task": null, "kind": "pixel",
            "session": null, "started_ms": NOW, "state": state, "paused": false, "active_ms": null,
            "position": null,
        })
    }

    #[test]
    fn a_station_is_a_workstation_row_with_its_active_agents_and_a_link() {
        let stations = json!([
            station_json(
                "w1",
                "online",
                json!([lease_json(300, "active"), lease_json(301, "queued")])
            ),
            station_json("w2", "revoked", json!([])),
        ]);
        let (machines, links) = station_rows(NOW, &stations);
        assert_eq!(machines.len(), 1, "a revoked station is left out");
        let row = &machines[0];
        assert_eq!(row["id"], "w1");
        assert_eq!(row["role"], "station");
        assert_eq!(row["kind"], "workstation");
        assert_eq!(row["vendor"], "apple");
        assert_eq!(row["os"], "macos");
        assert_eq!(row["status"], "online");
        assert_eq!(row["working"], 1);
        assert_eq!(
            row["agents"],
            json!([300]),
            "the queued lease's agent is not running"
        );
        assert_eq!(row["memory_mib"], 32768);
        assert_eq!(
            row["gpus"],
            json!([{ "name": "Apple M3", "vendor": "apple", "memory_mib": null }])
        );
        assert_eq!(row["station"]["form"], "laptop");
        assert_eq!(row["station"]["leases"], 2);
        assert_eq!(row["station"]["active"], 1);
        assert_eq!(row["station"]["account"], "owner");
        assert_eq!(row["station"]["account_name"], "zarathos");
        assert_eq!(
            Value::Array(links),
            json!([{ "a": "n0", "b": "w1", "online": true }])
        );
    }

    #[test]
    fn a_station_without_leases_is_idle_and_offline_in_its_link() {
        let stations = json!([station_json("w3", "offline", json!([]))]);
        let (machines, links) = station_rows(NOW, &stations);
        assert_eq!(machines[0]["agents"], json!([]));
        assert_eq!(machines[0]["working"], 0);
        assert_eq!(links[0]["online"], false);
    }

    #[test]
    fn fleet_errors_map_to_the_status_the_console_needs() {
        assert_eq!(unknown_node("n9"), "no node n9");
        assert_eq!(revoked_node("n3"), "node n3 is revoked");
        assert_eq!(offline_node("n3"), "node n3 is offline");
        assert_eq!(
            no_answer("n3", "POST", "/v1/runs"),
            "node n3 did not answer POST /v1/runs"
        );
        assert_eq!(status_of(&unknown_node("n9")), 404);
        assert_eq!(status_of(&revoked_node("n3")), 409);
        assert_eq!(status_of(&offline_node("n3")), 503);
        assert_eq!(status_of(&no_answer("n3", "POST", "/v1/runs")), 504);
        assert_eq!(status_of("connection refused"), 502);
    }
}
