//! Where agents run: the node a new agent is placed on, the move of an agent to another node, and the
//! drain of a node (every agent it runs is moved off, one at a time).
//!
//! A move carries the agent's VM between two noded. The VM is put to sleep (`sleep::put_to_sleep`),
//! its files are exported on the source (`POST /v1/runs/{run}/vms/{vm}/export`), copied in
//! [`CHUNK`]-byte pieces to an import on the target, and finished there into a sleeping run. Then the
//! placement is recorded (`nodes::Store::set_override`, saved with the node store), the target's
//! gateway serves the CID, the agent wakes on the target, and the source lets go of it (its gateway
//! and its run). Any failure before the placement is recorded aborts the import and wakes the agent
//! where it was.
//!
//! Routes (owner):
//!   POST /v1/agents/{cid}/move  {node} → {ok, from, to, bytes, ms}

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use apz_http::{Body, Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::Status;
use crate::fleet::{self, LOCAL};
use crate::nodes::{Node, OFFLINE_AFTER_MS};
use crate::sleep;
use crate::state::App;

/// Memory an agent's VM is spawned with (the payload `agents::spawn_agents` sends noded).
pub const AGENT_MIB: u64 = 1024;
/// A warm template for the role saves a cold boot: it counts as this much free memory when placing.
pub const WARM_BONUS_MIB: i64 = 1024;
/// Each VM a node already runs counts this much against it when placing, so new agents spread out
/// over the nodes instead of filling one.
pub const VM_PENALTY_MIB: i64 = 512;
/// Bytes per export read (noded serves at most 4 MiB at a time).
pub const CHUNK: u64 = 4 * 1024 * 1024;
/// Decoded bytes per import write: noded takes at most 512 KiB per call (its body limit is 1 MiB),
/// so each read of [`CHUNK`] is written in pieces of this size.
pub const WRITE_CHUNK: usize = 512 * 1024;
/// How long a move waits for the agent to be back after its wake.
const WAKE_WAIT: Duration = Duration::from_secs(60);

/// One node's standing for a placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub node: String,
    /// Memory not in use now, in MiB.
    pub free_mib: u64,
    /// VMs running on it now.
    pub running_vms: u64,
    /// It holds a warm template for the role.
    pub warm: bool,
}

/// A candidate's score; the higher wins. Free memory, less the VMs it runs, plus the warm bonus.
pub fn score(candidate: &Candidate) -> i64 {
    let free = i64::try_from(candidate.free_mib).unwrap_or(i64::MAX);
    let vms = i64::try_from(candidate.running_vms).unwrap_or(i64::MAX);
    let bonus = if candidate.warm { WARM_BONUS_MIB } else { 0 };
    free - vms.saturating_mul(VM_PENALTY_MIB) + bonus
}

/// The node a new agent goes to, from the candidates (pure). Only a node with room for one agent is
/// eligible. The highest score wins; a tie goes to `n0`, then to the earlier candidate. With no
/// eligible candidate the agent goes to `n0`, where noded says whether it fits.
pub fn choose(candidates: &[Candidate]) -> String {
    let mut best: Option<(i64, &Candidate)> = None;
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.free_mib >= AGENT_MIB)
    {
        let s = score(candidate);
        let better = match best {
            None => true,
            Some((top, held)) => {
                s > top || (s == top && candidate.node == LOCAL && held.node != LOCAL)
            }
        };
        if better {
            best = Some((s, candidate));
        }
    }
    best.map_or_else(
        || LOCAL.to_string(),
        |(_, candidate)| candidate.node.clone(),
    )
}

/// The node a new agent of `role` goes to: among the nodes that report and are not draining (`n0`
/// included), the best by [`choose`]. A node whose machine stats did not answer is left out.
pub fn choose_node(app: &App, role: &str) -> String {
    let draining: BTreeSet<String> = app
        .nodes
        .list()
        .into_iter()
        .filter(|node| node.draining)
        .map(|node| node.id)
        .collect();
    let candidates: Vec<Candidate> = fleet::reports(app)
        .into_iter()
        .filter(|(node, _, _)| !draining.contains(node))
        .filter_map(|(node, _status, runs)| {
            let (total, used) = memory_of(app, &node)?;
            Some(Candidate {
                free_mib: total.saturating_sub(used),
                running_vms: running_vms(&runs),
                warm: warm_for(&templates_of(app, &node), role),
                node,
            })
        })
        .collect();
    choose(&candidates)
}

