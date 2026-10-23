//! The hub's front door: who may call it, and how the hub proves who it is.
//!
//! - Loopback callers (the console on this machine, the local agent gateway) are trusted, but only
//!   under a local `Host` ([`trusted_host`]): a web page that rebinds its own domain to 127.0.0.1
//!   sends that domain as `Host` and is treated like any other caller.
//! - Any other caller of an owner route must send `Authorization: Bearer <owner token>` or the
//!   session cookie a sign-in gave it (`POST /v1/auth/session` with the owner token: an HttpOnly,
//!   Secure, SameSite=Strict cookie, so the browser console's event stream and downloads carry it;
//!   the hub keeps only each session's SHA-256, for [`SESSION_MS`]). The hub stores only the
//!   owner token's SHA-256 (`apz_trust::owner`). The token is made on the hub's first
//!   start and written only to `<state>/owner-token` (mode 0600, read on this machine by the
//!   desktop app and the CLI; never logged) and can be rotated.
//! - Node routes are public but rate-limited (`/v1/nodes/join`) or signed by the node's key
//!   (`/v1/nodes/{id}/…`, and `/internal/…` from a remote node) — `crate::nodes` checks those.
//! - Off loopback the hub serves HTTPS with a self-signed certificate made on first start
//!   (`<state>/tls/cert.pem`, `key.pem`); its SHA-256 is the pin join tokens carry.
//!
//! Saved in plaza.db's `hub_config` table (owner token hash, public URL, name). The owner token file
//! (`<state>/owner-token`, mode 0600) and the TLS certificate stay files under the state folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};

use crate::state::App;

const OWNER_TOKEN_FILE: &str = "owner-token";
/// The console's session cookie.
pub const SESSION_COOKIE: &str = "apz_session";
/// How long a console session lasts.
pub const SESSION_MS: u64 = 12 * 3_600_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "hub_config")]
pub struct HubConfig {
    /// SHA-256 of the owner token, hex.
    pub owner_token_sha256: String,
    /// How nodes reach this hub (e.g. `https://hub.example.org:7700`); join tokens carry it.
    pub public_url: String,
    pub name: String,
}

/// The hub config table: `hub_config`, one row.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(HubConfig)
}

/// The hub's identity and access settings.
pub struct Hub {
    pub config: Mutex<HubConfig>,
    /// The TLS certificate's SHA-256 (all zero when the hub has no certificate yet).
    pub cert_sha256: [u8; 32],
    /// Console sessions: SHA-256 of the session id → expiry (ms). In memory only.
    sessions: Mutex<BTreeMap<[u8; 32], u64>>,
    /// The state folder: the owner token file and the TLS certificate live here.
    state_dir: PathBuf,
}

impl Hub {
    /// The hub before plaza.db is open: the defaults (named `AgentPlaza`) with `public_url`, and the TLS
    /// certificate's SHA-256, made under `<state>/tls/` when `tls` is true and none exists. [`Hub::load`] then
    /// replaces the defaults with the stored `hub_config` row.
    pub fn new(state_dir: &Path, public_url: &str, tls: bool) -> Result<Hub, String> {
        std::fs::create_dir_all(state_dir).map_err(|e| format!("{}: {e}", state_dir.display()))?;
        let config = HubConfig {
            name: "AgentPlaza".into(),
            public_url: public_url.to_string(),
            ..Default::default()
        };
        let cert_sha256 = if tls {
            certificate(state_dir, public_url)
        } else {
            [0; 32]
        };
        Ok(Hub {
            config: Mutex::new(config),
            cert_sha256,
            sessions: Mutex::default(),
            state_dir: state_dir.to_path_buf(),
        })
    }

