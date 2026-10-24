//! The owner's routes for MCP servers (docs/design/mcp.md, "Routes"). Owner auth; agents never reach them.

use std::time::Instant;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Serialize;
use serde_json::{Value, json};

use super::client::{ServerInfo, ToolInfo};
use super::config::{self, McpServer, Setting, StoreError};
use super::runtime::{Ctx, LogLine, Status};
use super::{McpSecrets, OnBehalf, VaultSecrets, auto_risk, risk_for, wire_names};
use crate::state::App;

/// The node a test or an owner's start runs for when the request names none (as the vault does).
const DEFAULT_NODE: &str = "n0";

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/mcp/catalog", |_, _, _| {
            let target = super::runtimes::host_target().unwrap_or("none");
            let presets: Vec<Value> = super::catalog::PRESETS
                .iter()
                .map(|preset| preset.view(target))
                .collect();
            Response::json(200, &presets)
        })
        .doc(RouteDoc::new("List the MCP server presets"))
        .get("/v1/mcp/servers", list_route)
        .doc(RouteDoc::new("List the MCP servers"))
        .post("/v1/mcp/servers", create_route)
        .doc(RouteDoc::new("Add an MCP server").errors(&[400, 404, 409]))
        .get("/v1/mcp/servers/{id}", get_route)
        .doc(RouteDoc::new("Show an MCP server").errors(&[404]))
        .route("PATCH", "/v1/mcp/servers/{id}", patch_route)
        .doc(RouteDoc::new("Change an MCP server's settings").errors(&[400, 404, 409]))
        .delete("/v1/mcp/servers/{id}", delete_route)
        .doc(
            RouteDoc::new("Delete an MCP server")
                .status(204)
                .errors(&[404, 500]),
        )
        .post("/v1/mcp/servers/{id}/test", test_route)
        .doc(
            RouteDoc::new("Test an MCP server and list its tools")
                .outbound()
                .errors(&[404]),
        )
        .post("/v1/mcp/servers/{id}/start", start_route)
        .doc(RouteDoc::new("Start an MCP server").errors(&[404]))
        .post("/v1/mcp/servers/{id}/stop", stop_route)
        .doc(RouteDoc::new("Stop an MCP server").errors(&[404]))
        .get("/v1/mcp/servers/{id}/tools", tools_route)
        .doc(RouteDoc::new("List an MCP server's tools").errors(&[404]))
        .get("/v1/mcp/servers/{id}/logs", logs_route)
        .doc(RouteDoc::new("Read an MCP server's recent logs").errors(&[404]))
}

/// A server as the console shows it: its config, and its status over its instances.
#[derive(Serialize)]
pub struct ServerView {
    #[serde(flatten)]
    pub server: McpServer,
    pub status: Status,
    pub tools_count: usize,
    pub last_error: Option<String>,
    /// How the server runs: "vm", or "sandbox: <why>". Empty before its first start.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub launcher: String,
    pub last_used_ms: u64,
    pub server_info: Option<ServerInfo>,
}

pub fn view(app: &App, server: McpServer) -> ServerView {
    let summary = app.mcp.runtime.summary(&server.id);
    ServerView {
        tools_count: app.mcp.store.tools_of(&server.id).len(),
        status: summary.status,
        last_error: summary.last_error,
        launcher: summary.launcher,
        last_used_ms: summary.last_used_ms,
        server_info: summary.server_info,
        server,
    }
}

fn error(status: u16, message: &str, field: Option<&str>) -> Response {
    Response::json(status, &json!({"error": message, "field": field}))
}

/// Tells the vault which secret each of the server's settings wants under `mcp:<id>`, so the console can offer it.
fn register_bindings(app: &App, server: &McpServer) {
    let binding = format!("mcp:{}", server.id);
    let label = format!("MCP server {}", server.name);
    for setting in server.env.values().chain(server.headers.values()) {
        if let Setting::Secret { secret, .. } = setting {
            app.vault.register_consumer(&binding, &label, secret);
        }
    }
}

fn changed(app: &App, id: &str) {
    app.bus.publish("mcp.changed", &json!({"id": id}));
}

fn server_or_404(app: &App, id: &str) -> Result<McpServer, Response> {
    app.mcp
        .store
        .get(id)
        .ok_or_else(|| error(404, &format!("no MCP server `{id}`"), None))
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let views: Vec<ServerView> = app
        .mcp
        .store
        .list()
        .into_iter()
        .map(|s| view(app, s))
        .collect();
    Response::json(200, &views)
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match server_or_404(app, p.get("id")) {
        Ok(server) => Response::json(200, &view(app, server)),
        Err(response) => response,
    }
}

