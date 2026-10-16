//! The hub's OpenAPI 3.1 spec, read off its router (`apid openapi`).
//!
//! Every route the hub serves is in the spec: its method, path, path parameters and whether it needs
//! the owner token come from the router and [`hub_auth::is_public`], so they cannot drift. What a
//! route's [`RouteDoc`](apz_http::RouteDoc) adds (summary, request and response schemas, statuses) is
//! as good as the docs; a route without one is marked `x-undocumented`.
//!
//! `docs/api/openapi.json` is the committed spec. The tests below fail when it differs from what the
//! router gives (`APZ_BLESS=1 cargo test -p apid openapi` rewrites it), and when a route has no
//! doc.

use apz_http::{RouteInfo, Router};
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::hub_auth;

/// The spec of everything `router` serves.
pub fn spec<S>(router: &Router<S>) -> Value {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|settings| settings.definitions_path = "/components/schemas".into())
        .into_generator();
    let mut paths: Map<String, Value> = Map::new();
    let mut seen = std::collections::HashSet::new();
    for route in router.routes() {
        // The first of two identical routes is the one that answers.
        if !seen.insert((route.method, route.pattern)) {
            continue;
        }
        let entry = paths
            .entry(openapi_path(route.pattern))
            .or_insert_with(|| json!({}));
        entry[route.method.to_ascii_lowercase()] = operation(&route, &mut generator);
    }
    let mut schemas = generator.take_definitions(true);
    schemas.insert(
        "Error".into(),
        json!({
            "type": "object",
            "description": "Every error answer: what went wrong, in a sentence.",
            "properties": { "error": { "type": "string" } },
            "required": ["error"],
        }),
    );
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "AgentPlaza hub API",
            "description": "The hub's HTTP API (apid). Generated from its router by `apid openapi`: do not edit by hand.",
            "version": env!("CARGO_PKG_VERSION"),
            "license": { "name": "Apache-2.0", "identifier": "Apache-2.0" },
        },
        "servers": [{ "url": "http://127.0.0.1:7777", "description": "A hub on this machine" }],
        "security": [{ "owner": [] }],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "owner": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "The owner token, or a console session. Requests from this machine are the owner's.",
                },
            },
            "schemas": schemas,
        },
    })
}

/// The routes with no doc (or an empty summary).
pub fn undocumented<S>(router: &Router<S>) -> Vec<RouteInfo<'_>> {
    router
        .routes()
        .filter(|route| route.doc.is_none_or(|doc| doc.summary.is_empty()))
        .collect()
}

fn operation(route: &RouteInfo<'_>, generator: &mut schemars::SchemaGenerator) -> Value {
    let doc = route.doc.cloned().unwrap_or_default();
    let mut op = Map::new();
    op.insert("operationId".into(), operation_id(route).into());
    op.insert(
        "tags".into(),
        json!([doc.tag.unwrap_or_else(|| tag_of(route.pattern))]),
    );
    if !doc.summary.is_empty() {
        op.insert("summary".into(), doc.summary.into());
    }
    if !doc.description.is_empty() {
        op.insert("description".into(), doc.description.into());
    }
    let params: Vec<Value> = route
        .params()
        .map(|name| match name.strip_suffix("..") {
            Some(rest) => json!({
                "name": rest, "in": "path", "required": true, "schema": { "type": "string" },
                "description": "The rest of the path: it may contain slashes.",
            }),
            None => json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }),
        })
        .collect();
    if !params.is_empty() {
        op.insert("parameters".into(), params.into());
    }
    if let Some(request) = doc.request {
        op.insert(
            "requestBody".into(),
            json!({ "required": true, "content": { "application/json": { "schema": request(generator) } } }),
        );
    }
    let public = hub_auth::is_public(route.method, &sample_path(route.pattern));
    if public {
        op.insert("security".into(), json!([]));
    }

    let mut responses = Map::new();
    let status = doc.status.unwrap_or(200);
    let mut success = json!({ "description": success_text(status) });
    if doc.stream {
        success["content"] = json!({ "text/event-stream": { "schema": { "type": "string" } } });
    } else if let Some(response) = doc.response {
        success["content"] = json!({ "application/json": { "schema": response(generator) } });
    }
    responses.insert(status.to_string(), success);
    let error = |text: &str| {
        json!({
            "description": text,
            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } },
        })
    };
    if route.params().next().is_some() {
        responses.insert("400".into(), error("A path parameter is not valid UTF-8"));
    }
    if !public {
        responses.insert("401".into(), error("No owner token or session"));
    }
    for status in doc.errors {
        responses
            .entry(status.to_string())
            .or_insert_with(|| error(error_text(*status)));
    }
    op.insert("responses".into(), responses.into());
    if doc.outbound {
        op.insert("x-outbound".into(), true.into());
    }
    if route.doc.is_none_or(|doc| doc.summary.is_empty()) {
        op.insert("x-undocumented".into(), true.into());
    }
    op.into()
}

