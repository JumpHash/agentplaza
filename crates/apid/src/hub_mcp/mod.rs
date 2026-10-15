//! The hub as an MCP server, for clients outside the plaza (docs/design/hub-mcp.md).
//!
//! `POST /mcp` speaks JSON-RPC 2.0 over Streamable HTTP, in its JSON-response form. The server is off until the
//! owner enables it. In `secure` mode (the default) every request carries a bearer token (`apz_mcp_...`) whose scope
//! and allowlist decide which tools it sees and may call. In `open` mode a request with no token acts at the scope
//! the owner chose for open mode. Each call is rate limited (per token, or per client IP when anonymous) and written
//! to the audit log.
//!
//! The owner API, the database tables and the console come next. Until they exist, the methods only they will
//! call (minting, revoking, listing) are unused outside the tests, so dead code is allowed here.

#![allow(dead_code)]

pub mod audit;
pub mod auth;
pub mod owner;
pub mod rpc;
pub mod session;
pub mod store_db;
#[cfg(test)]
mod tests;
pub mod tools;

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use apz_http::{Request, Response, Router};
use serde_json::{Value, json};

use crate::db::Db;
use crate::state::App;
use audit::{Audit, CallRecord, MemoryAudit};
use auth::{
    AuthError, MemoryTokens, RateLimiter, Scope, TokenRecord, TokenStore, bearer, sha256_hex,
};
use session::Sessions;
use tools::{Ctx, Plaza, Tool};

/// The header that carries the session ID.
const SESSION_HEADER: &str = "Mcp-Session-Id";
/// The clock, in milliseconds. Injected so tests can move it.
pub type Clock = Box<dyn Fn() -> u64 + Send + Sync>;

/// The server's state: the switch, the allowed origins, the auth mode, the token store, the audit log, sessions and
/// limits.
pub struct HubMcp {
    enabled: AtomicBool,
    /// The origins the owner added. A request with no Origin header is never refused by this check.
    origins: RwLock<Vec<String>>,
    /// The hub's own origins, always allowed. Fixed when the server starts.
    defaults: Vec<String>,
    /// Whether the listener serves TLS. Every connection on it is TLS, or none is.
    tls: bool,
    bind: RwLock<Bind>,
    mode: RwLock<AuthMode>,
    open_scope: RwLock<Scope>,
    pub tokens: Box<dyn TokenStore>,
    pub audit: Box<dyn Audit>,
    sessions: Sessions,
    /// Shared with the editors' socket (`acp`), so a token's calls count one window whichever surface it uses.
    pub limiter: RateLimiter,
    clock: Clock,
}

/// Who a request acts for. A token's call uses the token's record. An anonymous call (open mode) uses a record of the
/// open scope with no allowlist and the ID `anon:<ip>`, so the tool checks and the rate limit read it like a token's.
struct Caller {
    record: TokenRecord,
    anonymous: bool,
    ip: Option<String>,
}

impl HubMcp {
    pub fn new(
        enabled: bool,
        origins: Vec<String>,
        tokens: Box<dyn TokenStore>,
        audit: Box<dyn Audit>,
        clock: Clock,
    ) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            origins: RwLock::new(origins),
            defaults: Vec::new(),
            tls: false,
            bind: RwLock::new(Bind::Local),
            mode: RwLock::new(AuthMode::Secure),
            open_scope: RwLock::new(Scope::Observe),
            tokens,
            audit,
            sessions: Sessions::default(),
            limiter: RateLimiter::default(),
            clock,
        }
    }

    /// Says how the listener is served: over TLS or not, and the hub's own origins.
    pub fn listening(mut self, tls: bool, defaults: Vec<String>) -> Self {
        self.tls = tls;
        self.defaults = defaults;
        self
    }

    /// Whether the listener serves TLS.
    pub fn listens_tls(&self) -> bool {
        self.tls
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
    }

    /// Whether a request from this peer reaches the server at all. Loopback always does. Any other peer does only
    /// when the owner chose `remote` and the listener serves TLS. A request with no known peer does not.
    pub fn reachable(&self, peer: Option<IpAddr>) -> bool {
        match peer {
            Some(ip) if ip.is_loopback() => true,
            Some(_) => *read(&self.bind) == Bind::Remote && self.tls,
            None => false,
        }
    }

    /// Whether a request carrying this Origin may call the server: the hub's own origins, or the owner's.
    fn origin_allowed(&self, origin: &str) -> bool {
        self.defaults.iter().any(|allowed| allowed == origin)
            || read(&self.origins).iter().any(|allowed| allowed == origin)
    }

    /// The origins the server allows now: the hub's own, then the owner's that are not among them.
    pub fn effective_origins(&self) -> Vec<String> {
        let mut out = self.defaults.clone();
        for origin in read(&self.origins).iter() {
            if !out.contains(origin) {
                out.push(origin.clone());
            }
        }
        out
    }

    /// The settings as the server runs them now.
    pub fn settings(&self) -> Settings {
        Settings {
            enabled: self.enabled(),
            bind: *read(&self.bind),
            allowed_origins: read(&self.origins).clone(),
            auth_mode: *read(&self.mode),
            open_scope: *read(&self.open_scope),
        }
    }

    /// Runs the server with these settings: the switch, the bind, the origins, the auth mode and the open scope.
    pub fn apply(&self, next: &Settings) {
        self.set_enabled(next.enabled);
        *write(&self.bind) = next.bind;
        *write(&self.origins) = next.allowed_origins.clone();
        *write(&self.mode) = next.auth_mode;
        *write(&self.open_scope) = next.open_scope;
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// Who a request acts for. A presented bearer token must be live, in both modes: a bad one is refused. With no
    /// token, `secure` refuses and `open` gives an anonymous caller the open scope, counted per client IP.
    fn caller(&self, req: &Request, now: u64) -> Result<Caller, AuthError> {
        match bearer(req) {
            Some(token) => Ok(Caller {
                record: self.tokens.verify(token, now)?,
                anonymous: false,
                ip: None,
            }),
            None if *read(&self.mode) == AuthMode::Secure => Err(AuthError::Missing),
            None => {
                let ip = req.peer.map(|ip| ip.to_string()).unwrap_or_default();
                let record = anonymous_record(format!("anon:{ip}"), *read(&self.open_scope), now);
                Ok(Caller {
                    record,
                    anonymous: true,
                    ip: Some(ip),
                })
            }
        }
    }
}