/// The field a body names when it gives a stdio server a container image: the image is refused, and the refusal
/// names `transport.image` (a parse error would not name a field).
fn refused_image(body: &Value) -> Option<&'static str> {
    let transport = body.get("transport")?;
    let stdio = transport.get("kind").and_then(Value::as_str) == Some("stdio");
    (stdio && transport.get("image").is_some()).then_some("transport.image")
}

fn create_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    if let Ok(body) = req.json::<Value>()
        && let Some(field) = refused_image(&body)
    {
        return error(
            400,
            &format!("bad config: {}", config::IMAGE_REFUSED),
            Some(field),
        );
    }
    let mut server: McpServer = match req.json() {
        Ok(server) => server,
        Err(e) => return error(400, &format!("bad config: {e}"), None),
    };
    let now = apz_plaza::now_ms();
    server.created_ms = now;
    server.updated_ms = now;
    match app.mcp.store.insert(server) {
        Ok(server) => {
            register_bindings(app, &server);
            changed(app, &server.id);
            Response::json(200, &view(app, server))
        }
        Err(err) => store_error(err),
    }
}

fn patch_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let current = match server_or_404(app, id) {
        Ok(server) => server,
        Err(response) => return response,
    };
    let Ok(Value::Object(patch)) = req.json::<Value>() else {
        return error(
            400,
            "the body must be a JSON object of the fields to change",
            None,
        );
    };
    let mut merged = serde_json::to_value(&current).unwrap_or(Value::Null);
    for (key, value) in patch {
        match key.as_str() {
            "id" if value.as_str() != Some(id) => {
                return error(400, "id: an id cannot change", Some("id"));
            }
            "created_ms" | "updated_ms" => {}
            _ => {
                merged[key.as_str()] = value;
            }
        }
    }
    if let Some(field) = refused_image(&merged) {
        return error(
            400,
            &format!("bad config: {}", config::IMAGE_REFUSED),
            Some(field),
        );
    }
    let mut next: McpServer = match serde_json::from_value(merged) {
        Ok(server) => server,
        Err(e) => return error(400, &format!("bad config: {e}"), None),
    };
    next.created_ms = current.created_ms;
    next.updated_ms = apz_plaza::now_ms();
    // A change that alters what runs stops the instances: the next use starts them with the new config.
    let runs_differently = next.transport != current.transport
        || next.env != current.env
        || next.headers != current.headers
        || next.allow_private != current.allow_private
        || next.storage != current.storage
        || !next.enabled;
    match app.mcp.store.replace(id, next) {
        Ok(server) => {
            if runs_differently {
                app.mcp.runtime.stop(id);
            }
            register_bindings(app, &server);
            changed(app, id);
            Response::json(200, &view(app, server))
        }
        Err(err) => store_error(err),
    }
}

/// Deletes a server: its instances stop, its vault bindings `mcp:<id>` are removed, and its data stays unless
/// `?purge=1` asks for it.
fn delete_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let server = match server_or_404(app, id) {
        Ok(server) => server,
        Err(response) => return response,
    };
    app.mcp.runtime.stop(id);
    app.mcp.store.remove(id);
    if let Err(why) = app
        .vault
        .remove_binding(&format!("mcp:{id}"), "owner", apz_plaza::now_ms())
    {
        eprintln!(
            "apid: cannot remove the vault bindings of MCP server {id}: {}",
            why.message()
        );
    }
    changed(app, id);
    if req.query_param("purge") == Some("1")
        && server.storage.volume
        && let Err(why) = app.mcp.runtime.purge_data(id)
    {
        return error(
            500,
            &format!("the server is deleted, but its data was not purged: {why}"),
            None,
        );
    }
    Response::new(204, "application/json", Vec::new())
}

/// The node a request names, or the default.
fn node_of(req: &Request) -> String {
    req.json::<Value>()
        .ok()
        .and_then(|body| body.get("node").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| DEFAULT_NODE.to_string())
}

fn owner_ctx<'a>(app: &App, secrets: &'a dyn McpSecrets, node: String) -> Ctx<'a> {
    super::ctx(
        app,
        secrets,
        OnBehalf::Owner {
            node: Some(node.clone()),
        },
        node,
    )
}

