//! The router: every module's routes, the proxies to noded and the hub, the
//! overview and the console's static files.

use std::collections::HashSet;
use std::path::{Component, Path};

use apz_http::{Response, RouteDoc, Router};
use serde_json::{Map, Value, json};

use crate::agents::Status;
use crate::state::App;
use crate::{agents, decisions, events, internal, plaza, providers, settings};

pub fn router() -> Router<App> {
    let router = Router::<App>::new()
        .get("/v1/overview", |app, _, _| {
            Response::json(200, &overview_value(app))
        })
        .doc(RouteDoc::new("Read the hub's overview"))
        .get("/v1/machines", |app, _, _| machines(app))
        .doc(RouteDoc::new("List the machines on this plaza"))
        .get("/v1/machine", |app, _, _| {
            app.noded.proxy("GET", "/v1/machine", b"")
        })
        .doc(RouteDoc::new("Read this machine's facts"))
        .get("/v1/hub", |app, _, _| {
            app.hub.proxy("GET", "/v1/status", b"")
        })
        .doc(RouteDoc::new("Read the hub's status"))
        .get("/v1/runs", |app, _, _| {
            app.noded.proxy("GET", "/v1/runs", b"")
        })
        .doc(RouteDoc::new("List the runs on this machine"))
        .post("/v1/runs", |app, req, _| {
            app.noded.proxy("POST", "/v1/runs", &req.body)
        })
        .doc(RouteDoc::new("Start a run"))
        .get("/v1/runs/{id}", |app, _, p| {
            app.noded
                .proxy("GET", &format!("/v1/runs/{}", p.get("id")), b"")
        })
        .doc(RouteDoc::new("Read a run"))
        .delete("/v1/runs/{id}", |app, _, p| {
            app.noded
                .proxy("DELETE", &format!("/v1/runs/{}", p.get("id")), b"")
        })
        .doc(RouteDoc::new("Delete a run"))
        .post("/v1/runs/{id}/input", |app, req, p| {
            app.noded.proxy(
                "POST",
                &format!("/v1/runs/{}/input", p.get("id")),
                &req.body,
            )
        })
        .doc(RouteDoc::new("Send input to a run"))
        // The hub machine's shared data volumes: read-only images agents mount at /data/<name>.
        .get("/v1/volumes", |app, _, _| {
            app.noded.proxy("GET", "/v1/volumes", b"")
        })
        .doc(RouteDoc::new("List the data volumes"))
        .post("/v1/volumes", |app, req, _| {
            app.noded.proxy("POST", "/v1/volumes", &req.body)
        })
        .doc(RouteDoc::new("Create a data volume"))
        .delete("/v1/volumes/{name}", |app, _, p| {
            app.noded
                .proxy("DELETE", &format!("/v1/volumes/{}", p.get("name")), b"")
        })
        .doc(RouteDoc::new("Delete a data volume"))
        // The hub machine's warm templates (new agents and sandboxes restore one instead of booting).
        .get("/v1/templates", |app, _, _| {
            app.noded.proxy("GET", "/v1/templates", b"")
        })
        .doc(RouteDoc::new("List the warm templates"))
        .delete("/v1/templates/{key}", |app, _, p| {
            app.noded
                .proxy("DELETE", &format!("/v1/templates/{}", p.get("key")), b"")
        })
        .doc(RouteDoc::new("Delete a warm template"));
    let router = events::routes(router);
    let router = crate::work_routes::routes(router);
    let router = crate::acp::routes(router);
    let router = settings::routes(router);
    let router = providers::routes(router);
    let router = crate::engine_instances::routes(router);
    let router = crate::context_track::routes(router);
    let router = crate::usage_providers::routes(router);
    let router = crate::evals::routes(router);
    let router = decisions::routes(router);
    let router = agents::routes(router);
    let router = plaza::routes(router);
    let router = crate::owner_board::routes(router);
    let router = internal::routes(router);
    let router = crate::connectors::routes(router);
    let router = crate::mcp::routes::routes(router);
    let router = crate::llm_proxy::routes(router);
    let router = crate::vendor_relay::routes(router);
    let router = crate::approvals::routes(router);
    let router = crate::vault::routes::routes(router);
    let router = crate::hub_mcp::owner::routes(router);
    let router = crate::library::routes(router);
    let router = crate::deliverables::routes(router);
    let router = crate::storage::routes(router);
    let router = crate::toolbox::routes(router);
    let router = crate::adopt::routes(router);
    let router = crate::projects::routes(router);
    let router = crate::project_memory::routes(router);
    let router = crate::maintenance::routes(router);
    let router = crate::environments::routes(router);
    let router = crate::project_files::routes(router);
    let router = crate::drafts::routes(router);
    let router = crate::project_settings::routes(router);
    let router = crate::harness_control::routes(router);
    let router = crate::appearance::routes(router);
    let router = crate::artifacts::routes::routes(router);
    let router = crate::content::routes(router);
    let router = crate::artifacts::gateway::routes(router);
    let router = crate::shares::routes::routes(router);
    let router = crate::previews::route::routes(router);
    let router = crate::shares::public::routes(router);
    let router = crate::shares::request::routes(router);
    let router = crate::weft_hub::routes(router);
    let router = crate::platforms::routes(router);
    let router = crate::review_graph::routes::routes(router);
    let router = crate::recipes::routes::routes(router);
    let router = crate::releases::routes::routes(router);
    let router = crate::triggers::routes::routes(router);
    let router = crate::unfurl::routes(router);
    let router = crate::ide::routes(router);
    let router = crate::ide_term::routes(router);
    let router = crate::rules::routes(router);
    let router = crate::standing_rules::routes(router);
    let router = crate::intake::routes(router);
    let router = crate::control::routes(router);
    let router = crate::lifecycle::routes(router);
    let router = crate::sleep::routes(router);
    let router = crate::metrics::prom::routes(router);
    let router = crate::placement::routes(router);
    let router = crate::targets::routes(router);
    let router = crate::vm_snapshots::routes(router);
    let router = crate::e2b::routes(router);
    let router = crate::egress_inject::routes(router);
    let router = crate::roles_custom::routes(router);
    let router = crate::agent_proposals::routes(router);
    let router = crate::nodes::routes(router);
    let router = crate::test_lab::routes(router);
    let router = crate::lab_observe::routes(router);
    let router = crate::topology::routes(router);
    let router = crate::mover::routes(router);
    let router = crate::transfer::routes(router);
    let router = crate::stations::routes(router);
    let router = crate::prices::routes(router);
    let router = crate::usage::routes(router);
    let router = crate::metrics::routes::routes(router);
    let router = crate::fleet::routes(router);
    let router = crate::node_link::routes(router);
    let router = crate::hub_auth::routes(router);
    let router = crate::hub_mcp::routes(router);
    let router = crate::prompts::routes::routes(router);
    // The import, GitHub and export paths first: `/v1/skills/export` must not match `/v1/skills/{id}`.
    let router = crate::skills::import::routes(router);
    let router = crate::skills::routes::routes(router);
    let router = crate::foundry::routes::routes(router);
    let router = crate::foundry::registry::routes(router);
    router
        .get("/", |app, req, _| start_page(app, req))
        .doc(RouteDoc::new("Serve the console's start page").errors(&[404]))
        .get("/{file}", |app, req, p| match p.get("file") {
            "index.html" => start_page(app, req),
            file => static_file(&app.config.console, file, accepts(req)),
        })
        .doc(RouteDoc::new("Serve a console file").errors(&[404]))
        .get("/assets/{file}", |app, req, p| {
            static_file(
                &app.config.console,
                &format!("assets/{}", p.get("file")),
                accepts(req),
            )
        })
        .doc(RouteDoc::new("Serve a console asset").errors(&[404]))
}