/// A node's memory as (total, used) in MiB: `n0`'s stats asked now, a joined node's from its last
/// heartbeat. None when the stats did not answer.
fn memory_of(app: &App, node: &str) -> Option<(u64, u64)> {
    let (stats, machine_total) = if node == LOCAL {
        let Ok((200, stats)) = fleet::noded(app, LOCAL, "GET", "/v1/machine/stats", None) else {
            return None;
        };
        (stats, None)
    } else {
        let machine = app.nodes.get(node)?.machine;
        (
            machine.get("stats")?.clone(),
            machine["memory_mib"].as_u64(),
        )
    };
    let used = stats["mem_used_mib"].as_u64()?;
    let total = stats["mem_total_mib"].as_u64().or(machine_total)?;
    Some((total, used))
}

/// The VMs running in a node's runs (its `GET /v1/runs`): every VM of a run that has not ended.
fn running_vms(runs: &Value) -> u64 {
    runs["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|run| run["ended_at"].is_null())
        .map(|run| u64::try_from(run["vms"].as_array().map_or(0, Vec::len)).unwrap_or(u64::MAX))
        .sum()
}

/// A node's template list (its `GET /v1/templates`); Null when it did not answer.
fn templates_of(app: &App, node: &str) -> Value {
    match fleet::noded(app, node, "GET", "/v1/templates", None) {
        Ok((200, body)) => body,
        _ => Value::Null,
    }
}

/// Whether a template list (an array, or `{templates: [...]}`) holds one for `role`.
fn warm_for(templates: &Value, role: &str) -> bool {
    let list = templates
        .as_array()
        .or_else(|| templates["templates"].as_array());
    list.is_some_and(|items| items.iter().any(|item| item["role"].as_str() == Some(role)))
}

/// Why a move to `target` is refused (pure): the agent is already on it, or the node is unknown,
/// revoked, draining or offline. `n0` is always there.
pub fn target_refusal(
    from: &str,
    target: &str,
    record: Option<&Node>,
    now_ms: u64,
) -> Option<(u16, String)> {
    if target == from {
        return Some((409, format!("agent is already on {target}")));
    }
    if target == LOCAL {
        return None;
    }
    let Some(node) = record else {
        return Some((404, format!("no node {target}")));
    };
    if node.revoked {
        return Some((409, format!("node {target} is revoked")));
    }
    if node.draining {
        return Some((409, format!("node {target} is draining")));
    }
    if now_ms.saturating_sub(node.last_seen_ms) >= OFFLINE_AFTER_MS {
        return Some((409, format!("node {target} is offline")));
    }
    None
}

/// Why an agent in `status` cannot move now (pure): a working agent moves when it is idle, and an
/// offline one has no running VM to carry.
pub fn status_refusal(cid: u64, status: Status) -> Option<(u16, String)> {
    match status {
        Status::Working => Some((
            409,
            format!("agent {cid} is working; move it when it is idle"),
        )),
        Status::Offline => Some((
            409,
            format!("agent {cid} is offline; it has no running VM to move"),
        )),
        Status::Idle | Status::Asleep => None,
    }
}

/// The agents whose move is in flight. A move holds its agent here, so a second move of it is refused.
static MOVING: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

/// Whether a move of agent `cid` is in flight.
pub fn is_moving(cid: u64) -> bool {
    MOVING.lock().is_ok_and(|set| set.contains(&cid))
}

/// Holds the agent's move slot; dropping it frees the slot whatever the move's outcome.
struct Claim(u64);

impl Claim {
    fn take(cid: u64) -> Option<Claim> {
        let mut set = MOVING.lock().unwrap_or_else(PoisonError::into_inner);
        // Lazily: an eager `then_some(Claim(cid))` would build a claim even when the slot is taken,
        // and dropping it here would lock MOVING again (and free the other move's slot).
        set.insert(cid).then(|| Claim(cid))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        MOVING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// What a move did: where the agent came from and went to, the bytes carried, and how long it took.
#[derive(Debug)]
pub struct Moved {
    pub from: String,
    pub to: String,
    pub bytes: u64,
    pub ms: u64,
}

/// One file of an export or import, as the node lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub len: u64,
    #[serde(default)]
    pub sha256: String,
}

/// One read from the source node: the chunk's bytes (base64, standard alphabet as noded writes it),
/// whether it reached the end of the file, and, for a chunk of zeros, how many (with `data` empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub data: String,
    pub eof: bool,
    pub zeros: Option<u64>,
}

/// Copies every part from the source to the target in pieces of at most `chunk` bytes. `fetch(name,
/// offset, len)` reads a piece on the source: its base64 data and whether it ends the file. `push(name,
/// offset, data)` writes a piece on the target. Each piece must decode to exactly the length asked
/// for, and the source must say eof at the end of a file and nowhere else. A part of length 0 is pushed
/// once with no data, so the target creates it. Returns the bytes copied.
pub fn copy_files<F, P>(
    chunk: u64,
    parts: &[Part],
    mut fetch: F,
    mut push: P,
) -> Result<u64, String>
where
    F: FnMut(&str, u64, u64) -> Result<Fetched, String>,
    P: FnMut(&str, u64, &str) -> Result<(), String>,
{
    if chunk == 0 {
        return Err("the chunk size must be at least one byte".into());
    }
    let mut copied = 0;
    for part in parts {
        if part.len == 0 {
            push(&part.name, 0, "")?;
            continue;
        }
        let mut offset = 0;
        while offset < part.len {
            let len = (part.len - offset).min(chunk);
            let Fetched { data, eof, zeros } = fetch(&part.name, offset, len)?;
            let decoded = match zeros {
                // A run of zeros is not sent: the target staged the file as a hole of its length.
                Some(zeros) if zeros == len && data.is_empty() => {
                    let last = offset + len == part.len;
                    if eof != last {
                        return Err(format!(
                            "{}: the source says eof {eof} at byte {}, of {}",
                            part.name,
                            offset + len,
                            part.len
                        ));
                    }
                    offset += len;
                    copied += len;
                    continue;
                }
                Some(zeros) => {
                    return Err(format!(
                        "{}: the piece at {offset} claims {zeros} zero bytes, not {len}",
                        part.name
                    ));
                }
                None => {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD
                        .decode(&data)
                        .map_err(|e| {
                            format!("{}: the piece at {offset} is not base64: {e}", part.name)
                        })?
                }
            };
            if u64::try_from(decoded.len()).unwrap_or(u64::MAX) != len {
                return Err(format!(
                    "{}: the piece at {offset} is {} bytes, not {len}",
                    part.name,
                    decoded.len()
                ));
            }
            let last = offset + len == part.len;
            if eof != last {
                return Err(format!(
                    "{}: the source says eof {eof} at byte {}, of {}",
                    part.name,
                    offset + len,
                    part.len
                ));
            }
            // The read is written in pieces of at most WRITE_CHUNK decoded bytes.
            for (index, piece) in decoded.chunks(WRITE_CHUNK).enumerate() {
                let at = offset + (index * WRITE_CHUNK) as u64;
                push(&part.name, at, &{
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(piece)
                })?;
            }
            offset += len;
            copied += len;
        }
    }
    Ok(copied)
}

/// The record of what a move has taken so far, so an abort can undo exactly that.
#[derive(Default)]
struct Staged {
    /// The export on the source; deleted when the move ends, whatever happened.
    export: Option<String>,
    /// The import on the target, until it finishes.
    import: Option<String>,
    /// The sleeping run the finished import made on the target.
    target_run: Option<String>,
    /// The placement before this move, once this move recorded its own.
    overridden: Option<Option<String>>,
    /// The target's gateway was asked to serve the CID (it may be serving it).
    served: bool,
}

/// Moves agent `cid` to node `target` (see the module doc). The agent is asleep when the copy starts,
/// and awake on the target when this returns Ok. Errors are the response for the caller.
pub fn move_agent(app: &App, cid: u64, target: &str) -> Result<Moved, Response> {
    let started = Instant::now();
    let Some(agent) = app.agents.get(cid) else {
        return Err(Response::error(404, &format!("no agent {cid}")));
    };
    let from = fleet::node_of(app, cid);
    let record = app.nodes.get(target);
    if let Some((status, message)) =
        target_refusal(&from, target, record.as_ref(), apz_plaza::now_ms())
    {
        return Err(Response::error(status, &message));
    }
    if let Some((status, message)) = status_refusal(cid, agent.status) {
        return Err(Response::error(status, &message));
    }
    if app.work.current(cid).is_some() || !app.work.queued(cid).is_empty() {
        return Err(Response::error(
            409,
            &format!("agent {cid} has work in flight or queued"),
        ));
    }
    let (run, vm) = crate::lifecycle::place(app, cid)?;
    let Some(_claim) = Claim::take(cid) else {
        return Err(Response::error(
            409,
            &format!("agent {cid} is already moving"),
        ));
    };
    let was_asleep = agent.status == Status::Asleep;
    if !was_asleep {
        sleep::put_to_sleep(app, cid)?;
    }
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "move_started",
        json!({"from": from, "to": target}),
    );

    let mut staged = Staged::default();
    let carried = carry(app, cid, &from, target, &run, vm, &mut staged);
    if let Some(export) = staged.export.take() {
        log_failure(
            "delete the export",
            call(app, &from, "DELETE", &format!("/v1/exports/{export}"), None),
        );
    }
    let (bytes, new_run) = match carried {
        Ok(carried) => carried,
        Err(message) => {
            abort(app, cid, target, &staged, was_asleep);
            return Err(Response::error(
                502,
                &format!("move to {target} failed: {message}"),
            ));
        }
    };

    // The agent's run and VM are the target's now; it wakes there.
    app.agents.update(cid, |slot| {
        slot.run = Some(new_run.clone());
        slot.vm = Some(0);
    });
    let woke = sleep::ensure_awake(app, cid, WAKE_WAIT);

    // The source lets go of the agent: its gateway stops serving the CID, and its run goes.
    log_failure(
        "stop serving on the source",
        gateway_call_on(app, &from, "DELETE", &format!("/v1/cids/{cid}"), None),
    );
    log_failure(
        "delete the source run",
        call(app, &from, "DELETE", &format!("/v1/runs/{run}"), None),
    );
    if let Some(agent) = app.agents.get(cid) {
        app.bus.publish("agent", &agent);
    }
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if let Err(err) = woke {
        return Err(Response::error(
            502,
            &format!("moved to {target} but the agent did not wake there: {err}"),
        ));
    }
    crate::metrics::ingest::record_event(
        &app.metrics,
        apz_metrics::Event {
            t_ms: apz_plaza::now_ms(),
            kind: "move".to_string(),
            entity_kind: apz_metrics::EntityKind::Agent,
            entity: cid.to_string(),
            duration_ms: Some(ms as f64),
            detail: format!("moved {from} → {target} in {ms} ms"),
            parts: Vec::new(),
        },
        target,
    );
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "moved",
        json!({"from": from, "to": target, "bytes": bytes, "ms": ms}),
    );
    Ok(Moved {
        from,
        to: target.to_string(),
        bytes,
        ms,
    })
}