fn test_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let server = match server_or_404(app, id) {
        Ok(server) => server,
        Err(response) => return response,
    };
    let node = node_of(req);
    let was_running = app.mcp.runtime.summary(id).status == Status::Running;
    let started = Instant::now();
    let secrets = VaultSecrets::new(app);
    let outcome = app
        .mcp
        .runtime
        .probe(&server, &owner_ctx(app, &secrets, node));
    let ms = started.elapsed().as_millis() as u64;
    if !was_running {
        app.mcp.runtime.stop(id);
    }
    match outcome {
        Ok(probe) => {
            app.mcp.store.set_tools(id, probe.tools.clone());
            changed(app, id);
            let tools = tool_rows(&server, &probe.tools);
            Response::json(
                200,
                &json!({
                    "ok": true,
                    "server_info": probe.server_info,
                    "tools": tools,
                    "resources": probe.resources,
                    "prompts": probe.prompts,
                    "ms": ms,
                }),
            )
        }
        Err(why) => Response::json(
            200,
            &json!({"ok": false, "error": why, "ms": ms, "stderr": tail(app, id, 20)}),
        ),
    }
}

fn start_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let server = match server_or_404(app, id) {
        Ok(server) => server,
        Err(response) => return response,
    };
    app.mcp.runtime.reset(id);
    let secrets = VaultSecrets::new(app);
    match app
        .mcp
        .runtime
        .probe(&server, &owner_ctx(app, &secrets, node_of(req)))
    {
        Ok(probe) => app.mcp.store.set_tools(id, probe.tools),
        Err(why) => eprintln!("apid: MCP server {id} did not start: {why}"),
    }
    let view = view(app, server);
    app.bus
        .publish("mcp.status", &json!({"id": id, "status": view.status}));
    Response::json(200, &view)
}

fn stop_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let server = match server_or_404(app, id) {
        Ok(server) => server,
        Err(response) => return response,
    };
    app.mcp.runtime.stop(id);
    let view = view(app, server);
    app.bus
        .publish("mcp.status", &json!({"id": id, "status": view.status}));
    Response::json(200, &view)
}

fn tools_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let server = match server_or_404(app, p.get("id")) {
        Ok(server) => server,
        Err(response) => return response,
    };
    let rows = tool_rows(&server, &app.mcp.store.tools_of(&server.id));
    Response::json(200, &rows)
}

fn logs_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if server_or_404(app, id).is_err() {
        return error(404, &format!("no MCP server `{id}`"), None);
    }
    let limit = req
        .query_param("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(100)
        .min(500);
    Response::json(200, &app.mcp.runtime.logs(id, limit))
}

/// The tools as the owner's views show them: `agent_name` is the name agents see, `auto_risk` the risk the
/// annotations give, `risk` the effective risk (after the owner's override), and `allowed` whether the
/// allow list offers the tool. `tool` repeats `agent_name` for the earlier console.
pub fn tool_rows(server: &McpServer, tools: &[ToolInfo]) -> Vec<Value> {
    let names = wire_names(&server.id, tools);
    tools
        .iter()
        .zip(names)
        .map(|(t, wire)| {
            json!({
                "name": t.name,
                "tool": wire,
                "agent_name": wire,
                "description": t.description,
                "input_schema": t.input_schema,
                "annotations": t.annotations,
                "auto_risk": auto_risk(t).as_str(),
                "risk": risk_for(server, t).as_str(),
                "allowed": server.tools.allow.admits(&t.name),
            })
        })
        .collect()
}

fn tail(app: &App, id: &str, n: usize) -> Vec<LogLine> {
    app.mcp.runtime.logs(id, n)
}

fn store_error(err: StoreError) -> Response {
    match err {
        StoreError::Invalid(invalid) => error(400, &invalid.to_string(), Some(&invalid.field)),
        StoreError::Duplicate(id) => error(
            409,
            &format!("an MCP server `{id}` already exists"),
            Some("id"),
        ),
        StoreError::Missing => error(404, "no such MCP server", None),
        StoreError::Full => error(
            400,
            &format!("at most {} MCP servers", config::MAX_SERVERS),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stdio_server_with_an_image_is_refused_at_transport_image() {
        let stdio_image = json!({"transport": {"kind": "stdio", "image": "bad image"}});
        assert_eq!(refused_image(&stdio_image), Some("transport.image"));
        let http = json!({"transport": {"kind": "http", "url": "https://example.com/mcp"}});
        assert_eq!(refused_image(&http), None);
        let package = json!({"transport": {"kind": "stdio", "package": {"kind": "npm"}}});
        assert_eq!(refused_image(&package), None);
        assert_eq!(refused_image(&json!({"id": "x"})), None);
    }
}