    /// Replaces the config with the stored `hub_config` row when there is one, keeping the public URL that
    /// [`Hub::new`] was given. A first start makes the owner token: it goes only to `<state>/owner-token`, and
    /// its hash waits in memory for the saver ([`Hub::rows`]). The log names that file and never the token.
    pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<HubConfig>(conn, def())?;
        let mut config = self.lock();
        let mut next = stored.unwrap_or_else(|| config.clone());
        next.public_url = config.public_url.clone();
        if next.owner_token_sha256.is_empty() {
            let token = apz_trust::owner::new_token();
            let path = owner_token_path(&self.state_dir);
            write_private(&path, token.as_bytes())
                .map_err(|err| StoreError::Io(std::io::Error::other(err)))?;
            next.owner_token_sha256 = hex_encode(&apz_trust::owner::hash(&token));
            // The log names the file and a command that uses it. The token itself is never logged or printed.
            eprintln!(
                "apid: owner token created: {} (use it with `agentplaza hub login`)",
                path.display()
            );
        }
        *config = next;
        Ok(())
    }

    /// The config's row, as it is now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &*self.lock())
    }

    /// Whether `token` is the owner's.
    pub fn owner_ok(&self, token: &str) -> bool {
        let stored = hex_decode(&self.lock().owner_token_sha256);
        stored.is_some_and(|stored| apz_trust::owner::check(token, &stored))
    }

    /// A new owner token (returned once; the old one stops working). Its hash waits in memory for the saver.
    pub fn rotate_owner_token(&self) -> Result<String, String> {
        let token = apz_trust::owner::new_token();
        write_private(&owner_token_path(&self.state_dir), token.as_bytes())?;
        self.lock().owner_token_sha256 = hex_encode(&apz_trust::owner::hash(&token));
        // A new owner token ends every console session made with the old one.
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        Ok(token)
    }

    /// A new console session for the owner: returns the session id (only its hash is kept).
    pub fn open_session(&self, now_ms: u64) -> String {
        let id = apz_trust::b64(&apz_trust::random::<32>());
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions.retain(|_, expires| *expires > now_ms);
        sessions.insert(apz_trust::sha256(id.as_bytes()), now_ms + SESSION_MS);
        id
    }

    /// Whether `id` is a live session.
    pub fn session_ok(&self, id: &str, now_ms: u64) -> bool {
        let hash = apz_trust::sha256(id.as_bytes());
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions.get(&hash).is_some_and(|expires| *expires > now_ms)
    }

    /// Ends the session `id` (a no-op when unknown).
    pub fn close_session(&self, id: &str) {
        let hash = apz_trust::sha256(id.as_bytes());
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&hash);
    }

    pub fn fingerprint_b64(&self) -> String {
        apz_trust::b64(&self.cert_sha256)
    }

    fn lock(&self) -> MutexGuard<'_, HubConfig> {
        self.config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Routes anyone may call without the owner token.
pub fn is_public(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("GET", "/v1/hub/identity")
            | ("POST", "/v1/nodes/join")
            | ("POST", "/v1/stations/join")
            | ("GET", "/v1/stations/link")
            | ("POST" | "DELETE", "/v1/auth/session")
            // The editors' socket checks its own caller (`acp::authenticate`): the owner, or a hub MCP token.
            | ("GET", "/v1/acp")
            // A git platform sends the owner's browser back here; it acts only on a state an owner's call made.
            | ("GET", "/v1/platforms/callback")
    ) || (path.starts_with("/v1/platforms/flows/") && path.ends_with("/go") && method == "GET")
        // A project's ingest address: the token in the path names the project (`maintenance`).
        || (method == "POST" && path.starts_with("/v1/ingest/") && path.len() > "/v1/ingest/".len())
        || (path.starts_with("/v1/nodes/n")
            && (path.ends_with("/commands")
                || path.ends_with("/results")
                || path.ends_with("/heartbeat")
                || path.ends_with("/tunnel")))
        || path.starts_with("/internal/")
        || !path.starts_with("/v1/")
}

/// The gate in front of the router: `Some(response)` refuses the request.
pub fn gate(app: &App, req: &Request) -> Option<Response> {
    if is_public(&req.method, &req.path) || is_owner(app, req) {
        return None;
    }
    Some(
        Response::json(
            401,
            &serde_json::json!({ "error": "sign in with the owner token" }),
        )
        .with_header("www-authenticate", "Bearer"),
    )
}

