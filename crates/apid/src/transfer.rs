//! Node-to-node transfers, the hub's side (plan 17.5). A move's files and a base's files go from the node that holds
//! them to the node that needs them. The hub only coordinates: it signs a grant (`apz_transfer::Grant`) for the pair
//! and the manifest, tells the receiver the sender's address and the grant, and the receiver fetches the chunks it
//! lacks straight from the sender. The chunk bytes never pass through the hub.
//!
//! The hub's own byte counts ([`meter`]) say what it carried: `control` is the requests and answers that coordinate a
//! transfer (node names, grants, manifests), `payload` is chunk data it relayed. The node-to-node path carries none.
//!
//! Routes (owner):
//!   POST /v1/transfers  {scope, from, to, files: [{name, path}]} → {manifest, from, to, fetched, hub_bytes}
//!     `from` publishes the files from its disk, `to` fetches them; a base's files go this way.

use std::sync::atomic::{AtomicU64, Ordering};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_transfer::Grant;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fleet;
use crate::state::App;

static CONTROL: AtomicU64 = AtomicU64::new(0);
static PAYLOAD: AtomicU64 = AtomicU64::new(0);

/// What the hub has carried for transfers, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Meter {
    /// The requests and answers that coordinate transfers: node names, grants, manifests.
    pub control: u64,
    /// Chunk data the hub relayed itself. A transfer the hub only coordinates carries none.
    pub payload: u64,
}

/// The hub's byte counts since it started.
pub fn meter() -> Meter {
    Meter {
        control: CONTROL.load(Ordering::Relaxed),
        payload: PAYLOAD.load(Ordering::Relaxed),
    }
}

/// Counts chunk data the hub relayed (a call that moves file bytes through it).
pub fn note_payload(bytes: usize) {
    PAYLOAD.fetch_add(bytes as u64, Ordering::Relaxed);
}

/// The hub's calls to nodes, as `(node, method, path, body) -> reply`.
pub type Caller<'a> = dyn FnMut(&str, &str, &str, &Value) -> Result<Value, String> + 'a;

/// One call for a transfer: the body and the reply are counted as control.
pub fn exchange(
    send: &mut Caller<'_>,
    node: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    CONTROL.fetch_add(body.to_string().len() as u64, Ordering::Relaxed);
    let reply = send(node, method, path, body)?;
    CONTROL.fetch_add(reply.to_string().len() as u64, Ordering::Relaxed);
    Ok(reply)
}

/// The hub's grant key (`<state_dir>/transfer_key`, made on first use).
pub fn key(app: &App) -> Result<SigningKey, String> {
    crate::shares::relay::load_key(&app.config.state_dir.join("transfer_key"))
        .map_err(|err| format!("the transfer key: {err}"))
}

/// The body of a call to `node`: `extra` plus the hub's name for the node and its grant key, which the node takes
/// for the call.
pub fn hub_body(key: &SigningKey, node: &str, mut extra: Value) -> Value {
    extra["hub"] = json!({
        "node": node,
        "hub_key": apz_transfer::key_to_text(&key.verifying_key()),
    });
    extra
}

/// The grant for `to` to fetch manifest `manifest_id` from `from`, short-lived.
pub fn grant_for(key: &SigningKey, scope: &str, manifest_id: &str, from: &str, to: &str) -> Grant {
    Grant::issue(
        key,
        scope,
        manifest_id,
        from,
        to,
        apz_transfer::now_ms() + apz_transfer::grant::TTL_MS,
    )
}

/// Where node `node` takes transfers from: its address as the hub has it, and the port it serves transfers on (a node
/// fact, from its heartbeat). The hub's own machine, or one with no address, is 127.0.0.1.
pub fn address_of(app: &App, node: &str) -> Result<String, String> {
    let record = app
        .nodes
        .get(node)
        .ok_or_else(|| format!("no node {node}"))?;
    let port = record
        .machine
        .transfer_port
        .ok_or_else(|| format!("node {node} serves no transfers"))?;
    let host = match record.address.as_str() {
        "" | "loopback" => "127.0.0.1".to_string(),
        ip if ip.contains(':') => format!("[{ip}]"),
        ip => ip.to_string(),
    };
    Ok(format!("{host}:{port}"))
}