fn anonymous_record(id: String, scope: Scope, now: u64) -> TokenRecord {
    TokenRecord {
        id,
        name: "anonymous".into(),
        scope,
        allowlist: Vec::new(),
        created_ms: now,
        expires_ms: None,
        last_used_ms: None,
        calls: 0,
        revoked_at_ms: None,
    }
}

/// Where the server listens: on this machine only, or on the network over TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bind {
    Local,
    Remote,
}

impl Bind {
    pub fn as_str(self) -> &'static str {
        match self {
            Bind::Local => "local",
            Bind::Remote => "remote",
        }
    }

    pub fn parse(text: &str) -> Option<Bind> {
        match text {
            "local" => Some(Bind::Local),
            "remote" => Some(Bind::Remote),
            _ => None,
        }
    }
}

/// Whether a request must carry a token (`secure`, the default) or may come without one (`open`, the owner's choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Secure,
    Open,
}

impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::Secure => "secure",
            AuthMode::Open => "open",
        }
    }

    pub fn parse(text: &str) -> Option<AuthMode> {
        match text {
            "secure" => Some(AuthMode::Secure),
            "open" => Some(AuthMode::Open),
            _ => None,
        }
    }
}

/// The owner's settings for the server: the `hub_mcp_settings` row when there is a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub bind: Bind,
    /// The origins the owner added, on top of the hub's own.
    pub allowed_origins: Vec<String>,
    pub auth_mode: AuthMode,
    /// The scope an anonymous call gets in open mode.
    pub open_scope: Scope,
}

impl Settings {
    /// The server switched off, secure, local, with no origins of the owner's.
    fn off() -> Settings {
        Settings {
            enabled: false,
            bind: Bind::Local,
            allowed_origins: Vec::new(),
            auth_mode: AuthMode::Secure,
            open_scope: Scope::Observe,
        }
    }
}