/// The hub's machine id in `/v1/machines` and in `/v1/nodes` (`n0`, see `nodes::views`), so a client
/// joins a node to its machine by id.
const HUB_MACHINE_ID: &str = "n0";

/// `GET /v1/machines`: the machines this plaza runs on. Today the one local
/// node: its facts from noded, the runs on it and how many agents are in them.
fn machines(app: &App) -> Response {
    // Without noded the hub's kind is unknown (noded probes it), so it is null and a client labels it generically.
    let unreachable = || {
        Response::json(
            200,
            &json!([{"id": HUB_MACHINE_ID, "name": "local", "kind": null, "reachable": false}]),
        )
    };
    // The latest probe round (probes.rs): a slow or stopped noded does not hold the list.
    let probed = crate::probes::latest(app);
    let Some(machine) = probed.machine.clone() else {
        return unreachable();
    };
    let runs = match app.noded.get("/v1/runs") {
        Ok((200, body)) => body["runs"].clone(),
        _ => json!([]),
    };
    let agents = {
        let run_ids: HashSet<&str> = runs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|run| run["id"].as_str())
            .collect();
        app.agents
            .list()
            .iter()
            .filter(|a| a.run.as_deref().is_some_and(|id| run_ids.contains(id)))
            .count()
    };
    let mut entry: Map<String, Value> = machine.as_object().cloned().unwrap_or_default();
    entry.insert("id".into(), json!(HUB_MACHINE_ID));
    entry.insert("noded_port".into(), json!(app.noded.port));
    entry.insert("runs".into(), runs);
    entry.insert("agents".into(), json!(agents));
    entry.insert("stats".into(), probed.stats.clone().unwrap_or(Value::Null));
    Response::json(200, &json!([entry]))
}