/// The copy: export the VM's files on `from`, import them on `target`, copy every file in pieces,
/// finish the import into a sleeping run, record the placement, and have the target's gateway serve
/// the CID. Returns the bytes copied and the target's run. What it takes goes into `staged`.
fn carry(
    app: &App,
    cid: u64,
    from: &str,
    target: &str,
    run: &str,
    vm: u32,
    staged: &mut Staged,
) -> Result<(u64, String), String> {
    let exported = call(
        app,
        from,
        "POST",
        &format!("/v1/runs/{run}/vms/{vm}/export"),
        Some(&json!({})),
    )?;
    let export = text(&exported, "export")?;
    staged.export = Some(export.clone());
    let parts: Vec<Part> = serde_json::from_value(exported["files"].clone())
        .map_err(|e| format!("the export's files: {e}"))?;
    let imported = call(
        app,
        target,
        "POST",
        "/v1/imports",
        Some(&json!({"meta": exported["meta"], "files": parts})),
    )?;
    let import = text(&imported, "import")?;
    staged.import = Some(import.clone());
    let bytes = copy_files(
        CHUNK,
        &parts,
        |name, offset, len| fetch_piece(app, from, &export, name, offset, len),
        |name, offset, data| push_piece(app, target, &import, name, offset, data),
    )?;
    let finished = call(
        app,
        target,
        "POST",
        &format!("/v1/imports/{import}/finish"),
        Some(&json!({})),
    )?;
    staged.import = None;
    let new_run = text(&finished, "run")?;
    staged.target_run = Some(new_run.clone());
    if finished["cid"].as_u64() != Some(cid) {
        return Err(format!(
            "the import is for CID {}, not {cid}",
            finished["cid"]
        ));
    }
    // The placement is recorded and saved before anything routes by it.
    staged.overridden = Some(app.nodes.set_override(cid, Some(target)));
    save_placements(app)?;
    staged.served = true;
    gateway_call_on(
        app,
        target,
        "POST",
        &format!("/v1/cids/{cid}"),
        Some(&json!({})),
    )?;
    Ok((bytes, new_run))
}

