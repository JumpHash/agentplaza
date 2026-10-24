//! The owner's browser of the tool library: the tree, a search and an entry's detail with the seats that may use it.
//! Read-only, and the owner's own (the console's Tools screen reads them).

use std::collections::BTreeSet;

use apz_http::{RouteDoc, Router};
use serde_json::{Value, json};

use super::{Catalog, describe, search, sources};
use crate::state::App;

/// The routes of the tool library.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/tool-library", |app, req, _| {
            let path = req.query_param("path").unwrap_or("");
            let catalog = Catalog::build(Some(app));
            if catalog.entry(path).is_some() {
                return match describe(Some(app), None, path) {
                    Ok(value) => apz_http::Response::json(200, &value),
                    Err(why) => apz_http::Response::json(404, &json!({"error": why})),
                };
            }
            if !path.is_empty() && catalog.group(path).is_none() {
                return apz_http::Response::json(
                    404,
                    &json!({"error": format!("no tool or group at {path}")}),
                );
            }
            let children = catalog.children(Some(app), None, path);
            // At the root, the route groups the library leaves out and why.
            let internal: Vec<Value> = if path.is_empty() {
                sources::INTERNAL
                    .iter()
                    .map(|(group, reason)| json!({"group": group, "reason": reason}))
                    .collect()
            } else {
                Vec::new()
            };
            apz_http::Response::json(
                200,
                &json!({
                    "path": path,
                    "summary": catalog.group(path).unwrap_or(""),
                    "children": children,
                    "internal": internal,
                }),
            )
        })
        .doc(RouteDoc::new("Browse the tool library: a group's children, or an entry's detail (path)"))
        .get("/v1/tool-library/search", |app, req, _| {
            let query = req.query_param("q").unwrap_or("");
            let under = req.query_param("under").unwrap_or("");
            if query.trim().is_empty() {
                return apz_http::Response::json(400, &json!({"error": "q is required"}));
            }
            let hits: Vec<Value> = search(Some(app), None, query, under)
                .into_iter()
                .map(|(entry, score)| {
                    json!({"path": entry.path, "summary": entry.summary, "runs": entry.runs, "score": score})
                })
                .collect();
            apz_http::Response::json(200, &json!({"query": query, "hits": hits}))
        })
        .doc(RouteDoc::new("Search the tool library by words (q, optionally under a group)"))
        .get("/v1/tool-library/entry", |app, req, _| {
            let path = req.query_param("path").unwrap_or("");
            let catalog = Catalog::build(Some(app));
            let Some(entry) = catalog.entry(path) else {
                return apz_http::Response::json(404, &json!({"error": format!("no tool at {path}")}));
            };
            let mut seats: BTreeSet<String> = app.roles.tools.keys().cloned().collect();
            seats.insert(crate::settings::ORCHESTRATOR_ROLE.to_string());
            let seats: Vec<Value> = seats
                .into_iter()
                .map(|seat| {
                    let reason = super::visible(Some(app), &seat, entry).err();
                    json!({"seat": seat, "allowed": reason.is_none(), "reason": reason})
                })
                .collect();
            let mut value = describe(Some(app), None, path).unwrap_or(Value::Null);
            value["seats"] = Value::Array(seats);
            apz_http::Response::json(200, &value)
        })
        .doc(RouteDoc::new("One tool library entry with its schema, and which seats may use it (path)"))
}