/// The most body bytes a request that passed [`gate`] may send, decided from its line and headers before
/// the body is read: the owner and the plaza's own daemons (`x-apz-internal`) keep the server's limit;
/// a node's report (signed, checked by its route) up to [`NODE_BODY`]; any other caller of a public
/// route up to [`PUBLIC_BODY`].
pub fn body_allowance(app: &App, req: &Request) -> usize {
    allowance(req, &app.token, || is_owner(app, req))
}

fn allowance(req: &Request, internal_token: &str, owner: impl FnOnce() -> bool) -> usize {
    let internal = req.path.starts_with("/internal/")
        && req
            .header("x-apz-internal")
            .is_some_and(|token| apz_trust::ct_eq(token.as_bytes(), internal_token.as_bytes()));
    if internal || owner() {
        return usize::MAX;
    }
    if req.path.starts_with("/v1/nodes/n") {
        return NODE_BODY;
    }
    PUBLIC_BODY
}

/// The most a node's report (results, commands, heartbeats) may carry.
pub const NODE_BODY: usize = 64 << 20;

/// The most any other caller of a public route may send: joins, ingest, callbacks, sessions.
pub const PUBLIC_BODY: usize = 256 << 10;

/// Whether a request is the owner's: the machine's own (a loopback or unknown peer under a local `Host`), the owner
/// token as a bearer, or a live console session. Owner routes and the editors' socket both ask this first.
pub fn is_owner(app: &App, req: &Request) -> bool {
    is_owner_of(&app.hub_auth, req, apz_plaza::now_ms())
}

fn is_owner_of(hub: &Hub, req: &Request, now_ms: u64) -> bool {
    // A request whose peer is unknown or loopback is the machine's own (console, local gateway), when its
    // `Host` says so too: a DNS-rebinding page reaches loopback with its own name in `Host`. A browser
    // request from a page of another origin (another site, or a sandboxed frame's opaque origin) also
    // reaches loopback with a local `Host`, so it gets no loopback trust either. A WebSocket handshake keeps it:
    // the socket's own Origin policy (`apz_http::ws::Origins`) refuses a foreign page with its 403.
    if req.peer.is_none_or(|peer| peer.is_loopback()) && (websocket(req) || !cross_origin(req)) {
        let public_host = host_of(&hub.lock().public_url);
        if trusted_host(req.header("host"), Some(&public_host)) {
            return true;
        }
    }
    let presented = req.header("authorization").and_then(|value| {
        let (scheme, token) = value.split_once(' ')?;
        scheme
            .eq_ignore_ascii_case("bearer")
            .then_some(token.trim())
    });
    if presented.is_some_and(|token| hub.owner_ok(token)) {
        return true;
    }
    session_of(req).is_some_and(|id| hub.session_ok(id, now_ms))
}

/// Whether a browser sent `req` from a page of another origin than the one it reached: `Sec-Fetch-Site` says
/// so, or `Origin` names another scheme, host or port than `Host` (an opaque origin sends `null`). The
/// console's own requests are same-origin; the CLI, the desktop app and the daemons send neither header.
fn cross_origin(req: &Request) -> bool {
    if req
        .header("sec-fetch-site")
        .is_some_and(|site| !matches!(site, "same-origin" | "none"))
    {
        return true;
    }
    let Some(origin) = req.header("origin") else {
        return false;
    };
    let authority = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    match (authority, req.header("host")) {
        (Some(authority), Some(host)) => !authority.eq_ignore_ascii_case(host),
        _ => true,
    }
}

