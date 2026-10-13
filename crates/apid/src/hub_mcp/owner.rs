//! The owner's API for the hub's MCP server (docs/design/hub-mcp.md, "Hub API"): the status and settings, the
//! tokens, and the call log. Every `/v1` path is behind `hub_auth::gate`, which asks for the owner's token, so
//! only the owner reaches these handlers. A token's value is in the answer to its creation, and nowhere else.

use apz_http::{Request, Response, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::audit::CallRecord;
use super::auth::{NewToken, Scope, TokenRecord};
use super::{tools, AuthMode, Bind, HubMcp, Settings};
use crate::db::Db;
use crate::state::App;

const DAY_MS: u64 = 86_400_000;
/// The longest a token may be set to live, in days.
const MAX_EXPIRY_DAYS: u64 = 3_650;

/// What the owner routes need: the server, the database that keeps its settings, and the hub's public URL.
pub struct Ctx<'a> {
    pub hub: &'a HubMcp,
    pub db: &'a Db,
    pub public_url: &'a str,
}

fn ctx(app: &App) -> Ctx<'_> {
    Ctx { hub: &app.hub_mcp, db: &app.db, public_url: &app.config.public_url }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/hub-mcp", |app, _, _| status(&ctx(app)))
        .route("PUT", "/v1/hub-mcp", |app, req, _| put_settings(&ctx(app), req))
        .get("/v1/hub-mcp/tokens", |app, _, _| list_tokens(&ctx(app)))
        .post("/v1/hub-mcp/tokens", |app, req, _| create_token(&ctx(app), req))
        .delete("/v1/hub-mcp/tokens/{id}", |app, _, p| revoke_token(&ctx(app), p.get("id")))
        .get("/v1/hub-mcp/calls", |app, req, _| calls(&ctx(app), req))
}

/// `GET /v1/hub-mcp`: the switch, the bind, the auth mode, the origins, the URL to use, and the live token count.
pub fn status(ctx: &Ctx) -> Response {
    let settings = ctx.hub.settings();
    let live = ctx.hub.tokens.list().iter().filter(|token| token.revoked_at_ms.is_none()).count();
    let warning = (settings.auth_mode == AuthMode::Open).then(|| {
        let reach = if settings.bind == Bind::Remote { "from any network that reaches this hub" } else { "from this machine only" };
        format!("open mode: a call with no token gets the {} scope, {reach}", settings.open_scope.as_str())
    });
    Response::json(
        200,
        &json!({
            "enabled": settings.enabled,
            "bind": settings.bind.as_str(),
            "auth_mode": settings.auth_mode.as_str(),
            "open_scope": settings.open_scope.as_str(),
            "warning": warning,
            "tls": ctx.hub.listens_tls(),
            "allowed_origins": settings.allowed_origins,
            "effective_origins": ctx.hub.effective_origins(),
            "url": format!("{}/mcp", ctx.public_url.trim_end_matches('/')),
            "tokens": live,
            "stored": ctx.db.enabled(),
        }),
    )
}

#[derive(Debug, Deserialize)]
pub struct SettingsBody {
    pub enabled: bool,
    pub bind: String,
    pub allowed_origins: Vec<String>,
    /// `secure` when omitted: the safe mode.
    #[serde(default = "secure_text")]
    pub auth_mode: String,
    /// `observe` when omitted.
    #[serde(default = "observe_text")]
    pub open_scope: String,
    /// Required with `open` over `remote`: the owner has read what anyone on the network may then call.
    #[serde(default)]
    pub confirm_open_remote: bool,
}

fn secure_text() -> String {
    "secure".into()
}

fn observe_text() -> String {
    "observe".into()
}

/// `PUT /v1/hub-mcp {enabled, bind, auth_mode, open_scope, allowed_origins, confirm_open_remote?}`.
pub fn put_settings(ctx: &Ctx, req: &Request) -> Response {
    match req.json::<SettingsBody>() {
        Ok(body) => apply_settings(ctx, body),
        Err(_) => Response::error(400, "the body is {enabled, bind, auth_mode, open_scope, allowed_origins, confirm_open_remote?}"),
    }
}

