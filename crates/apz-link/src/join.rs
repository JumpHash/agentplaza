//! `apz-link join <token>`: decodes the token, refuses an expired one, checks the hub's
//! certificate against the token's pin (`GET /v1/hub/identity` over pinned TLS), generates the
//! node key, redeems the code (`POST /v1/nodes/join` with `{secret, public_key, name, machine}`)
//! and saves the identity. The token's secret is never written to disk or printed.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_http::client::{Target, request_json};
use apz_trust::node::NodeKey;
use apz_trust::token::JoinToken;
use serde_json::{Value, json};

use crate::identity::{self, Identity};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Joins the hub named in `token`. `name` is this node's display name, `machine` its summary
/// (cpus, memory_mib, host, version). `mesh` is the node's address on a mesh (`--mesh`): the hub then
/// reaches the node there (path `mesh`), else the node keeps its tunnel (path `tunnel`). Refuses when
/// `dir` already holds an identity.
pub fn join(
    token: &str,
    name: &str,
    machine: Value,
    dir: &Path,
    mesh: Option<&str>,
) -> Result<Identity, String> {
    if dir.join("node.json").exists() {
        return Err(format!("already joined: {}", dir.display()));
    }
    if let Some(address) = mesh {
        check_mesh_address(address)?;
    }
    let token = JoinToken::decode(token)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("system clock is before 1970: {e}"))?
        .as_secs();
    if token.expires <= now {
        return Err("this join code has expired".into());
    }

    let hub_url = token.hub_url.trim_end_matches('/').to_string();
    let target = Target {
        base: hub_url.clone(),
        pin: Some(token.cert_sha256),
        bearer: None,
        headers: Vec::new(),
    };

    let (status, body) = request_json(&target, "GET", "/v1/hub/identity", None, TIMEOUT)
        .map_err(|e| format!("cannot reach the hub: {e}"))?;
    if status != 200 {
        return Err(hub_error(status, &body));
    }
    if let Some(fingerprint) = body.get("fingerprint").and_then(Value::as_str)
        && fingerprint != apz_trust::b64(&token.cert_sha256)
    {
        return Err("the hub's certificate does not match the code".into());
    }
    let identity_name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let key = NodeKey::generate();
    let request = json!({
        "secret": apz_trust::b64(&token.secret),
        "public_key": key.public_b64(),
        "name": name,
        "machine": machine,
        "path": if mesh.is_some() { "mesh" } else { "tunnel" },
        "mesh_address": mesh.unwrap_or_default(),
    });
    let (status, body) = request_json(&target, "POST", "/v1/nodes/join", Some(&request), TIMEOUT)
        .map_err(|e| format!("cannot reach the hub: {e}"))?;
    if status != 201 {
        return Err(hub_error(status, &body));
    }

    let node_id = body
        .get("node_id")
        .and_then(Value::as_str)
        .ok_or("the hub's join reply has no node_id")?;
    let cid_base = body
        .get("cid_base")
        .and_then(Value::as_u64)
        .ok_or("the hub's join reply has no cid_base")?;
    let cid_count = body
        .get("cid_count")
        .and_then(Value::as_u64)
        .ok_or("the hub's join reply has no cid_count")?;
    let hub_name = body
        .get("hub")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or(identity_name);

    let identity = Identity {
        hub_url,
        pin: apz_trust::b64(&token.cert_sha256),
        node_id: node_id.to_string(),
        cid_base,
        cid_count,
        hub_name,
        mesh: mesh.map(str::to_string),
    };
    identity::save(dir, &identity, &key)?;
    Ok(identity)
}

/// A mesh address is `host:port` with a port: what the node listens on and the hub dials.
pub fn check_mesh_address(address: &str) -> Result<(), String> {
    let (host, port) = address
        .rsplit_once(':')
        .ok_or("--mesh needs host:port (the node's address on the mesh)")?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return Err("--mesh needs host:port (the node's address on the mesh)".into());
    }
    Ok(())
}

/// The hub's `error` field when it sent one, else its status code.
fn hub_error(status: u16, body: &Value) -> String {
    match body.get("error").and_then(Value::as_str) {
        Some(message) => message.to_string(),
        None => format!("the hub answered {status}"),
    }
}

/// This machine's summary for the hub: `{"cpus", "memory_mib", "host", "version"}` from
/// `/proc/cpuinfo`, `/proc/meminfo`, `/etc/hostname` and the crate version.
pub fn machine_summary() -> Value {
    let mut summary = serde_json::Map::new();
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        let cpus = cpuinfo
            .lines()
            .filter(|line| line.starts_with("processor"))
            .count();
        summary.insert("cpus".into(), json!(cpus));
    }
    if let Some(kb) = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| mem_total_kb(&text))
    {
        summary.insert("memory_mib".into(), json!(kb / 1024));
    }
    if let Ok(host) = std::fs::read_to_string("/etc/hostname") {
        summary.insert("host".into(), json!(host.trim()));
    }
    summary.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    Value::Object(summary)
}

/// The `MemTotal` figure of `/proc/meminfo`, in kB; None when absent or malformed.
fn mem_total_kb(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find(|line| line.starts_with("MemTotal:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kb| kb.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("apz-join-{tag}-{}", std::process::id()))
    }

    #[test]
    fn expired_token_is_refused_before_any_network_call() {
        let token =
            JoinToken::issue("https://hub.example:7443", [0; 32], now() - 1000, 900).encode();
        let dir = scratch("expired");
        let err = join(&token, "node", json!({}), &dir, None).unwrap_err();
        assert_eq!(err, "this join code has expired");
        assert!(!dir.exists());
    }

    #[test]
    fn a_dir_that_holds_node_json_is_refused() {
        let dir = scratch("joined");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.json"), "{}").unwrap();
        let token = JoinToken::issue("https://hub.example:7443", [0; 32], now(), 900).encode();
        let err = join(&token, "node", json!({}), &dir, None).unwrap_err();
        assert!(err.starts_with("already joined: "), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mesh_address_is_host_and_port() {
        assert!(check_mesh_address("100.64.0.7:7447").is_ok());
        assert!(check_mesh_address("node.tailnet.ts.net:7447").is_ok());
        for bad in ["100.64.0.7", ":7447", "host:port", "host:70000"] {
            assert!(check_mesh_address(bad).is_err(), "{bad}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn machine_summary_reports_cpus_on_linux() {
        let summary = machine_summary();
        assert!(summary["cpus"].as_u64().unwrap() > 0);
        assert_eq!(summary["version"], env!("CARGO_PKG_VERSION"));
    }
}