/// Whether `req` asks to become a WebSocket (`Upgrade: websocket`).
fn websocket(req: &Request) -> bool {
    req.header("upgrade")
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// Whether a `Host` header names this machine, so a loopback peer may act as the owner. Only `localhost`,
/// the loopback addresses, `*.localhost` and the hub's own public host pass. A missing header (an HTTP/1.0
/// client sends none) keeps the loopback trust.
fn trusted_host(host: Option<&str>, public_host: Option<&str>) -> bool {
    let Some(host) = host else {
        return true;
    };
    let public = public_host
        .is_some_and(|public| !public.is_empty() && host_name(host).eq_ignore_ascii_case(public));
    apz_http::host_is_loopback(host) || public
}

/// The name in a `Host` value: the port dropped and IPv6 unbracketed (`[::1]:7700` is `::1`). A bare
/// IPv6 address has no port to drop.
fn host_name(host: &str) -> &str {
    if let Some(v6) = host.strip_prefix('[') {
        return v6.split(']').next().unwrap_or_default();
    }
    if host.matches(':').count() > 1 {
        return host;
    }
    host.split(':').next().unwrap_or_default()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/hub/identity", identity_route)
        .doc(RouteDoc::new("Show the hub's identity"))
        .post("/v1/hub/owner-token/rotate", rotate_route)
        .doc(RouteDoc::new("Rotate the owner token").errors(&[500]))
        .get("/v1/auth/check", |_, _, _| {
            Response::json(200, &serde_json::json!({ "ok": true }))
        })
        .doc(RouteDoc::new("Check that the session is signed in"))
        .post("/v1/auth/session", sign_in_route)
        .doc(RouteDoc::new("Sign in with the owner token").errors(&[400]))
        .delete("/v1/auth/session", sign_out_route)
        .doc(RouteDoc::new("Sign out of this browser"))
}

/// The session id in the request's `Cookie` header, if any.
fn session_of(req: &Request) -> Option<&str> {
    req.header("cookie")?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

#[derive(Deserialize)]
struct SignIn {
    token: String,
}

/// `POST /v1/auth/session {token}`: the owner token for a session cookie. One generic refusal.
fn sign_in_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let Ok(SignIn { token }) = req.json::<SignIn>() else {
        return Response::error(400, "send {\"token\": \"…\"}");
    };
    if !app.hub_auth.owner_ok(&token) {
        return Response::error(401, "that is not the owner token");
    }
    let id = app.hub_auth.open_session(apz_plaza::now_ms());
    let cookie = format!(
        "{SESSION_COOKIE}={id}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
        SESSION_MS / 1000
    );
    Response::json(200, &serde_json::json!({ "ok": true })).with_header("set-cookie", &cookie)
}

/// `DELETE /v1/auth/session`: ends this browser's session and clears its cookie.
fn sign_out_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    if let Some(id) = session_of(req) {
        app.hub_auth.close_session(id);
    }
    let cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0");
    Response::json(200, &serde_json::json!({ "ok": true })).with_header("set-cookie", &cookie)
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
    match app.hub_auth.rotate_owner_token() {
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
    let existing = std::fs::read_to_string(&cert_path)
        .ok()
        .zip(std::fs::read_to_string(&key_path).ok());
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
    let mut names = vec![
        host_of(public_url),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    names.retain(|name| !name.is_empty());
    names
}

/// The host of a URL such as `https://hub.example.org:7700/x` or `http://[::1]:7700`.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default().to_string(),
        None => host.split(':').next().unwrap_or_default().to_string(),
    }
}

fn owner_token_path(state_dir: &Path) -> PathBuf {
    state_dir.join(OWNER_TOKEN_FILE)
}