/// Undoes what a failed move took: the placement goes back (and is saved so), the target drops the
/// copy and the CID, and an agent that was awake before the move is woken where it is.
fn abort(app: &App, cid: u64, target: &str, staged: &Staged, was_asleep: bool) {
    if let Some(previous) = &staged.overridden {
        app.nodes.set_override(cid, previous.as_deref());
        log_failure("save the placement back", save_placements(app));
    }
    if let Some(import) = &staged.import {
        log_failure(
            "delete the import",
            call(
                app,
                target,
                "DELETE",
                &format!("/v1/imports/{import}"),
                None,
            ),
        );
    }
    if staged.served {
        log_failure(
            "stop serving on the target",
            gateway_call_on(app, target, "DELETE", &format!("/v1/cids/{cid}"), None),
        );
    }
    if let Some(run) = &staged.target_run {
        log_failure(
            "delete the target run",
            call(app, target, "DELETE", &format!("/v1/runs/{run}"), None),
        );
    }
    if !was_asleep {
        if let Err(err) = sleep::ensure_awake(app, cid, WAKE_WAIT) {
            eprintln!("apid: move of agent {cid} aborted; it did not wake again: {err}");
        }
    }
}

/// Reads one piece of an export on `node`: its base64 data and whether it ends the file.
fn fetch_piece(
    app: &App,
    node: &str,
    export: &str,
    name: &str,
    offset: u64,
    len: u64,
) -> Result<Fetched, String> {
    let body = call(
        app,
        node,
        "GET",
        &format!("/v1/exports/{export}/{name}?offset={offset}&len={len}"),
        None,
    )?;
    let data = body["data"]
        .as_str()
        .ok_or("the export's piece has no data")?
        .to_string();
    Ok(Fetched {
        data,
        eof: body["eof"].as_bool().unwrap_or(false),
        zeros: body["zeros"].as_u64(),
    })
}

