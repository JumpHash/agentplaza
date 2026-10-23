//! The release routes (PLAN 22.12). Weft's own release routes stay the statement-level API; these wrap them with what
//! the project adds (the candidate's builds, the checks, the life's moves).
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /v1/projects/{id}/releases` | the project's releases, newest first |
//! | `POST /v1/projects/{id}/releases` | cuts a draft; `stage: "candidate"` cuts the candidate too |
//! | `GET /v1/releases/{id}` | the release with its changes, artifacts, verdicts, promotions and sync |
//! | `POST /v1/releases/{id}/candidate` | builds every target and cuts the candidate (a draft) |
//! | `POST /v1/releases/{id}/checks` | the hub's check result for the release's commit (`passed`, `reason`) |
//! | `POST /v1/releases/{id}/qualify`, `/publish` | the next move of the life |
//! | `POST /v1/releases/{id}/promote` | `{environment}`: promotes a published release |
//! | `POST /v1/releases/{id}/yank` | `{reason}`: withdraws a published release |
//! | `GET /v1/releases/{id}/report` | the release, its run and its life as Weft holds it |
//! | `GET /v1/releases/{id}/artifacts/{target}` | one target's artifact, with Weft's record of it |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use super::entity::Release;
use super::flow::{self, Builder, Cut, Deps, NoPools};
use crate::state::App;

fn deps(app: &App) -> Deps<'_> {
    Deps {
        weft: &app.weft,
        runs: &app.recipe_runs,
        reviews: &app.reviews,
        releases: &app.releases,
    }
}

/// The status of a refused move: an unknown release is 404, a Weft that is not running 503, and anything Weft or the
/// life refused is 409.
fn refusal(e: &str) -> Response {
    let code = if e == "no such release" {
        404
    } else if e.contains("Weft is not running") {
        503
    } else {
        409
    };
    Response::error(code, e)
}

fn answer(result: Result<Release, String>) -> Response {
    match result {
        Ok(release) => Response::json(200, &release),
        Err(e) => refusal(&e),
    }
}

fn body(req: &Request) -> Result<Value, Response> {
    if req.body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&req.body).map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

fn project_missing(app: &App, id: &str) -> bool {
    app.projects.get(id).is_none()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/releases", |app, _, p: &Params<'_>| {
            if project_missing(app, p.get("id")) {
                return Response::error(404, "no such project");
            }
            Response::json(200, &json!({ "releases": app.releases.list(p.get("id")) }))
        })
        .doc(RouteDoc::new("List a project's releases, newest first").errors(&[404]))
        .post(
            "/v1/projects/{id}/releases",
            |app, req: &Request, p: &Params<'_>| {
                let id = p.get("id");
                if project_missing(app, id) {
                    return Response::error(404, "no such project");
                }
                let v = match body(req) {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let cut: Cut = match serde_json::from_value(v.clone()) {
                    Ok(c) => c,
                    Err(e) => return Response::error(400, &format!("bad release: {e}")),
                };
                let repo = match crate::weft_hub::ensure_repo(app, id) {
                    Ok(r) => r,
                    Err(e) => return refusal(&e),
                };
                let d = deps(app);
                let drafted = match flow::draft(&d, id, &repo, &cut) {
                    Ok(r) => r,
                    Err(e) => return refusal(&e),
                };
                if v["stage"].as_str() == Some("candidate") {
                    return answer(flow::candidate(&d, &drafted.id, &NoPools, cut.number));
                }
                match flow::get(&d, &drafted.id) {
                    Ok(r) => Response::json(201, &r),
                    Err(e) => refusal(&e),
                }
            },
        )
        .doc(
            RouteDoc::new("Cut a release's draft, or its candidate with `stage: candidate`")
                .errors(&[400, 404, 409, 503]),
        )
        .get("/v1/releases/{id}", |app, _, p: &Params<'_>| {
            let d = deps(app);
            let id = p.get("id");
            // The entity is read back from Weft's life of the release, so what Weft moved elsewhere shows here.
            let _ = flow::refresh(&d, id);
            answer(flow::get(&d, id))
        })
        .doc(
            RouteDoc::new(
                "Show a release with its changes, artifacts, verdicts, promotions and sync",
            )
            .errors(&[404]),
        )
        .post(
            "/v1/releases/{id}/candidate",
            |app, req: &Request, p: &Params<'_>| {
                let v = match body(req) {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let number = v["number"].as_u64().map(|n| n as u32);
                answer(flow::candidate(
                    &deps(app),
                    p.get("id"),
                    &NoPools as &dyn Builder,
                    number,
                ))
            },
        )
        .doc(
            RouteDoc::new(
                "Build every target of a draft and cut its candidate, which the hub signs",
            )
            .errors(&[404, 409, 503]),
        )
        .post(
            "/v1/releases/{id}/checks",
            |app, req: &Request, p: &Params<'_>| {
                let v = match body(req) {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let Some(passed) = v["passed"].as_bool() else {
                    return Response::error(400, "checks say whether they passed");
                };
                let reason = v["reason"].as_str().unwrap_or("");
                answer(flow::checks(&deps(app), p.get("id"), passed, reason))
            },
        )
        .doc(
            RouteDoc::new("Record the checks of a release's commit (ci): passed or failed")
                .errors(&[400, 404, 409]),
        )
        .post("/v1/releases/{id}/qualify", |app, _, p: &Params<'_>| {
            answer(flow::qualify(&deps(app), p.get("id")))
        })
        .doc(RouteDoc::new("Send a candidate to qualification").errors(&[404, 409]))
        .post("/v1/releases/{id}/publish", |app, _, p: &Params<'_>| {
            answer(flow::publish(&deps(app), p.get("id")))
        })
        .doc(
            RouteDoc::new("Publish a release whose checks passed or that was qualified")
                .errors(&[404, 409]),
        )
        .post(
            "/v1/releases/{id}/promote",
            |app, req: &Request, p: &Params<'_>| {
                let v = match body(req) {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let Some(environment) = v["environment"].as_str().filter(|e| !e.trim().is_empty())
                else {
                    return Response::error(
                        400,
                        "a promotion names its environment or store track",
                    );
                };
                answer(flow::promote(&deps(app), p.get("id"), environment))
            },
        )
        .doc(
            RouteDoc::new("Promote a published release to an environment or store track")
                .errors(&[400, 404, 409]),
        )
        .post(
            "/v1/releases/{id}/yank",
            |app, req: &Request, p: &Params<'_>| {
                let v = match body(req) {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let Some(reason) = v["reason"].as_str().filter(|r| !r.trim().is_empty()) else {
                    return Response::error(400, "a yank says why");
                };
                answer(flow::yank(&deps(app), p.get("id"), reason))
            },
        )
        .doc(
            RouteDoc::new("Withdraw a published release, with the reason").errors(&[400, 404, 409]),
        )
        .get("/v1/releases/{id}/report", |app, _, p: &Params<'_>| {
            let d = deps(app);
            let release = match flow::get(&d, p.get("id")) {
                Ok(r) => r,
                Err(e) => return refusal(&e),
            };
            let run = release
                .run
                .as_deref()
                .and_then(|run| app.recipe_runs.view(run))
                .map(|v| crate::recipes::routes::summary(&v));
            let life = d
                .weft
                .read(&format!(
                    "/repos/{}/releases/{}",
                    release.repo, release.version
                ))
                .map(|v| v["life"].clone())
                .unwrap_or(Value::Null);
            Response::json(
                200,
                &json!({ "release": release, "run": run, "life": life }),
            )
        })
        .doc(
            RouteDoc::new("A release's report: the release, its run and its life in Weft")
                .errors(&[404]),
        )
        .get(
            "/v1/releases/{id}/artifacts/{target}",
            |app, _, p: &Params<'_>| {
                let d = deps(app);
                let release = match flow::get(&d, p.get("id")) {
                    Ok(r) => r,
                    Err(e) => return refusal(&e),
                };
                let target = p.get("target");
                let Some(row) = release.artifacts.iter().find(|a| a.target == target) else {
                    return Response::error(404, "no artifact for that target");
                };
                let weft = d
                    .weft
                    .read(&format!("/artifacts/{}", row.artifact))
                    .unwrap_or(Value::Null);
                Response::json(200, &json!({ "artifact": row, "weft": weft }))
            },
        )
        .doc(
            RouteDoc::new("One target's artifact of a release, with Weft's record of it")
                .errors(&[404]),
        )
}