/// `GET /v1/overview`: counts and health at a glance.
pub(crate) fn overview_value(app: &App) -> Value {
    let probed = crate::probes::latest(app);
    let agents = app.agents.list();
    let count = |s: Status| agents.iter().filter(|a| a.status == s).count();
    let mut tasks = serde_json::Map::new();
    for task in app.plaza.tasks.list() {
        let n = tasks.entry(task.state().to_string()).or_insert(json!(0));
        *n = json!(n.as_u64().unwrap_or(0) + 1);
    }
    let settings = app.settings.get();
    json!({
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
        "noded": probed.noded_up(),
        "hub": probed.hub_up,
    })
}

/// The encodings a request accepts, from its `Accept-Encoding` (q=0 means refused).
fn accepts(req: &apz_http::Request) -> Vec<String> {
    req.header("accept-encoding")
        .unwrap_or("")
        .split(',')
        .filter_map(|part| {
            let mut bits = part.split(';').map(str::trim);
            let name = bits.next()?.to_ascii_lowercase();
            let refused =
                bits.any(|b| b.strip_prefix("q=").and_then(|q| q.parse::<f32>().ok()) == Some(0.0));
            (!name.is_empty() && !refused).then_some(name)
        })
        .collect()
}

/// One file under the console directory (no `..`, no absolute paths). When the build wrote a
/// precompressed sibling (`.br`, `.gz`, see tools/apz-pack) the request accepts, that is served
/// instead with its `Content-Encoding`. Content-hashed names never change, so they are cached for
/// a year; everything else is revalidated.
fn static_file(root: &Path, name: &str, accepts: Vec<String>) -> Response {
    let rel = Path::new(name);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Response::error(404, "not found");
    }
    let path = root.join(rel);
    let cache = if is_fingerprinted(name) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    for (encoding, suffix) in [("br", "br"), ("gzip", "gz")] {
        if !accepts.iter().any(|a| a == encoding) {
            continue;
        }
        let mut packed = path.as_os_str().to_owned();
        packed.push(".");
        packed.push(suffix);
        if let Ok(bytes) = std::fs::read(&packed) {
            return Response::new(200, content_type(&path), bytes)
                .with_header("content-encoding", encoding)
                .with_header("vary", "accept-encoding")
                .with_header("cache-control", cache);
        }
    }
    match std::fs::read(&path) {
        Ok(bytes) => Response::new(200, content_type(&path), bytes)
            .with_header("vary", "accept-encoding")
            .with_header("cache-control", cache),
        Err(_) => Response::error(404, "not found"),
    }
}