/// The origins the hub allows whatever the owner's list says: its own loopback names on its port, and the origin of
/// its public URL. The scheme is the listener's.
pub fn default_origins(port: u16, public_url: &str, tls: bool) -> Vec<String> {
    let scheme = if tls { "https" } else { "http" };
    let mut origins = vec![
        format!("{scheme}://127.0.0.1:{port}"),
        format!("{scheme}://localhost:{port}"),
        format!("{scheme}://[::1]:{port}"),
    ];
    if let Some(origin) = origin_of(public_url) {
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    origins
}

/// A URL's origin: its scheme and host, with no path.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split('/').next().filter(|host| !host.is_empty())?;
    Some(format!("{scheme}://{host}"))
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

impl HubMcp {
    /// The server at start. With a database, its settings and its token and audit stores come from there; a settings
    /// row that cannot be read leaves the server off. Without one, the server is off and its stores are in memory
    /// (lost at restart). `defaults` are the hub's own origins; `tls` says whether the listener serves TLS.
    pub fn open(defaults: Vec<String>, tls: bool, db: Option<Arc<Db>>) -> HubMcp {
        let clock: Clock = Box::new(apz_plaza::now_ms);
        let (settings, tokens, audit): (Settings, Box<dyn TokenStore>, Box<dyn Audit>) = match db {
            Some(db) => {
                let settings = db.hub_mcp_settings().unwrap_or_else(|err| {
                    eprintln!("hub-mcp: settings not read, the server stays off: {err}");
                    Settings::off()
                });
                (
                    settings,
                    Box::new(store_db::DbTokens::new(Arc::clone(&db))),
                    Box::new(store_db::DbAudit::new(db)),
                )
            }
            None => (
                Settings::off(),
                Box::new(MemoryTokens::default()),
                Box::new(MemoryAudit::default()),
            ),
        };
        let hub = HubMcp::new(false, Vec::new(), tokens, audit, clock).listening(tls, defaults);
        hub.apply(&settings);
        hub
    }
}

/// What the routes need from the host: this server's state, and the plaza's read model.
pub trait Host {
    fn hub_mcp(&self) -> &HubMcp;
    fn plaza(&self) -> &dyn Plaza;
}

impl Host for App {
    fn hub_mcp(&self) -> &HubMcp {
        &self.hub_mcp
    }

    fn plaza(&self) -> &dyn Plaza {
        self
    }
}

/// `POST /mcp` and `GET /mcp`.
pub fn routes<S: Host>(router: Router<S>) -> Router<S> {
    router
        .post("/mcp", |state, req, _| {
            handle_post(state.hub_mcp(), state.plaza(), req)
        })
        .get("/mcp", |state, req, _| {
            if state.hub_mcp().enabled() && state.hub_mcp().reachable(req.peer) {
                Response::error(405, "this endpoint accepts POST").with_header("Allow", "POST")
            } else {
                Response::error(404, "no such endpoint")
            }
        })
}

/// Answers one `POST /mcp`: a single message or a batch.
pub fn handle_post(hub: &HubMcp, plaza: &dyn Plaza, req: &Request) -> Response {
    if !hub.enabled() || !hub.reachable(req.peer) {
        return Response::error(404, "no such endpoint");
    }
    if let Some(origin) = req.header("origin") {
        if !hub.origin_allowed(origin) {
            return Response::error(403, "this origin may not call the hub's MCP server");
        }
    }
    let now = hub.now();
    let caller = match hub.caller(req, now) {
        Ok(caller) => caller,
        Err(err) => return unauthorized(err),
    };
    let token = &caller.record;
    let body: Value = match serde_json::from_slice(&req.body) {
        Ok(body) => body,
        Err(_) => {
            return Response::json(
                200,
                &rpc::failure(&Value::Null, rpc::PARSE_ERROR, "parse error", None),
            );
        }
    };
    let batch = body.is_array();
    let messages: Vec<Value> = match body {
        Value::Array(items) => items,
        single => vec![single],
    };
    if batch && messages.is_empty() {
        return Response::json(
            200,
            &rpc::failure(&Value::Null, rpc::INVALID_REQUEST, "empty batch", None),
        );
    }
    let initializing = messages.len() == 1
        && messages[0].get("method").and_then(Value::as_str) == Some("initialize");
    if !initializing {
        let Some(session) = req.header("mcp-session-id") else {
            return Response::error(
                400,
                "send initialize first: the Mcp-Session-Id header is missing",
            );
        };
        if hub.sessions.touch(session, &token.id, now).is_err() {
            return Response::error(404, "unknown or expired session: initialize again");
        }
    }
    let mut answers: Vec<Value> = Vec::new();
    let mut opened: Option<String> = None;
    for message in &messages {
        match rpc::classify(message) {
            rpc::Message::Invalid => answers.push(rpc::failure(
                &Value::Null,
                rpc::INVALID_REQUEST,
                "not a JSON-RPC 2.0 request",
                None,
            )),
            rpc::Message::Notification => {}
            rpc::Message::Request { id, method, params } if method == "initialize" => {
                if batch {
                    answers.push(rpc::failure(
                        &id,
                        rpc::INVALID_REQUEST,
                        "initialize cannot be batched",
                        None,
                    ));
                } else {
                    match hub.sessions.open(&token.id, now) {
                        Ok(session) => {
                            opened = Some(session);
                            answers.push(rpc::result(&id, initialize_result(&params)));
                        }
                        Err(why) => {
                            answers.push(rpc::failure(&id, rpc::INTERNAL_ERROR, &why, None))
                        }
                    }
                }
            }
            rpc::Message::Request { id, method, params } => {
                answers.push(dispatch(hub, plaza, &caller, &id, method, &params, now));
            }
        }
    }
    if answers.is_empty() {
        return Response::new(202, "text/plain; charset=utf-8", Vec::<u8>::new());
    }
    let reply = if batch {
        Response::json(200, &answers)
    } else {
        Response::json(200, &answers[0])
    };
    match opened {
        Some(session) => reply.with_header(SESSION_HEADER, &session),
        None => reply,
    }
}

fn unauthorized(err: AuthError) -> Response {
    Response::json(401, &json!({ "error": err.message() }))
        .with_header("WWW-Authenticate", "Bearer")
}

/// The answer to `initialize`: the negotiated version, and what this server offers (tools only).
fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    json!({
        "protocolVersion": rpc::negotiate(requested),
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "agentplaza", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "AgentPlaza's hub: the tools this token's scope and allowlist permit, for one plaza.",
    })
}

