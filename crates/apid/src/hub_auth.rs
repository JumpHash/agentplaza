//! The hub's front door: who may call it, and how the hub proves who it is.
//!
//! - Loopback callers (the console on this machine, the local agent gateway) are trusted as today.
//! - Any other caller of an owner route must send `Authorization: Bearer <owner token>`; the hub
//!   stores only the token's SHA-256 (`apz_trust::owner`). The token is made on the hub's first
//!   start (printed once, and written to `<state>/owner-token` mode 0600 for the desktop app to
//!   read on this machine) and can be rotated.
//! - Node routes are public but rate-limited (`/v1/nodes/join`) or signed by the node's key
//!   (`/v1/nodes/{id}/…`, and `/internal/…` from a remote node) — `crate::nodes` checks those.
//! - Off loopback the hub serves HTTPS with a self-signed certificate made on first start
//!   (`<state>/tls/cert.pem`, `key.pem`); its SHA-256 is the pin join tokens carry.
//! Saved: `<state>/hub.json` (owner token hash, public URL, name), mode 0600.

use std::path::Path;
use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};

use crate::state::App;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HubConfig {
    /// SHA-256 of the owner token, hex.
    pub owner_token_sha256: String,
    /// How nodes reach this hub (e.g. `https://hub.example.org:7700`); join tokens carry it.
    pub public_url: String,
    pub name: String,
}

/// The hub's identity and access settings.
pub struct Hub {
    pub config: Mutex<HubConfig>,
    /// The TLS certificate's SHA-256 (all zero when the hub has no certificate yet).
    pub cert_sha256: [u8; 32],
}

impl Hub {
    /// A loopback-only hub: no certificate, no owner token (everything local is trusted).
    pub fn local() -> Hub {
        Hub {
            config: Mutex::new(HubConfig { name: "AgentPlaza".into(), public_url: "http://127.0.0.1:7700".into(), ..Default::default() }),
            cert_sha256: [0; 32],
        }
    }
    /// Loads or creates `<state>/hub.json`, the owner token and the TLS certificate (made when
    /// `tls` is true and none exists). Prints a new owner token once to stderr.
    pub fn load(_state_dir: &Path, _public_url: &str, _tls: bool) -> Result<Hub, String> {
        Err("contract: O1".into())
    }
    /// Whether `token` is the owner's.
    pub fn owner_ok(&self, _token: &str) -> bool {
        false // contract: O1
    }
    /// A new owner token (returned once; the old one stops working).
    pub fn rotate_owner_token(&self, _state_dir: &Path) -> Result<String, String> {
        Err("contract: O1".into())
    }
    pub fn fingerprint_b64(&self) -> String {
        apz_trust::b64(&self.cert_sha256)
    }
}

/// Routes anyone may call without the owner token.
pub fn is_public(method: &str, path: &str) -> bool {
    matches!((method, path), ("GET", "/v1/hub/identity") | ("POST", "/v1/nodes/join"))
        || (path.starts_with("/v1/nodes/n") && (path.ends_with("/commands") || path.ends_with("/results") || path.ends_with("/heartbeat")))
        || path.starts_with("/internal/")
        || !path.starts_with("/v1/")
}

/// The gate in front of the router: `Some(response)` refuses the request.
pub fn gate(_app: &App, _req: &Request) -> Option<Response> {
    None // contract: O1 — loopback passes; public routes pass; else bearer owner token or 401
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/hub/identity", identity_route)
        .post("/v1/hub/owner-token/rotate", rotate_route)
        .get("/v1/auth/check", |_, _, _| Response::json(200, &serde_json::json!({ "ok": true })))
}

/// `{name, version, url, fingerprint, nodes}` — what a joining node and a signing-in console see.
fn identity_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    Response::error(501, "contract: O1")
}

fn rotate_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    Response::error(501, "contract: O1")
}