/// Writes one piece of an import on `node`.
fn push_piece(
    app: &App,
    node: &str,
    import: &str,
    name: &str,
    offset: u64,
    data: &str,
) -> Result<(), String> {
    call(
        app,
        node,
        "POST",
        &format!("/v1/imports/{import}/{name}"),
        Some(&json!({"offset": offset, "data": data})),
    )
    .map(|_| ())
}

/// Saves the node store, placements included, now (the saver would do it on its next pass).
fn save_placements(app: &App) -> Result<(), String> {
    app.db
        .store("nodes", &crate::db::snapshot_of(app, "nodes"))
        .map(|_| ())
}

/// A node's answer as its body when it is 2xx; otherwise the failure as text.
fn answered(what: &str, result: Result<(u16, Value), String>) -> Result<Value, String> {
    match result {
        Ok((code, body)) if (200..300).contains(&code) => Ok(body),
        Ok((code, body)) => Err(format!("{what} answered {code}: {body}")),
        Err(err) => Err(format!("{what}: {err}")),
    }
}

/// A noded call on `node`, answered as [`answered`].
fn call(
    app: &App,
    node: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    answered(
        &format!("{node} {method} {path}"),
        fleet::noded(app, node, method, path, body),
    )
}

/// A gateway (vsockd) call on `node`, answered as [`answered`].
fn gateway_call_on(
    app: &App,
    node: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    answered(
        &format!("{node} gateway {method} {path}"),
        fleet::hub_on(app, node, method, path, body),
    )
}

/// A field of a reply, as text.
fn text(body: &Value, key: &str) -> Result<String, String> {
    body[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("no {key} in the reply"))
}

/// Logs a clean-up step that failed; the move's outcome does not turn on it.
fn log_failure(what: &str, result: Result<impl Sized, String>) {
    if let Err(err) = result {
        eprintln!("apid: move: could not {what}: {err}");
    }
}

/// The text of an error response, for a log line.
fn body_text(resp: &Response) -> String {
    match &resp.body {
        Body::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        _ => String::new(),
    }
}

