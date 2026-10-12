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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};

use crate::state::App;

const HUB_FILE: &str = "hub.json";
const OWNER_TOKEN_FILE: &str = "owner-token";

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
    /// Loads or creates `<state>/hub.json`, the owner token and the TLS certificate (made when
    /// `tls` is true and none exists). Prints a new owner token once to stderr.
    pub fn load(state_dir: &Path, public_url: &str, tls: bool) -> Result<Hub, String> {
        std::fs::create_dir_all(state_dir).map_err(|e| format!("{}: {e}", state_dir.display()))?;
        let path = state_dir.join(HUB_FILE);
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<HubConfig>(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => HubConfig { name: "AgentPlaza".into(), ..Default::default() },
            Err(err) => return Err(format!("{}: {err}", path.display())),
        };
        config.public_url = public_url.to_string();
        if config.owner_token_sha256.is_empty() {
            let token = apz_trust::owner::new_token();
            config.owner_token_sha256 = hex_encode(&apz_trust::owner::hash(&token));
            write_private(&owner_token_path(state_dir), token.as_bytes())?;
            eprintln!("apid: the owner token (shown once; also in {}): {token}", owner_token_path(state_dir).display());
        }
        save(state_dir, &config)?;
        let cert_sha256 = if tls { certificate(state_dir, public_url) } else { [0; 32] };
        Ok(Hub { config: Mutex::new(config), cert_sha256 })
    }

    /// Whether `token` is the owner's.
    pub fn owner_ok(&self, token: &str) -> bool {
        let stored = hex_decode(&self.lock().owner_token_sha256);
        stored.is_some_and(|stored| apz_trust::owner::check(token, &stored))
    }

    /// A new owner token (returned once; the old one stops working).
    pub fn rotate_owner_token(&self, state_dir: &Path) -> Result<String, String> {
        let token = apz_trust::owner::new_token();
        let mut config = self.lock();
        let mut next = config.clone();
        next.owner_token_sha256 = hex_encode(&apz_trust::owner::hash(&token));
        save(state_dir, &next)?;
        *config = next;
        write_private(&owner_token_path(state_dir), token.as_bytes())?;
        Ok(token)
    }

    pub fn fingerprint_b64(&self) -> String {
        apz_trust::b64(&self.cert_sha256)
    }

    fn lock(&self) -> MutexGuard<'_, HubConfig> {
        self.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
pub fn gate(app: &App, req: &Request) -> Option<Response> {
    if is_public(&req.method, &req.path) {
        return None;
    }
    // A request whose peer is unknown or loopback is the machine's own (console, local gateway).
    if req.peer.is_none_or(|peer| peer.is_loopback()) {
        return None;
    }
    let presented = req.header("authorization").and_then(|value| {
        let (scheme, token) = value.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then_some(token.trim())
    });
    if presented.is_some_and(|token| app.hub_auth.owner_ok(token)) {
        return None;
    }
    Some(Response::json(401, &serde_json::json!({ "error": "sign in with the owner token" })).with_header("www-authenticate", "Bearer"))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/hub/identity", identity_route)
        .post("/v1/hub/owner-token/rotate", rotate_route)
        .get("/v1/auth/check", |_, _, _| Response::json(200, &serde_json::json!({ "ok": true })))
}

/// `{name, version, url, fingerprint, nodes}` — what a joining node and a signing-in console see.
fn identity_route(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    let (name, url) = {
        let config = app.hub_auth.lock();
        (config.name.clone(), config.public_url.clone())
    };
    let fingerprint = (app.hub_auth.cert_sha256 != [0; 32]).then(|| app.hub_auth.fingerprint_b64());
    Response::json(
        200,
        &serde_json::json!({
            "name": name,
            "version": env!("CARGO_PKG_VERSION"),
            "url": url,
            "fingerprint": fingerprint,
            "nodes": app.nodes.list().len(),
        }),
    )
}

fn rotate_route(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    match app.hub_auth.rotate_owner_token(&app.config.state_dir) {
        Ok(token) => Response::json(200, &serde_json::json!({ "token": token })),
        Err(err) => Response::error(500, &err),
    }
}

/// The TLS certificate's SHA-256, making the certificate on first use. Zero (and logged) when
/// the certificate cannot be made or read.
fn certificate(state_dir: &Path, public_url: &str) -> [u8; 32] {
    match certificate_fingerprint(state_dir, public_url) {
        Ok(fingerprint) => fingerprint,
        Err(err) => {
            eprintln!("apid: no TLS certificate: {err}");
            [0; 32]
        }
    }
}