/// Distributes files that node `from` holds to node `to`: `from` publishes them as blobs (`files`: `name` and the
/// `path` on `from`), the hub signs a grant for `to` and says where to fetch (`from_addr`), and `to` fetches the chunks
/// it lacks. Returns the manifest's id and what `to` fetched.
pub fn distribute_with(
    key: &SigningKey,
    scope: &str,
    (from, from_addr): (&str, &str),
    to: &str,
    files: &Value,
    send: &mut Caller<'_>,
) -> Result<Value, String> {
    let published = exchange(
        send,
        from,
        "POST",
        "/v1/transfers/manifests",
        &hub_body(key, from, json!({"files": files})),
    )?;
    let id = text(&published, "id")?;
    let grant = grant_for(key, scope, &id, from, to);
    let fetched = exchange(
        send,
        to,
        "POST",
        "/v1/transfers/fetch",
        &hub_body(
            key,
            to,
            json!({
                "from": from_addr,
                "grant": grant.encode(),
                "manifest": published["manifest"],
            }),
        ),
    )?;
    Ok(json!({"manifest": id, "from": from, "to": to, "fetched": fetched}))
}

/// `POST /v1/transfers`: [`distribute_with`] over the plaza's nodes.
pub fn distribute(
    app: &App,
    scope: &str,
    from: &str,
    to: &str,
    files: &Value,
) -> Result<Value, String> {
    let key = key(app)?;
    let from_addr = address_of(app, from)?;
    let mut send = |node: &str, method: &str, path: &str, body: &Value| {
        answered(
            &format!("{node} {method} {path}"),
            fleet::noded(app, node, method, path, Some(body)),
        )
    };
    distribute_with(&key, scope, (from, &from_addr), to, files, &mut send)
}

/// A node's answer as its body when it is 2xx; otherwise the failure as text.
fn answered(what: &str, result: Result<(u16, Value), String>) -> Result<Value, String> {
    match result {
        Ok((code, body)) if (200..300).contains(&code) => Ok(body),
        Ok((code, body)) => Err(format!("{what} answered {code}: {body}")),
        Err(err) => Err(format!("{what}: {err}")),
    }
}

fn text(body: &Value, key: &str) -> Result<String, String> {
    body[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("the node's answer has no {key}"))
}

/// Routes: distribute files between two nodes.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/transfers", distribute_route)
        .doc(
            RouteDoc::new("Distribute files from one node to another: the receiver fetches the chunks it lacks from the sender")
                .request::<TransferBody>()
                .errors(&[400, 502]),
        )
}