/// The pattern as an OpenAPI path: a rest-of-path parameter `{path..}` is `{path}` there (OpenAPI
/// has no such parameter; its description says it may hold slashes).
fn openapi_path(pattern: &str) -> String {
    pattern.replace("..}", "}")
}

/// `get_v1_agents_cid_tell` for `GET /v1/agents/{cid}/tell`.
fn operation_id(route: &RouteInfo<'_>) -> String {
    let mut id = route.method.to_ascii_lowercase();
    for segment in route.pattern.split('/').filter(|s| !s.is_empty()) {
        id.push('_');
        id.extend(
            segment
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }),
        );
    }
    id.replace("__", "_").trim_end_matches('_').to_string()
}

/// The group a route is listed under by default: its first segment after `/v1/`, else its first
/// segment (`internal`, `e2b`, `mcp`); the console's own files are `console`.
fn tag_of(pattern: &'static str) -> &'static str {
    let mut segments = pattern.split('/').filter(|s| !s.is_empty());
    match (segments.next(), segments.next()) {
        (Some("v1"), Some(first)) if !first.starts_with('{') => first,
        (Some("v1"), _) => "hub",
        (None | Some("assets"), _) => "console",
        (Some(first), _) if first.starts_with('{') => "console",
        (Some(first), _) => first,
    }
}

/// A concrete path for a pattern, to ask the auth rule about it: every `{name}` becomes `n1` (the auth
/// rule recognises a node's own routes by a node id, which looks like that).
fn sample_path(pattern: &str) -> String {
    pattern
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "n1"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn success_text(status: u16) -> &'static str {
    match status {
        201 => "Created",
        202 => "Accepted",
        204 => "No content",
        _ => "OK",
    }
}

fn error_text(status: u16) -> &'static str {
    match status {
        400 => "The request is not valid",
        403 => "Not allowed",
        404 => "Not found",
        405 => "Not a socket upgrade",
        409 => "Conflicts with the current state",
        411 => "A body without a length",
        413 => "Too large",
        422 => "Refused by a rule",
        429 => "Too many requests",
        500 => "The hub failed",
        502 => "An upstream failed",
        503 => "Not available now",
        504 => "An upstream timed out",
        _ => "An error",
    }
}

/// `apid openapi`: the hub's spec on stdout, and how much of it is documented on stderr.
pub fn print() {
    let router = crate::api::router();
    println!(
        "{}",
        serde_json::to_string_pretty(&spec(&router)).expect("a spec serializes")
    );
    eprintln!(
        "apid: {} routes, {} without docs",
        router.routes().count(),
        undocumented(&router).len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMITTED: &str = "../../docs/api/openapi.json";

    #[test]
    fn the_committed_spec_is_the_routers() {
        let router = crate::api::router();
        let fresh = serde_json::to_string_pretty(&spec(&router)).unwrap() + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(COMMITTED);
        if std::env::var_os("APZ_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &fresh).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == fresh,
            "docs/api/openapi.json is not what the router serves: regenerate it with \
             `APZ_BLESS=1 cargo test -p apid openapi` and commit the change"
        );
    }

    #[test]
    fn every_route_is_documented() {
        let router = crate::api::router();
        let missing = undocumented(&router);
        assert!(
            missing.is_empty(),
            "every route needs a RouteDoc with a summary (`.doc(RouteDoc::new(..))` after it); these have none:\n{}",
            missing
                .iter()
                .map(|r| format!("  {} {}", r.method, r.pattern))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    #[test]
    fn operation_ids_are_unique() {
        let router = crate::api::router();
        let mut ids = std::collections::HashMap::new();
        for route in router.routes() {
            let previous = ids.insert(operation_id(&route), (route.method, route.pattern));
            assert!(
                previous.is_none_or(|p| p == (route.method, route.pattern)),
                "{} {} and {:?} share the operation id {}",
                route.method,
                route.pattern,
                previous,
                operation_id(&route),
            );
        }
    }

    #[test]
    fn every_path_parameter_is_named_once() {
        for route in crate::api::router().routes() {
            let names: Vec<_> = route.params().collect();
            let mut unique = names.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                names.len(),
                unique.len(),
                "{} {} names a parameter twice",
                route.method,
                route.pattern
            );
        }
    }

    #[test]
    fn ids_and_tags_read_off_the_pattern() {
        let route = RouteInfo {
            method: "GET",
            pattern: "/v1/agents/{cid}/tell",
            doc: None,
        };
        assert_eq!(operation_id(&route), "get_v1_agents_cid_tell");
        assert_eq!(tag_of("/v1/agents/{cid}/tell"), "agents");
        assert_eq!(tag_of("/internal/model"), "internal");
        assert_eq!(tag_of("/e2b/sandboxes"), "e2b");
        assert_eq!(tag_of("/"), "console");
        assert_eq!(tag_of("/{file}"), "console");
        assert_eq!(
            sample_path("/v1/nodes/{id}/heartbeat"),
            "/v1/nodes/n1/heartbeat"
        );
        assert_eq!(
            openapi_path("/v1/stations/{id}/files/{path..}"),
            "/v1/stations/{id}/files/{path}"
        );
    }
}