/// Saves the settings (to the database first, when there is one) and runs the server with them at once. A refused
/// body changes nothing.
pub fn apply_settings(ctx: &Ctx, body: SettingsBody) -> Response {
    let Some(bind) = Bind::parse(&body.bind) else {
        return Response::error(400, "bind is local or remote");
    };
    let Some(auth_mode) = AuthMode::parse(&body.auth_mode) else {
        return Response::error(400, "auth_mode is secure or open");
    };
    let Some(open_scope) = Scope::parse(&body.open_scope) else {
        return Response::error(400, "open_scope is observe, operate or govern");
    };
    if let Some(bad) = body.allowed_origins.iter().find(|origin| !valid_origin(origin)) {
        return Response::error(400, &format!("{bad} is not an origin: a scheme and a host, with no path"));
    }
    if bind == Bind::Remote && !ctx.hub.listens_tls() {
        return Response::error(400, "the hub is not serving TLS; remote MCP needs it");
    }
    if auth_mode == AuthMode::Open && bind == Bind::Remote && !body.confirm_open_remote {
        return Response::error(
            400,
            "open over remote lets anyone on the network call the tools open_scope allows: send confirm_open_remote true to allow it",
        );
    }
    let next = Settings { enabled: body.enabled, bind, allowed_origins: body.allowed_origins, auth_mode, open_scope };
    if ctx.db.enabled() {
        if let Err(err) = ctx.db.hub_mcp_settings_put(&next) {
            return Response::error(500, &format!("the settings are not saved: {err}"));
        }
    }
    ctx.hub.apply(&next);
    status(ctx)
}

/// An origin: `http://` or `https://`, then a host, and nothing after it.
fn valid_origin(origin: &str) -> bool {
    let rest = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://"));
    rest.is_some_and(|host| !host.is_empty() && !host.contains(['/', ' ']))
}

/// The record of a token as the API shows it: never the token itself.
pub fn token_json(record: &TokenRecord) -> Value {
    json!({
        "id": record.id,
        "name": record.name,
        "scope": record.scope.as_str(),
        "tools": record.allowlist,
        "created_ms": record.created_ms,
        "expires_ms": record.expires_ms,
        "last_used_ms": record.last_used_ms,
        "calls": record.calls,
        "revoked_at_ms": record.revoked_at_ms,
    })
}

/// `GET /v1/hub-mcp/tokens`.
pub fn list_tokens(ctx: &Ctx) -> Response {
    let tokens: Vec<Value> = ctx.hub.tokens.list().iter().map(token_json).collect();
    Response::json(200, &json!({ "tokens": tokens }))
}

#[derive(Debug, Deserialize)]
pub struct TokenBody {
    pub name: String,
    pub scope: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub expires_days: Option<u64>,
}

fn create_token(ctx: &Ctx, req: &Request) -> Response {
    match req.json::<TokenBody>() {
        Ok(body) => issue(ctx, body, apz_plaza::now_ms()),
        Err(_) => Response::error(400, "the body is {name, scope, tools?, expires_days?}"),
    }
}

/// `POST /v1/hub-mcp/tokens`: mints a token. The answer carries the token once, with its record.
pub fn issue(ctx: &Ctx, body: TokenBody, now: u64) -> Response {
    let Some(scope) = Scope::parse(&body.scope) else {
        return Response::error(400, "scope is observe, operate or govern");
    };
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 80 {
        return Response::error(400, "name is 1 to 80 characters");
    }
    for tool in &body.tools {
        if tools::find(tool).filter(|found| scope.permits(found.scope())).is_none() {
            return Response::error(400, &format!("{tool} is not a tool of scope {}", scope.as_str()));
        }
    }
    let expires_ms = match body.expires_days {
        None => None,
        Some(days) if (1..=MAX_EXPIRY_DAYS).contains(&days) => Some(now + days * DAY_MS),
        Some(_) => return Response::error(400, "expires_days is 1 to 3650"),
    };
    match ctx.hub.tokens.create(NewToken { name, scope, allowlist: body.tools, expires_ms }, now) {
        Ok((token, record)) => Response::json(201, &json!({ "token": token, "record": token_json(&record) })),
        Err(err) => Response::error(500, &format!("the token is not created: {err}")),
    }
}