#[derive(Deserialize, schemars::JsonSchema)]
struct TransferBody {
    /// What the grant is for, for the record (`base:<project>:<kind>`).
    scope: String,
    from: String,
    to: String,
    /// The files, by name and the path on `from`.
    files: Vec<TransferFile>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
struct TransferFile {
    name: String,
    path: String,
}

fn distribute_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: TransferBody = match req.json() {
        Ok(body) => body,
        Err(err) => return Response::error(400, &format!("invalid transfer: {err}")),
    };
    if body.files.is_empty() {
        return Response::error(400, "a transfer needs at least one file");
    }
    if body.from == body.to {
        return Response::error(400, "a transfer needs two nodes");
    }
    let files = serde_json::to_value(&body.files).unwrap_or(Value::Null);
    match distribute(app, &body.scope, &body.from, &body.to, &files) {
        Ok(mut done) => {
            // What the hub itself carried so far: control only, when the transfer went node to node.
            done["hub_bytes"] = json!(meter());
            Response::json(200, &done)
        }
        Err(err) => Response::error(502, &err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use apz_blobs::{Blobs, Hash};
    use apz_transfer::{Manifest, Node};

    /// A stand-in for two nodes' noded, each a blob store served on loopback. It answers what the real noded answers
    /// for the two routes a transfer uses, adopting the hub's name and grant key as noded does.
    struct Pair {
        hub: SigningKey,
        a: Arc<Node>,
        b: Arc<Node>,
        b_blobs: Arc<Blobs>,
        _server: apz_http::Handle,
        addr: String,
        dirs: Vec<tempfile::TempDir>,
    }

    fn pair() -> Pair {
        let dirs = vec![tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let a_blobs = Arc::new(Blobs::open(dirs[0].path()).unwrap());
        let b_blobs = Arc::new(Blobs::open(dirs[1].path()).unwrap());
        let a = Node::new("", Arc::clone(&a_blobs));
        let b = Node::new("", Arc::clone(&b_blobs));
        let server = a.spawn("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = format!("127.0.0.1:{}", server.port());
        Pair {
            hub: SigningKey::from_bytes(&[5; 32]),
            a,
            b,
            b_blobs,
            _server: server,
            addr,
            dirs,
        }
    }

    /// The stand-in noded: the hub's name and grant key are taken from each call's `hub`, as noded takes them.
    fn stand_in(p: &Pair) -> impl FnMut(&str, &str, &str, &Value) -> Result<Value, String> + '_ {
        move |node, _method, path, body| {
            let hub = &body["hub"];
            let key = apz_transfer::key_from_text(hub["hub_key"].as_str().unwrap())?;
            let me = if node == "a" { &p.a } else { &p.b };
            me.set_id(node);
            me.trust(key);
            match path {
                "/v1/transfers/manifests" => {
                    let mut files = Vec::new();
                    for file in body["files"].as_array().unwrap() {
                        let name = file["name"].as_str().unwrap();
                        let path = file["path"].as_str().unwrap();
                        let handle = std::fs::File::open(path).map_err(|e| e.to_string())?;
                        files.push(Manifest::store(me.blobs(), name, handle)?);
                    }
                    let manifest = Manifest { files };
                    let id = me.publish(&manifest)?;
                    Ok(json!({"id": id, "manifest": manifest}))
                }
                "/v1/transfers/fetch" => {
                    let grant = Grant::decode(body["grant"].as_str().unwrap())?;
                    let manifest: Manifest =
                        serde_json::from_value(body["manifest"].clone()).unwrap();
                    let from = body["from"].as_str().unwrap();
                    let report = apz_transfer::fetch(me.blobs(), node, from, &grant, &manifest)?;
                    Ok(
                        json!({"fetched": report.fetched, "reused": report.reused, "bytes": report.bytes}),
                    )
                }
                other => Err(format!("no route {other}")),
            }
        }
    }

    #[test]
    fn a_base_reaches_a_second_node_and_the_hub_carries_no_chunk_bytes() {
        let p = pair();
        let base = dirs_file(&p.dirs[0], "base.bin", 24 << 20, 3);
        let files = json!([{"name": "base.bin", "path": base}]);
        let before = meter();
        let mut send = stand_in(&p);
        let done = distribute_with(
            &p.hub,
            "base:p:build",
            ("a", &p.addr),
            "b",
            &files,
            &mut send,
        )
        .unwrap();
        let after = meter();
        // The chunks went from a to b on loopback; the hub carried only the coordination.
        let moved = done["fetched"]["bytes"].as_u64().unwrap();
        assert_eq!(moved, 24 << 20, "the whole base came: {done}");
        assert_eq!(p.a.served_bytes(), moved, "a sent them to b");
        assert_eq!(
            after.payload, before.payload,
            "the hub relayed no chunk bytes"
        );
        assert!(
            after.control - before.control < 64 * 1024,
            "control only: {after:?}"
        );
        let bytes = std::fs::read(&base).unwrap();
        assert_eq!(
            p.b_blobs.get(&Hash::of(&bytes)).unwrap(),
            bytes,
            "b holds the base"
        );
    }

    /// A file of `len` bytes of noise in `dir`, returned as its path.
    fn dirs_file(dir: &tempfile::TempDir, name: &str, len: usize, seed: u64) -> String {
        let mut x = seed | 1;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_grant_for_one_pair_is_the_hubs_and_names_its_manifest() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let g = grant_for(&key, "move:1:e", "m", "a", "b");
        assert_eq!(
            g.check(&key.verifying_key(), "a", Some("b"), apz_transfer::now_ms()),
            Ok(())
        );
        assert_eq!(
            g.check(&key.verifying_key(), "a", Some("c"), apz_transfer::now_ms()),
            Err(apz_transfer::Refusal::NotTheReceiver)
        );
    }
}