/// Writes `path` with mode 0600 (the owner token and the TLS key are private).
fn write_private(path: &Path, contents: &[u8]) -> Result<(), String> {
    apz_os::private::write(path, contents).map_err(|err| format!("{}: {err}", path.display()))
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
    use std::net::{IpAddr, Ipv4Addr};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-hub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn token_of(dir: &Path) -> String {
        std::fs::read_to_string(owner_token_path(dir)).unwrap()
    }

    /// An in-memory plaza.db with the `hub_config` table, as App::start opens it.
    fn store() -> apz_store::rusqlite::Connection {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        conn
    }

    /// A hub over a fresh store: [`Hub::new`] then [`Hub::load`].
    fn open_hub(dir: &Path, public_url: &str, tls: bool) -> Hub {
        let hub = Hub::new(dir, public_url, tls).unwrap();
        hub.load(&store()).unwrap();
        hub
    }

    #[test]
    fn sessions_open_check_close_expire_and_end_on_rotation() {
        let dir = scratch("sessions");
        let hub = open_hub(&dir, "http://127.0.0.1:7700", false);
        let now = 1_000_000;
        let a = hub.open_session(now);
        let b = hub.open_session(now);
        assert_ne!(a, b);
        assert!(hub.session_ok(&a, now + 1));
        assert!(!hub.session_ok("not-a-session", now));
        assert!(!hub.session_ok(&a, now + SESSION_MS));
        hub.close_session(&b);
        assert!(!hub.session_ok(&b, now + 1));
        let c = hub.open_session(now);
        hub.rotate_owner_token().unwrap();
        assert!(!hub.session_ok(&c, now + 1));
    }

    #[test]
    fn body_allowance_follows_the_caller_before_the_body_is_read() {
        let req = |path: &str, internal: Option<&str>| Request {
            method: "POST".into(),
            path: path.into(),
            headers: internal
                .map(|token| vec![("x-apz-internal".to_string(), token.to_string())])
                .unwrap_or_default(),
            ..Request::default()
        };
        let not_owner = || false;
        // The owner and the plaza's own daemons keep the server's limit.
        assert_eq!(
            allowance(&req("/v1/projects", None), "t0k", || true),
            usize::MAX
        );
        assert_eq!(
            allowance(&req("/internal/agent", Some("t0k")), "t0k", not_owner),
            usize::MAX
        );
        // The internal header counts only on /internal/ routes, and only with the right token.
        assert_eq!(
            allowance(&req("/internal/agent", Some("wrong")), "t0k", not_owner),
            PUBLIC_BODY
        );
        assert_eq!(
            allowance(&req("/v1/ingest/abc", Some("t0k")), "t0k", not_owner),
            PUBLIC_BODY
        );
        // A node's report may be large; every other public caller is small.
        assert_eq!(
            allowance(&req("/v1/nodes/n1/results", None), "t0k", not_owner),
            NODE_BODY
        );
        assert_eq!(
            allowance(&req("/v1/nodes/join", None), "t0k", not_owner),
            PUBLIC_BODY
        );
        assert_eq!(
            allowance(&req("/v1/ingest/abc", None), "t0k", not_owner),
            PUBLIC_BODY
        );
    }

    #[test]
    fn session_cookie_is_read_among_others_and_sign_in_is_public() {
        let req = |cookie: &str| Request {
            method: "GET".into(),
            path: "/v1/agents".into(),
            query: String::new(),
            headers: vec![("cookie".into(), cookie.into())],
            body: Vec::new(),
            peer: None,
        };
        assert_eq!(
            session_of(&req("theme=dark; apz_session=abc ; x=1")),
            Some("abc")
        );
        assert_eq!(session_of(&req("apz_session=")), None);
        assert_eq!(session_of(&req("apz_sessionx=abc")), None);
        assert!(is_public("POST", "/v1/auth/session"));
        assert!(is_public("DELETE", "/v1/auth/session"));
        assert!(!is_public("GET", "/v1/auth/check"));
        assert!(!is_public("GET", "/v1/nodes/n1/runs"));
    }

    #[test]
    fn a_browser_request_from_another_origin_gets_no_loopback_trust() {
        let dir = scratch("cross-origin");
        let hub = open_hub(&dir, "http://127.0.0.1:7700", false);
        let now = 1_000_000;
        let local = Some(IpAddr::from(Ipv4Addr::LOCALHOST));
        let post = |headers: &[(&str, &str)]| Request {
            method: "POST".into(),
            path: "/v1/settings".into(),
            headers: std::iter::once(("host", "127.0.0.1:7700"))
                .chain(headers.iter().copied())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            peer: local,
            ..Request::default()
        };
        // The console's own page, and callers that are not browsers, keep the machine's trust.
        assert!(is_owner_of(&hub, &post(&[]), now));
        assert!(is_owner_of(
            &hub,
            &post(&[
                ("origin", "http://127.0.0.1:7700"),
                ("sec-fetch-site", "same-origin")
            ]),
            now
        ));
        // Another site, another port of this machine, and a sandboxed frame's opaque origin do not.
        assert!(!is_owner_of(
            &hub,
            &post(&[("origin", "https://site.example")]),
            now
        ));
        assert!(!is_owner_of(
            &hub,
            &post(&[("origin", "http://127.0.0.1:7701")]),
            now
        ));
        assert!(!is_owner_of(&hub, &post(&[("origin", "null")]), now));
        assert!(!is_owner_of(
            &hub,
            &post(&[("sec-fetch-site", "cross-site")]),
            now
        ));
        // A WebSocket handshake passes the gate; the socket's Origin policy answers a foreign page with 403.
        assert!(is_owner_of(
            &hub,
            &post(&[("origin", "https://site.example"), ("upgrade", "websocket")]),
            now
        ));
        // The owner token still admits a caller wherever it comes from.
        let token = format!("Bearer {}", token_of(&dir));
        assert!(is_owner_of(
            &hub,
            &post(&[("origin", "null"), ("authorization", token.as_str())]),
            now
        ));
    }

    #[test]
    fn the_owner_is_the_machine_the_owner_token_or_a_console_session() {
        let dir = scratch("owner-of");
        let hub = open_hub(&dir, "http://127.0.0.1:7700", false);
        let token = token_of(&dir);
        let now = 1_000_000;
        let remote: IpAddr = Ipv4Addr::new(192, 0, 2, 1).into();
        let from = |peer: Option<IpAddr>, headers: Vec<(String, String)>| Request {
            method: "GET".into(),
            path: "/v1/acp".into(),
            headers,
            peer,
            ..Request::default()
        };
        assert!(is_owner_of(&hub, &from(None, vec![]), now));
        assert!(is_owner_of(
            &hub,
            &from(Some(Ipv4Addr::LOCALHOST.into()), vec![]),
            now
        ));
        assert!(!is_owner_of(&hub, &from(Some(remote), vec![]), now));
        let owner = vec![("authorization".into(), format!("Bearer {token}"))];
        assert!(is_owner_of(&hub, &from(Some(remote), owner), now));
        let wrong = vec![(
            "authorization".into(),
            "Bearer apz_mcp_not-the-owner".into(),
        )];
        assert!(!is_owner_of(&hub, &from(Some(remote), wrong), now));
        let session = hub.open_session(now);
        let cookie = vec![("cookie".into(), format!("{SESSION_COOKIE}={session}"))];
        assert!(is_owner_of(&hub, &from(Some(remote), cookie.clone()), now));
        assert!(!is_owner_of(
            &hub,
            &from(Some(remote), cookie),
            now + SESSION_MS
        ));
    }

    #[test]
    fn the_editors_socket_is_public_and_the_session_list_is_not() {
        assert!(is_public("GET", "/v1/acp"));
        assert!(!is_public("POST", "/v1/acp"));
        assert!(!is_public("GET", "/v1/acp/sessions"));
    }

    #[test]
    fn first_start_makes_the_owner_token_and_a_second_load_keeps_it() {
        let dir = scratch("first");
        let conn = store();
        let hub = Hub::new(&dir, "http://127.0.0.1:7700", false).unwrap();
        hub.load(&conn).unwrap();
        let token = token_of(&dir);
        assert!(token.starts_with(apz_trust::owner::PREFIX));
        assert!(hub.owner_ok(&token));
        // Unix: mode 0600. Windows has no mode bits (the profile's ACL keeps the files private).
        #[cfg(unix)]
        {
            assert_eq!(mode(&owner_token_path(&dir)), 0o600);
        }
        let first = hub.lock().owner_token_sha256.clone();
        apz_store::table::replace(&conn, def(), &hub.rows().unwrap()).unwrap();
        drop(hub);

        let again = Hub::new(&dir, "http://127.0.0.1:7700", false).unwrap();
        again.load(&conn).unwrap();
        assert_eq!(again.lock().owner_token_sha256, first);
        assert!(again.owner_ok(&token));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_token_is_refused() {
        let dir = scratch("other");
        let hub = open_hub(&dir, "http://127.0.0.1:7700", false);
        assert!(!hub.owner_ok("apzo_not-the-owner"));
        assert!(!hub.owner_ok(""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_invalidates_the_old_token() {
        let dir = scratch("rotate");
        let conn = store();
        let hub = Hub::new(&dir, "http://127.0.0.1:7700", false).unwrap();
        hub.load(&conn).unwrap();
        let old = token_of(&dir);
        let new = hub.rotate_owner_token().unwrap();
        assert_ne!(old, new);
        assert!(!hub.owner_ok(&old));
        assert!(hub.owner_ok(&new));
        assert_eq!(token_of(&dir), new);
        apz_store::table::replace(&conn, def(), &hub.rows().unwrap()).unwrap();
        drop(hub);

        let again = Hub::new(&dir, "http://127.0.0.1:7700", false).unwrap();
        again.load(&conn).unwrap();
        assert!(again.owner_ok(&new));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn public_routes_need_no_owner_token() {
        assert!(is_public("GET", "/v1/hub/identity"));
        assert!(is_public("POST", "/v1/nodes/join"));
        assert!(is_public("POST", "/v1/nodes/n3/heartbeat"));
        assert!(
            is_public("POST", "/v1/stations/join"),
            "a workstation joins with a code"
        );
        assert!(
            is_public("GET", "/v1/stations/link"),
            "a station's link checks its own token"
        );
        assert!(
            !is_public("GET", "/v1/stations"),
            "the registry is the owner's"
        );
        assert!(
            !is_public("POST", "/v1/stations/codes"),
            "a join code is the owner's"
        );
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

    #[test]
    fn only_a_local_or_the_public_host_is_the_machine() {
        let public = Some("hub.example.org");
        assert!(trusted_host(Some("localhost:7700"), None));
        assert!(trusted_host(Some("LocalHost"), None));
        assert!(trusted_host(Some("127.0.0.1:7700"), None));
        assert!(trusted_host(Some("[::1]:7700"), None));
        assert!(trusted_host(Some("::1"), None));
        assert!(trusted_host(Some("app.localhost:7700"), None));
        assert!(trusted_host(Some("hub.example.org:7700"), public));
        assert!(trusted_host(Some("HUB.example.org"), public));
        assert!(!trusted_host(Some("hub.example.org:7700"), None));
        assert!(!trusted_host(Some("evil.example:7700"), public));
        assert!(!trusted_host(Some("evil.localhost.example:7700"), None));
        assert!(!trusted_host(Some("localhost.evil.example"), None));
        assert!(!trusted_host(Some(".localhost"), None));
        assert!(!trusted_host(Some(""), None));
        assert!(!trusted_host(Some("evil.example:7700"), Some("")));
        assert!(trusted_host(None, None), "an HTTP/1.0 client sends no Host");
        assert!(trusted_host(None, public));
    }

    #[test]
    fn a_loopback_peer_under_a_foreign_host_is_not_the_owner() {
        let dir = scratch("rebind");
        let hub = open_hub(&dir, "https://hub.example.org:7700", false);
        let loopback = |host: Option<&str>| Request {
            method: "GET".into(),
            path: "/v1/agents".into(),
            headers: host
                .map(|host| vec![("host".into(), host.into())])
                .unwrap_or_default(),
            peer: Some(Ipv4Addr::LOCALHOST.into()),
            ..Request::default()
        };
        let now = 1_000_000;
        assert!(is_owner_of(&hub, &loopback(Some("127.0.0.1:7700")), now));
        assert!(is_owner_of(
            &hub,
            &loopback(Some("hub.example.org:7700")),
            now
        ));
        assert!(is_owner_of(&hub, &loopback(None), now));
        assert!(!is_owner_of(
            &hub,
            &loopback(Some("evil.example:7700")),
            now
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