fn dispatch(
    hub: &HubMcp,
    plaza: &dyn Plaza,
    caller: &Caller,
    id: &Value,
    method: &str,
    params: &Value,
    now: u64,
) -> Value {
    match method {
        "ping" => rpc::result(id, json!({})),
        "tools/list" => {
            let tools: Vec<Value> = tools::visible(tools::REGISTRY, &caller.record)
                .into_iter()
                .map(|tool| descriptor(tool))
                .collect();
            rpc::result(id, json!({ "tools": tools }))
        }
        "tools/call" => call_tool(hub, plaza, caller, id, params, now),
        _ => rpc::failure(
            id,
            rpc::METHOD_NOT_FOUND,
            &format!("method not found: {method}"),
            None,
        ),
    }
}

fn descriptor(tool: &dyn Tool) -> Value {
    json!({
        "name": tool.name(),
        "description": tool.description(),
        "inputSchema": tool.input_schema(),
        "outputSchema": tool.output_schema(),
        "annotations": { "readOnlyHint": tool.scope() == Scope::Observe },
    })
}

/// `tools/call`: checks the tool and the rate limit, runs it, and audits the call either way.
fn call_tool(
    hub: &HubMcp,
    plaza: &dyn Plaza,
    caller: &Caller,
    id: &Value,
    params: &Value,
    now: u64,
) -> Value {
    let token = &caller.record;
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return rpc::failure(
            id,
            rpc::INVALID_PARAMS,
            "tools/call needs a tool name",
            None,
        );
    };
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let digest = sha256_hex(args.to_string().as_bytes());
    let (ok, error, reply): (bool, Option<&'static str>, Value) = match tools::find(name)
        .filter(|tool| tools::allowed(token, *tool))
    {
        None => (
            false,
            Some("unknown_tool"),
            rpc::failure(
                id,
                rpc::INVALID_PARAMS,
                &format!("unknown tool: {name}"),
                None,
            ),
        ),
        Some(tool) => match hub.limiter.take(&token.id, token.scope, now) {
            Err(retry_after) => (
                false,
                Some("rate_limited"),
                rpc::failure(
                    id,
                    rpc::RATE_LIMITED,
                    "rate limit exceeded",
                    Some(json!({ "retryAfter": retry_after })),
                ),
            ),
            Ok(()) => {
                if !caller.anonymous {
                    hub.tokens.record_use(&token.id, now);
                }
                match tool.call(&Ctx { plaza }, &args) {
                    Ok(out) => {
                        let text = tool.summary(&out);
                        let result = json!({
                            "content": [{ "type": "text", "text": text }],
                            "structuredContent": out,
                            "isError": false,
                        });
                        (true, None, rpc::result(id, result))
                    }
                    Err(message) => {
                        let result = json!({ "content": [{ "type": "text", "text": message }], "isError": true });
                        (false, Some("tool_error"), rpc::result(id, result))
                    }
                }
            }
        },
    };
    hub.audit.record(CallRecord {
        at_ms: now,
        token_id: (!caller.anonymous).then(|| token.id.clone()),
        client_ip: if caller.anonymous {
            caller.ip.clone()
        } else {
            None
        },
        tool: name.chars().take(64).collect(),
        args_sha256: digest,
        ok,
        error: error.map(str::to_string),
        duration_ms: hub.now().saturating_sub(now),
    });
    reply
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn the_hubs_own_origins_come_first_and_the_public_url_adds_one() {
        let local = default_origins(19080, "http://127.0.0.1:19080", false);
        assert_eq!(
            local,
            vec![
                "http://127.0.0.1:19080",
                "http://localhost:19080",
                "http://[::1]:19080"
            ]
        );
        let public = default_origins(19080, "https://hub.example/", true);
        assert_eq!(public[3], "https://hub.example");
        assert_eq!(
            public[0], "https://127.0.0.1:19080",
            "the listener's scheme, over TLS"
        );
    }
}