/// The console's start page, with the content origin its HTML panels load from allowed as a frame source.
fn start_page(app: &App, req: &apz_http::Request) -> Response {
    match std::fs::read_to_string(app.config.console.join("index.html")) {
        Ok(page) => {
            let origin = crate::content::frame_origin(app, req);
            Response::new(
                200,
                "text/html; charset=utf-8",
                crate::content::console_page(&page, origin.as_deref()).into_bytes(),
            )
            .with_header("cache-control", "no-cache")
        }
        Err(_) => Response::error(404, "not found"),
    }
}

/// Build outputs named `<stem>-<hash>.<ext>`: esbuild's `main-I6T7NTXS.js` (8 base32
/// characters) and apz-pack's `apz_engine_web-1a2b3c4d.js` (8 hex digits).
fn is_fingerprinted(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    let stem = file.split('.').next().unwrap_or("");
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-').is_some_and(|(head, hash)| {
        let base32 = hash
            .bytes()
            .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b));
        let hex = hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        !head.is_empty() && hash.len() == 8 && (base32 || hex)
    })
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
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        // The backdrop shader and its WebGL2 translation (apz-pack web-backdrop).
        Some("wgsl" | "glsl") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod static_tests {
    use super::*;

    fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
        resp.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn precompressed_siblings_are_served_when_accepted() {
        let dir = std::env::temp_dir().join(format!("apid-static-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("apz_engine_web-1a2b3c4d_bg.wasm"), b"plain").unwrap();
        std::fs::write(dir.join("apz_engine_web-1a2b3c4d_bg.wasm.br"), b"br").unwrap();
        std::fs::write(dir.join("index.html"), b"<p>").unwrap();
        let name = "apz_engine_web-1a2b3c4d_bg.wasm";

        let br = static_file(&dir, name, vec!["gzip".into(), "br".into()]);
        assert_eq!(br.bytes(), b"br");
        assert_eq!(header(&br, "content-encoding"), Some("br"));
        assert_eq!(header(&br, "content-type"), Some("application/wasm"));
        assert!(header(&br, "cache-control").unwrap().contains("immutable"));

        // gzip accepted but no .gz written: the plain bytes, unencoded.
        let plain = static_file(&dir, name, vec!["gzip".into()]);
        assert_eq!(plain.bytes(), b"plain");
        assert_eq!(header(&plain, "content-encoding"), None);

        let html = static_file(&dir, "index.html", vec!["br".into()]);
        assert_eq!(header(&html, "cache-control"), Some("no-cache"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn accept_encoding_parsing_drops_refused_encodings() {
        let req = apz_http::Request {
            headers: vec![(
                "accept-encoding".into(),
                "gzip, deflate, br;q=0, zstd".into(),
            )],
            ..Default::default()
        };
        assert_eq!(accepts(&req), vec!["gzip", "deflate", "zstd"]);
    }

    #[test]
    fn fingerprinted_names() {
        assert!(is_fingerprinted("main-I6T7NTXS.js"));
        assert!(is_fingerprinted("apz_engine_web-1a2b3c4d_bg.wasm"));
        assert!(is_fingerprinted("assets/main-WE5MHQGG.css"));
        assert!(!is_fingerprinted("index.html"));
        assert!(!is_fingerprinted("inter-var.ttf"));
        assert!(!is_fingerprinted("SourceSerif4-Semibold.ttf"));
    }
}
