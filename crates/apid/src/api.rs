//! The router: every module's routes, the proxies to noded and the hub, the
//! overview and the console's static files.

use std::collections::HashSet;
use std::path::{Component, Path};

use apz_http::{Response, Router};
use serde_json::{json, Map, Value};

use crate::agents::Status;
use crate::state::App;
use crate::{agents, artifacts, events, internal, plaza, providers, settings};

pub fn router() -> Router<App> {
    let router = Router::<App>::new()
        .get("/v1/overview", |app, _, _| overview(app))
        .get("/v1/machines", |app, _, _| machines(app))
        .get("/v1/machine", |app, _, _| app.noded.proxy("GET", "/v1/machine", b""))
        .get("/v1/hub", |app, _, _| app.hub.proxy("GET", "/v1/status", b""))
        .get("/v1/runs", |app, _, _| app.noded.proxy("GET", "/v1/runs", b""))
        .post("/v1/runs", |app, req, _| app.noded.proxy("POST", "/v1/runs", &req.body))
        .get("/v1/runs/{id}", |app, _, p| app.noded.proxy("GET", &format!("/v1/runs/{}", p.get("id")), b""))
        .delete("/v1/runs/{id}", |app, _, p| app.noded.proxy("DELETE", &format!("/v1/runs/{}", p.get("id")), b""))
        .post("/v1/runs/{id}/input", |app, req, p| {
            app.noded.proxy("POST", &format!("/v1/runs/{}/input", p.get("id")), &req.body)
        });
    let router = events::routes(router);
    let router = settings::routes(router);
    let router = providers::routes(router);
    let router = agents::routes(router);
    let router = plaza::routes(router);
    let router = internal::routes(router);
    let router = artifacts::routes(router);
    router
        .get("/", |app, _, _| static_file(&app.config.console, "index.html"))
        .get("/{file}", |app, _, p| static_file(&app.config.console, p.get("file")))
        .get("/assets/{file}", |app, _, p| static_file(&app.config.console, &format!("assets/{}", p.get("file"))))
}

/// `GET /v1/machines`: the machines this plaza runs on. Today the one local
/// node: its facts from noded, the runs on it and how many agents are in them.
fn machines(app: &App) -> Response {
    let unreachable = || Response::json(200, &json!([{"id": "local", "reachable": false}]));
    let Ok((200, machine)) = app.noded.get("/v1/machine") else {
        return unreachable();
    };
    let runs = match app.noded.get("/v1/runs") {
        Ok((200, body)) => body["runs"].clone(),
        _ => json!([]),
    };
    let agents = {
        let run_ids: HashSet<&str> = runs.as_array().into_iter().flatten().filter_map(|run| run["id"].as_str()).collect();
        app.agents.list().iter().filter(|a| a.run.as_deref().is_some_and(|id| run_ids.contains(id))).count()
    };
    let mut entry: Map<String, Value> = machine.as_object().cloned().unwrap_or_default();
    entry.insert("id".into(), json!("local"));
    entry.insert("noded_port".into(), json!(app.noded.port));
    entry.insert("runs".into(), runs);
    entry.insert("agents".into(), json!(agents));
    Response::json(200, &json!([entry]))
}

/// `GET /v1/overview`: counts and health at a glance.
fn overview(app: &App) -> Response {
    let agents = app.agents.list();
    let count = |s: Status| agents.iter().filter(|a| a.status == s).count();
    let mut tasks = serde_json::Map::new();
    for task in app.plaza.tasks.list() {
        let n = tasks.entry(task.state().to_string()).or_insert(json!(0));
        *n = json!(n.as_u64().unwrap_or(0) + 1);
    }
    let settings = app.settings.get();
    Response::json(
        200,
        &json!({
            "agents": {
                "total": agents.len(),
                "idle": count(Status::Idle),
                "working": count(Status::Working),
                "offline": count(Status::Offline),
            },
            "tasks": tasks,
            "default_model": settings.default_model,
            "orchestrator_model": settings.orchestrator_model,
            "dev_models": app.settings.dev_models(),
            "noded": app.noded.get("/v1/machine").map(|(s, _)| s == 200).unwrap_or(false),
            "hub": app.hub.get("/healthz").map(|(s, _)| s == 200).unwrap_or(false),
        }),
    )
}

/// One file under the console directory (no `..`, no absolute paths).
fn static_file(root: &Path, name: &str) -> Response {
    let rel = Path::new(name);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Response::error(404, "not found");
    }
    let path = root.join(rel);
    match std::fs::read(&path) {
        Ok(bytes) => Response::new(200, content_type(&path), bytes).with_header("cache-control", "no-cache"),
        Err(_) => Response::error(404, "not found"),
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => apz_http::JSON,
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
