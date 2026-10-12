//! Fleet routing: every call that reaches an agent's gateway (vsockd) or a machine's noded goes
//! through here. The hub's own machine is node `n0`, reached over loopback; a joined node is
//! reached through its command queue (`nodes::Store::enqueue`), which its apz-link long-polls.
//! An agent lives on the node whose CID range holds its CID.

use apz_http::{Params, Request, Response, Router};
use serde_json::{json, Value};

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

/// A node's machine and runs for the console: `GET /v1/nodes/{id}/machine`, `/runs`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
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