/// `DELETE /v1/hub-mcp/tokens/{id}`: 204 when revoked, 404 when no live token has the id.
pub fn revoke_token(ctx: &Ctx, id: &str) -> Response {
    if ctx.hub.tokens.revoke(id, apz_plaza::now_ms()) {
        Response::new(204, "text/plain; charset=utf-8", Vec::<u8>::new())
    } else {
        Response::error(404, "no live token has that id")
    }
}

/// The call log's filters: a token id, a tool name and a row limit (1 to 500, default 100).
#[derive(Debug, Default)]
pub struct CallsQuery {
    pub token: Option<String>,
    pub tool: Option<String>,
    pub limit: Option<usize>,
}

/// `GET /v1/hub-mcp/calls?token=&tool=&limit=`.
fn calls(ctx: &Ctx, req: &Request) -> Response {
    let limit = match req.query_param("limit") {
        None => None,
        Some(text) => match text.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => return Response::error(400, "limit is a number"),
        },
    };
    let query = CallsQuery {
        token: req.query_param("token").map(str::to_string),
        tool: req.query_param("tool").map(str::to_string),
        limit,
    };
    list_calls(ctx, query)
}

pub fn list_calls(ctx: &Ctx, query: CallsQuery) -> Response {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let rows: Vec<Value> = ctx
        .hub
        .audit
        .query(query.token.as_deref(), query.tool.as_deref(), limit)
        .iter()
        .map(call_json)
        .collect();
    Response::json(200, &json!({ "calls": rows }))
}

