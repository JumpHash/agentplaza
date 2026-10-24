//! The upgrades' routes (PLAN 24.2): a project's schedules and batches, and an advisory's fix (the entry point PLAN 12.3's
//! advisories use, `upgrades::for_advisory`).
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /v1/projects/{id}/upgrades` | the project's schedules (per ecosystem) and its batches, newest first |
//! | `POST /v1/projects/{id}/upgrades/fix` `{advisory, ecosystem, package, fixed, priority}` | starts an advisory's fix now |
//! | `POST /v1/projects/{id}/upgrades/run` | plans every ecosystem of the project now (as its schedule does) |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use super::{Advisory, Ecosystem};
use crate::state::App;

fn list_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let pid = p.get("id");
    if app.projects.get(pid).is_none() {
        return Response::error(404, "no such project");
    }
    Response::json(
        200,
        &json!({
            "schedules": app.upgrades.schedules(pid),
            "batches": app.upgrades.batches(pid),
        }),
    )
}

fn fix_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let pid = p.get("id");
    if app.projects.get(pid).is_none() {
        return Response::error(404, "no such project");
    }
    let body: Value = match serde_json::from_slice(&req.body) {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let (Some(id), Some(eco), Some(package), Some(fixed), Some(priority)) = (
        body["advisory"].as_str(),
        body["ecosystem"].as_str(),
        body["package"].as_str(),
        body["fixed"].as_str(),
        body["priority"].as_str(),
    ) else {
        return Response::error(
            400,
            "a fix names its advisory, ecosystem, package, fixed release and priority",
        );
    };
    let Some(ecosystem) = Ecosystem::parse(eco) else {
        return Response::error(400, "an unknown ecosystem");
    };
    let Some(hub) = app.handle().upgrade() else {
        return Response::error(503, "the hub is stopping");
    };
    let advisory = Advisory {
        id: id.to_string(),
        ecosystem,
        package: package.to_string(),
        fixed: fixed.to_string(),
        priority: priority.to_string(),
    };
    match super::for_advisory(&hub, pid, advisory) {
        Ok(()) => Response::json(
            202,
            &json!({ "project": pid, "ecosystem": eco, "package": package, "fixed": fixed }),
        ),
        Err(e) => Response::error(409, &e),
    }
}

fn run_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let pid = p.get("id");
    if app.projects.get(pid).is_none() {
        return Response::error(404, "no such project");
    }
    let Some(hub) = app.handle().upgrade() else {
        return Response::error(503, "the hub is stopping");
    };
    match super::start_all(&hub, pid) {
        Ok(()) => Response::json(202, &json!({ "project": pid })),
        Err(e) => Response::error(409, &e),
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/projects/{id}/upgrades/run", run_route)
        .doc(
            RouteDoc::new("Plans every ecosystem of the project now: each batch becomes a task, as a schedule's does")
                .status(202)
                .errors(&[404, 409]),
        )
        .get("/v1/projects/{id}/upgrades", list_route)
        .doc(RouteDoc::new("A project's dependency upgrades: its schedules per ecosystem and its batches").errors(&[404]))
        .post("/v1/projects/{id}/upgrades/fix", fix_route)
        .doc(
            RouteDoc::new("Starts an advisory's fix now: the crate moves to its fixed release through the upgrade batches")
                .status(202)
                .errors(&[400, 404, 409]),
        )
}
