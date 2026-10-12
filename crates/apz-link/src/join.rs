//! `apz-link join <token>`: decodes the token, refuses an expired one, checks the hub's
//! certificate against the token's pin (`GET /v1/hub/identity` over pinned TLS), generates the
//! node key, redeems the code (`POST /v1/nodes/join` with `{secret, public_key, name, machine}`)
//! and saves the identity. The token's secret is never written to disk or printed.

use std::path::Path;

use serde_json::Value;

use crate::identity::Identity;

/// Joins the hub named in `token`. `name` is this node's display name, `machine` its summary
/// (cpus, memory_mib, host, version). Refuses when `dir` already holds an identity.
pub fn join(token: &str, name: &str, machine: Value, dir: &Path) -> Result<Identity, String> {
    let _ = (token, name, machine, dir);
    Err("contract: L2".into())
}

/// This machine's summary for the hub: `{"cpus", "memory_mib", "host", "version"}` from
/// `/proc/cpuinfo`, `/proc/meminfo`, `/etc/hostname` and the crate version.
pub fn machine_summary() -> Value {
    Value::Null
}