fn call_json(call: &CallRecord) -> Value {
    json!({
        "token_id": call.token_id,
        "client_ip": call.client_ip,
        "tool": call.tool,
        "args_sha256": call.args_sha256,
        "ok": call.ok,
        "error": call.error,
        "duration_ms": call.duration_ms,
        "at_ms": call.at_ms,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::hub_mcp::audit::MemoryAudit;
    use crate::hub_mcp::auth::{AuthError, MemoryTokens};

    const T0: u64 = 1_800_000_000_000;
    const URL: &str = "https://hub.test";

    fn hub() -> HubMcp {
        HubMcp::new(false, vec![], Box::new(MemoryTokens::default()), Box::new(MemoryAudit::default()), Box::new(|| T0)).listening(true, Vec::new())
    }

    fn test_ctx<'a>(hub: &'a HubMcp, db: &'a Db) -> Ctx<'a> {
        Ctx { hub, db, public_url: URL }
    }

    fn text(resp: &Response) -> String {
        String::from_utf8_lossy(resp.bytes()).into_owned()
    }

    fn body(resp: &Response) -> Value {
        serde_json::from_slice(resp.bytes()).unwrap_or(Value::Null)
    }

    fn token_body(name: &str, scope: &str, tools: &[&str], days: Option<u64>) -> TokenBody {
        TokenBody {
            name: name.into(),
            scope: scope.into(),
            tools: tools.iter().map(|tool| tool.to_string()).collect(),
            expires_days: days,
        }
    }

    #[test]
    fn every_owner_path_sits_behind_the_owner_gate() {
        let paths = [
            ("GET", "/v1/hub-mcp"),
            ("PUT", "/v1/hub-mcp"),
            ("GET", "/v1/hub-mcp/tokens"),
            ("POST", "/v1/hub-mcp/tokens"),
            ("DELETE", "/v1/hub-mcp/tokens/mcp_0123456789ab"),
            ("GET", "/v1/hub-mcp/calls"),
        ];
        for (method, path) in paths {
            assert!(!crate::hub_auth::is_public(method, path), "{method} {path} must need the owner token");
        }
        assert!(crate::hub_auth::is_public("POST", "/mcp"), "/mcp sits outside /v1: its own bearer token guards it");
    }

    #[test]
    fn the_token_is_shown_once_and_never_listed() {
        let (hub, db) = (hub(), Db::connect(&BTreeMap::new()));
        let ctx = test_ctx(&hub, &db);
        let created = issue(&ctx, token_body("laptop", "operate", &[], Some(30)), T0);
        assert_eq!(created.status, 201);
        let token = body(&created)["token"].as_str().expect("the token").to_string();
        assert!(token.starts_with("apz_mcp_"));
        assert_eq!(body(&created)["record"]["expires_ms"].as_u64(), Some(T0 + 30 * DAY_MS));
        let id = body(&created)["record"]["id"].as_str().expect("the id").to_string();

        let listing = text(&list_tokens(&ctx));
        assert!(!listing.contains(&token), "the listing never carries a token");
        assert!(listing.contains("laptop") && listing.contains(&id));
        assert_eq!(hub.tokens.verify(&token, T0).map(|record| record.name), Ok("laptop".to_string()));

        assert_eq!(revoke_token(&ctx, &id).status, 204);
        assert_eq!(revoke_token(&ctx, &id).status, 404, "a second revoke finds nothing live");
        assert_eq!(hub.tokens.verify(&token, T0), Err(AuthError::Revoked));
    }

    #[test]
    fn bad_input_is_refused_and_mints_nothing() {
        let (hub, db) = (hub(), Db::connect(&BTreeMap::new()));
        let ctx = test_ctx(&hub, &db);
        assert_eq!(issue(&ctx, token_body("x", "root", &[], None), T0).status, 400);
        assert_eq!(issue(&ctx, token_body("x", "observe", &["no_such_tool"], None), T0).status, 400);
        assert_eq!(issue(&ctx, token_body("x", "observe", &[], Some(0)), T0).status, 400);
        assert_eq!(issue(&ctx, token_body("  ", "observe", &[], None), T0).status, 400);
        assert!(hub.tokens.list().is_empty());
    }

    #[test]
    fn settings_switch_the_server_and_bad_settings_change_nothing() {
        let (hub, db) = (hub(), Db::connect(&BTreeMap::new()));
        let ctx = test_ctx(&hub, &db);
        assert!(!hub.enabled());
        let on = SettingsBody { enabled: true, bind: "remote".into(), allowed_origins: vec!["https://console.test".into()], auth_mode: "secure".into(), open_scope: "observe".into(), confirm_open_remote: false };
        assert_eq!(apply_settings(&ctx, on).status, 200);
        assert!(hub.enabled());
        assert_eq!(hub.settings().bind, Bind::Remote);
        let bad_bind = SettingsBody { enabled: false, bind: "everywhere".into(), allowed_origins: vec![], auth_mode: "secure".into(), open_scope: "observe".into(), confirm_open_remote: false };
        assert_eq!(apply_settings(&ctx, bad_bind).status, 400);
        let bad_origin = SettingsBody { enabled: false, bind: "local".into(), allowed_origins: vec!["console.test/".into()], auth_mode: "secure".into(), open_scope: "observe".into(), confirm_open_remote: false };
        assert_eq!(apply_settings(&ctx, bad_origin).status, 400);
        assert!(hub.enabled(), "a refused body changes nothing");
        let off = SettingsBody { enabled: false, bind: "local".into(), allowed_origins: vec![], auth_mode: "secure".into(), open_scope: "observe".into(), confirm_open_remote: false };
        assert_eq!(apply_settings(&ctx, off).status, 200);
        assert!(!hub.enabled());
    }

    #[test]
    fn the_call_log_filters_by_token_and_tool() {
        let (hub, db) = (hub(), Db::connect(&BTreeMap::new()));
        let ctx = test_ctx(&hub, &db);
        let (_, record) = hub
            .tokens
            .create(NewToken { name: "reader".into(), scope: Scope::Observe, allowlist: vec![], expires_ms: None }, T0)
            .expect("create");
        for (at, tool) in [(T0 + 1, "hub_status"), (T0 + 2, "hub_agents")] {
            hub.audit.record(CallRecord {
                at_ms: at,
                token_id: Some(record.id.clone()), client_ip: None,
                tool: tool.into(),
                args_sha256: "0".repeat(64),
                ok: true,
                error: None,
                duration_ms: 1,
            });
        }
        let count = |resp: Response| body(&resp)["calls"].as_array().map_or(0, |rows| rows.len());
        assert_eq!(count(list_calls(&ctx, CallsQuery { token: Some(record.id.clone()), ..Default::default() })), 2);
        assert_eq!(count(list_calls(&ctx, CallsQuery { tool: Some("hub_agents".into()), ..Default::default() })), 1);
        assert_eq!(count(list_calls(&ctx, CallsQuery { token: Some("mcp_000000000000".into()), ..Default::default() })), 0);
    }
}