/// Drains `node` (it was just set draining): on a background thread, each agent it runs is moved to
/// the node [`choose_node`] picks for its role, one at a time. A move that fails is logged and the
/// agent stays where it is.
pub fn spawn_drain(app: &App, node: &str) {
    let Some(hub) = app.handle().upgrade() else {
        eprintln!("apid: cannot drain {node}: the hub is not running");
        return;
    };
    let node = node.to_string();
    std::thread::spawn(move || {
        for agent in hub.agents.list() {
            if fleet::node_of(&hub, agent.cid) != node || agent.status == Status::Offline {
                continue;
            }
            let target = choose_node(&hub, &agent.role);
            match move_agent(&hub, agent.cid, &target) {
                Ok(moved) => eprintln!(
                    "apid: drain of {node}: agent {} moved to {} ({} bytes, {} ms)",
                    agent.cid, moved.to, moved.bytes, moved.ms
                ),
                Err(resp) => eprintln!(
                    "apid: drain of {node}: agent {} stays: {}",
                    agent.cid,
                    body_text(&resp)
                ),
            }
        }
    });
}

/// Routes: `POST /v1/agents/{cid}/move`.
pub fn routes(router: Router<App>) -> Router<App> {
    router.post("/v1/agents/{cid}/move", move_route)
}

#[derive(Deserialize)]
struct MoveBody {
    node: String,
}

/// `POST /v1/agents/{cid}/move {node}`: the owner moves an agent to another node.
fn move_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let agent = match crate::agents::agent_at(app, p) {
        Ok(agent) => agent,
        Err(res) => return res,
    };
    let body: MoveBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    match move_agent(app, agent.cid, &body.node) {
        Ok(moved) => Response::json(
            200,
            &json!({"ok": true, "from": moved.from, "to": moved.to, "bytes": moved.bytes, "ms": moved.ms}),
        ),
        Err(res) => res,
    }
}