fn certificate_fingerprint(state_dir: &Path, public_url: &str) -> Result<[u8; 32], String> {
    let dir = state_dir.join("tls");
    let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
    let existing = std::fs::read_to_string(&cert_path).ok().zip(std::fs::read_to_string(&key_path).ok());
    let cert_pem = match existing {
        Some((cert_pem, _key_pem)) => cert_pem,
        None => {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let pair = apz_trust::tls::self_signed(&names(public_url))?;
            write_private(&cert_path, pair.cert_pem.as_bytes())?;
            write_private(&key_path, pair.key_pem.as_bytes())?;
            pair.cert_pem
        }
    };
    apz_trust::tls::fingerprint(&cert_pem)
}

/// The names the certificate is valid for: the public host, then loopback.
fn names(public_url: &str) -> Vec<String> {
    let mut names = vec![host_of(public_url), "localhost".to_string(), "127.0.0.1".to_string()];
    names.retain(|name| !name.is_empty());
    names
}

/// The host of a URL such as `https://hub.example.org:7700/x` or `http://[::1]:7700`.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default().to_string(),
        None => host.split(':').next().unwrap_or_default().to_string(),
    }
}

fn owner_token_path(state_dir: &Path) -> PathBuf {
    state_dir.join(OWNER_TOKEN_FILE)
}

fn save(state_dir: &Path, config: &HubConfig) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    write_private(&state_dir.join(HUB_FILE), text.as_bytes())
}

/// Writes `path` with mode 0600 (the owner token, the hub's record and the key are private).
fn write_private(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let fail = |err: std::io::Error| format!("{}: {err}", path.display());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(fail)?;
    file.write_all(contents).map_err(fail)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(fail)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(text.as_bytes().chunks(2)) {
        *slot = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-hub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn token_of(dir: &Path) -> String {
        std::fs::read_to_string(owner_token_path(dir)).unwrap()
    }

    #[test]
    fn first_start_makes_hub_json_and_owner_token_and_second_load_keeps_it() {
        let dir = scratch("first");
        let hub = Hub::load(&dir, "http://127.0.0.1:7700", false).unwrap();
        let token = token_of(&dir);
        assert!(token.starts_with(apz_trust::owner::PREFIX));
        assert!(hub.owner_ok(&token));
        assert_eq!(mode(&owner_token_path(&dir)), 0o600);
        assert_eq!(mode(&dir.join(HUB_FILE)), 0o600);
        let first = hub.lock().owner_token_sha256.clone();
        drop(hub);

        let again = Hub::load(&dir, "http://127.0.0.1:7700", false).unwrap();
        assert_eq!(again.lock().owner_token_sha256, first);
        assert!(again.owner_ok(&token));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_token_is_refused() {
        let dir = scratch("other");
        let hub = Hub::load(&dir, "http://127.0.0.1:7700", false).unwrap();
        assert!(!hub.owner_ok("apzo_not-the-owner"));
        assert!(!hub.owner_ok(""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_invalidates_the_old_token() {
        let dir = scratch("rotate");
        let hub = Hub::load(&dir, "http://127.0.0.1:7700", false).unwrap();
        let old = token_of(&dir);
        let new = hub.rotate_owner_token(&dir).unwrap();
        assert_ne!(old, new);
        assert!(!hub.owner_ok(&old));
        assert!(hub.owner_ok(&new));
        assert_eq!(token_of(&dir), new);
        drop(hub);

        let again = Hub::load(&dir, "http://127.0.0.1:7700", false).unwrap();
        assert!(again.owner_ok(&new));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn public_routes_need_no_owner_token() {
        assert!(is_public("GET", "/v1/hub/identity"));
        assert!(is_public("POST", "/v1/nodes/join"));
        assert!(is_public("POST", "/v1/nodes/n3/heartbeat"));
        assert!(is_public("GET", "/internal/anything"));
        assert!(is_public("GET", "/index.html"));
        assert!(!is_public("GET", "/v1/auth/check"));
        assert!(!is_public("POST", "/v1/hub/owner-token/rotate"));
    }

    #[test]
    fn host_is_read_from_the_public_url() {
        assert_eq!(host_of("https://hub.example.org:7700/x"), "hub.example.org");
        assert_eq!(host_of("http://[::1]:7700"), "::1");
        assert_eq!(host_of("http://127.0.0.1:7700"), "127.0.0.1");
    }
}
