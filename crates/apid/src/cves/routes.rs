//! The CVE report's routes (PLAN 24.3).
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /v1/projects/{id}/cves` | the project's findings (most severe first), its feeds' last polls and counts |
//! | `GET /v1/projects/{id}/cves/{cid}` | one finding with its releases and environments |
//! | `GET /v1/projects/{id}/cves/export?format=json\|csv` | the report as JSON or CSV (an attachment) |
//! | `POST /v1/projects/{id}/cves/poll` | polls the feeds now (202) |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::json;

use super::{counts, export};
use crate::state::App;

fn list_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let pid = p.get("id");
    if app.projects.get(pid).is_none() {
        return Response::error(404, "no such project");
    }
    let findings = app.cves.findings(pid);
    Response::json(
        200,
        &json!({
            "project": pid,
            "polled_ms": app.cves.polled_ms(pid),
            "feeds": app.cves.feeds(pid),
            "counts": counts(&findings),
            "findings": findings,
        }),
    )
}

fn detail_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let (pid, cid) = (p.get("id"), p.get("cid"));
    if app.projects.get(pid).is_none() {
        return Response::error(404, "no such project");
    }
    match app.cves.finding(pid, cid) {
        Some(finding) => Response::json(200, &export::finding_json(&finding)),
        None => Response::error(404, "no such finding"),
    }
}

fn export_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let pid = p.get("id");
    if app.projects.get(pid).is_none() {
        return Response::error(404, "no such project");
    }
    let findings = app.cves.findings(pid);
    match req.query_param("format").unwrap_or("json") {
        "json" => Response::new(200, "application/json", export::json(&findings)).with_header(
            "Content-Disposition",
            &format!("attachment; filename=\"{pid}-cves.json\""),
        ),
        "csv" => Response::new(200, "text/csv; charset=utf-8", export::csv(&findings)).with_header(
            "Content-Disposition",
            &format!("attachment; filename=\"{pid}-cves.csv\""),
        ),
        _ => Response::error(400, "format is json or csv"),
    }
}

fn poll_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let pid = p.get("id");
    if app.projects.get(pid).is_none() {
        return Response::error(404, "no such project");
    }
    let Some(hub) = app.handle().upgrade() else {
        return Response::error(503, "the hub is stopping");
    };
    if !hub.weft.running() {
        return Response::error(
            409,
            "Weft is not running: the project has no repository to read",
        );
    }
    super::start(&hub, pid);
    Response::json(202, &json!({ "project": pid }))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/cves", list_route)
        .doc(RouteDoc::new("A project's CVE report: its findings (advisories that reach its packages, with the releases and environments they reach, reachability and the fix), its feeds' last polls and counts").errors(&[404]))
        .get("/v1/projects/{id}/cves/export", export_route)
        .doc(RouteDoc::new("The CVE report exported as JSON or CSV (format=json|csv)").errors(&[400, 404]))
        .get("/v1/projects/{id}/cves/{cid}", detail_route)
        .doc(RouteDoc::new("One CVE finding with its releases and environments").errors(&[404]))
        .post("/v1/projects/{id}/cves/poll", poll_route)
        .doc(RouteDoc::new("Polls the advisory feeds for the project now").status(202).errors(&[404, 409, 503]))
}