#[cfg(test)]
mod tests {
    use crate::fleet::node_for;

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn unb64(text: &str) -> Result<Vec<u8>, base64::DecodeError> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(text)
    }

    use super::*;

    fn candidate(node: &str, free_mib: u64, running_vms: u64, warm: bool) -> Candidate {
        Candidate {
            node: node.to_string(),
            free_mib,
            running_vms,
            warm,
        }
    }

    fn node_record(id: &str, last_seen_ms: u64, draining: bool, revoked: bool) -> Node {
        Node {
            id: id.to_string(),
            name: id.to_string(),
            public_key: String::new(),
            cid_base: 1000,
            cid_count: 100,
            joined_ms: 0,
            last_seen_ms,
            machine: Value::Null,
            draining,
            revoked,
            address: String::new(),
        }
    }

    #[test]
    fn the_node_with_the_most_free_memory_takes_a_new_agent() {
        let picked = choose(&[
            candidate("n0", 2048, 0, false),
            candidate("n1", 8192, 0, false),
        ]);
        assert_eq!(picked, "n1");
    }

    #[test]
    fn a_warm_template_outweighs_a_little_more_memory() {
        let picked = choose(&[
            candidate("n0", 3000, 0, true),
            candidate("n1", 3500, 0, false),
        ]);
        assert_eq!(picked, "n0");
        let picked = choose(&[
            candidate("n0", 3000, 0, false),
            candidate("n1", 3500, 0, false),
        ]);
        assert_eq!(picked, "n1", "without the template the roomier node wins");
    }

    #[test]
    fn the_vms_a_node_runs_count_against_it() {
        let picked = choose(&[
            candidate("n0", 4096, 4, false),
            candidate("n1", 3000, 0, false),
        ]);
        assert_eq!(picked, "n1");
    }

    #[test]
    fn a_tie_goes_to_n0_whatever_the_order() {
        let picked = choose(&[
            candidate("n1", 4096, 0, false),
            candidate("n0", 4096, 0, false),
        ]);
        assert_eq!(picked, "n0");
    }

    #[test]
    fn a_node_without_room_for_an_agent_is_skipped_and_none_means_n0() {
        let picked = choose(&[
            candidate("n0", 512, 0, false),
            candidate("n1", 2048, 0, false),
        ]);
        assert_eq!(picked, "n1");
        assert_eq!(choose(&[candidate("n1", 512, 0, true)]), "n0");
        assert_eq!(choose(&[]), "n0");
    }

    #[test]
    fn a_move_is_refused_to_the_same_node_an_unknown_or_unusable_node() {
        let now = 1_000_000_000;
        let fresh = node_record("n1", now, false, false);
        assert_eq!(
            target_refusal("n1", "n1", Some(&fresh), now).map(|r| r.0),
            Some(409)
        );
        assert_eq!(
            target_refusal("n0", "n2", None, now).map(|r| r.0),
            Some(404)
        );
        let revoked = node_record("n1", now, false, true);
        assert_eq!(
            target_refusal("n0", "n1", Some(&revoked), now).map(|r| r.0),
            Some(409)
        );
        let draining = node_record("n1", now, true, false);
        assert_eq!(
            target_refusal("n0", "n1", Some(&draining), now).map(|r| r.0),
            Some(409)
        );
        let silent = node_record("n1", now - OFFLINE_AFTER_MS, false, false);
        assert_eq!(
            target_refusal("n0", "n1", Some(&silent), now).map(|r| r.0),
            Some(409)
        );
        assert_eq!(
            target_refusal("n1", "n0", None, now),
            None,
            "n0 takes agents from a joined node"
        );
        assert_eq!(target_refusal("n0", "n1", Some(&fresh), now), None);
    }

    #[test]
    fn a_working_or_offline_agent_does_not_move() {
        assert_eq!(status_refusal(7, Status::Working).map(|r| r.0), Some(409));
        assert_eq!(status_refusal(7, Status::Offline).map(|r| r.0), Some(409));
        assert_eq!(status_refusal(7, Status::Idle), None);
        assert_eq!(status_refusal(7, Status::Asleep), None);
    }

    #[test]
    fn a_recorded_move_wins_over_the_cid_ranges() {
        assert_eq!(node_for(Some("n0".into()), Some("n1".into())), "n0");
        assert_eq!(node_for(None, Some("n1".into())), "n1");
        assert_eq!(node_for(None, None), LOCAL);
    }

    #[test]
    fn the_override_map_survives_a_snapshot_and_a_clear_is_saved_too() {
        let store = crate::nodes::Store::default();
        assert_eq!(store.set_override(1005, Some("n2")), None);
        assert_eq!(store.override_of(1005).as_deref(), Some("n2"));
        let restored = crate::nodes::Store::default();
        restored.restore(&store.snapshot()).unwrap();
        assert_eq!(restored.override_of(1005).as_deref(), Some("n2"));
        assert_eq!(restored.set_override(1005, None).as_deref(), Some("n2"));
        let after_clear = crate::nodes::Store::default();
        after_clear.restore(&restored.snapshot()).unwrap();
        assert_eq!(after_clear.override_of(1005), None, "the clear is saved");
    }

    #[test]
    fn a_snapshot_from_before_overrides_restores_with_none() {
        let store = crate::nodes::Store::default();
        store.restore(r#"{"nodes":[],"codes":[]}"#).unwrap();
        assert_eq!(store.override_of(1005), None);
    }

    #[test]
    fn node_of_takes_the_override_over_the_range_owner() {
        let store = crate::nodes::Store::default();
        store
            .restore(
                &json!({"nodes": [{"id": "n1", "name": "a", "public_key": "", "cid_base": 1000,
                    "cid_count": 100, "joined_ms": 0, "last_seen_ms": 0}], "codes": []})
                .to_string(),
            )
            .unwrap();
        let owner = |cid| store.owner_of_cid(cid).map(|n| n.id);
        assert_eq!(node_for(store.override_of(1005), owner(1005)), "n1");
        store.set_override(1005, Some("n0"));
        assert_eq!(node_for(store.override_of(1005), owner(1005)), "n0");
        assert_eq!(node_for(store.override_of(1006), owner(1006)), "n1");
    }

    #[test]
    fn a_claim_holds_the_agent_until_it_is_dropped() {
        let first = Claim::take(4242).expect("the first claim");
        assert!(is_moving(4242));
        assert!(Claim::take(4242).is_none(), "a second move waits");
        drop(first);
        assert!(!is_moving(4242));
        assert!(Claim::take(4242).is_some());
    }

    /// A source of `files` that answers `copy_files` pieces, and a target that collects them.
    fn copy_with(
        files: &std::collections::BTreeMap<String, Vec<u8>>,
        parts: &[Part],
        chunk: u64,
        eof_early: bool,
    ) -> (
        Result<u64, String>,
        std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        let mut target: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
        let result = copy_files(
            chunk,
            parts,
            |name, offset, len| {
                let bytes = &files[name];
                let end = usize::try_from(offset + len).unwrap();
                let piece = &bytes[usize::try_from(offset).unwrap()..end];
                let eof = end == bytes.len() || eof_early;
                Ok(Fetched {
                    data: b64(piece),
                    eof,
                    zeros: None,
                })
            },
            |name, offset, data| {
                let bytes = unb64(data).unwrap();
                let file = target.entry(name.to_string()).or_default();
                assert_eq!(file.len() as u64, offset, "pieces arrive in order");
                file.extend(bytes);
                Ok(())
            },
        );
        (result, target)
    }

    fn part(name: &str, len: usize) -> Part {
        Part {
            name: name.to_string(),
            len: len as u64,
            sha256: String::new(),
        }
    }

    #[test]
    fn a_file_is_copied_in_pieces_and_arrives_whole() {
        let data: Vec<u8> = (0..10).collect();
        let files = std::collections::BTreeMap::from([("memory".to_string(), data.clone())]);
        let (result, target) = copy_with(&files, &[part("memory", 10)], 4, false);
        assert_eq!(result, Ok(10));
        assert_eq!(target["memory"], data);
    }

    #[test]
    fn every_write_is_at_most_512_kib_and_the_file_arrives_whole() {
        let len = 2 * WRITE_CHUNK + 5;
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let files = std::collections::BTreeMap::from([("memory".to_string(), data.clone())]);
        let mut target: Vec<u8> = Vec::new();
        let mut largest = 0;
        let result = copy_files(
            CHUNK,
            &[part("memory", len)],
            |_, offset, piece_len| {
                let piece = &files["memory"][usize::try_from(offset).unwrap()..]
                    [..usize::try_from(piece_len).unwrap()];
                Ok(Fetched {
                    data: b64(piece),
                    eof: offset + piece_len == len as u64,
                    zeros: None,
                })
            },
            |_, offset, encoded| {
                let bytes = unb64(encoded).unwrap();
                largest = largest.max(bytes.len());
                assert_eq!(target.len() as u64, offset, "writes arrive in order");
                target.extend(bytes);
                Ok(())
            },
        );
        assert_eq!(result, Ok(len as u64));
        assert!(
            largest <= WRITE_CHUNK,
            "the largest write was {largest} bytes"
        );
        assert_eq!(target, data);
    }

    #[test]
    fn an_empty_file_is_created_and_copies_nothing() {
        let files = std::collections::BTreeMap::from([("vmstate".to_string(), Vec::new())]);
        let (result, target) = copy_with(&files, &[part("vmstate", 0)], CHUNK, false);
        assert_eq!(result, Ok(0));
        assert_eq!(target["vmstate"], Vec::<u8>::new());
    }

    #[test]
    fn a_run_of_zeros_is_counted_and_never_pushed() {
        let parts = [part("memory", 8)];
        let mut pushed = Vec::new();
        let copied = copy_files(
            4,
            &parts,
            |_, offset, len| {
                Ok(if offset == 0 {
                    Fetched {
                        data: String::new(),
                        eof: false,
                        zeros: Some(len),
                    }
                } else {
                    Fetched {
                        data: b64(&[7, 7, 7, 7]),
                        eof: true,
                        zeros: None,
                    }
                })
            },
            |_, at, data| {
                pushed.push((at, data.to_string()));
                Ok(())
            },
        )
        .expect("copies");
        assert_eq!(copied, 8);
        assert_eq!(pushed, vec![(4, b64(&[7, 7, 7, 7]))]);
    }

    #[test]
    fn a_source_that_ends_early_or_a_short_piece_fails_the_copy() {
        let files = std::collections::BTreeMap::from([("memory".to_string(), vec![1_u8; 10])]);
        let (result, _) = copy_with(&files, &[part("memory", 10)], 4, true);
        assert!(result.unwrap_err().contains("eof"), "eof before the end");
        let mut short = |_: &str, _: u64, len: u64| {
            Ok(Fetched {
                data: b64(&vec![1_u8; usize::try_from(len).unwrap() - 1]),
                eof: false,
                zeros: None,
            })
        };
        let parts = [part("memory", 10)];
        let result = copy_files(4, &parts, &mut short, |_, _, _| Ok(()));
        assert!(result.unwrap_err().contains("bytes"), "a short piece");
    }

    #[test]
    fn a_failed_push_stops_the_copy() {
        let parts = [part("memory", 8)];
        let result = copy_files(
            4,
            &parts,
            |_, offset, len| {
                let piece = vec![0_u8; usize::try_from(len).unwrap()];
                Ok(Fetched {
                    data: b64(&piece),
                    eof: offset + len == 8,
                    zeros: None,
                })
            },
            |_, offset, _| {
                if offset == 4 {
                    Err("the import refused the piece".to_string())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result, Err("the import refused the piece".to_string()));
    }
}
