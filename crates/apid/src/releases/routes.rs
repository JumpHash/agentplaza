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
//! | `POST /v1/releases/{id}/sync` | syncs the release to its git platforms now (22.4) |
//! | `POST /v1/releases/{id}/notes` | `{notes}`: edits the notes (the platforms get them on the next sync) |
//! | `POST /v1/releases/{id}/lane` | cuts the release's line (`release/1.4`) at a published release |
//! | `POST /v1/projects/{id}/backports` | `{change, line, worker}`: backports a landed fix to a line, with its review |
//! | `POST /v1/projects/{id}/backports/{change}/land` | lands a backport whose review passed |
//! | `GET /v1/release-channels` | the built-in channels, their qualification depth and store tracks |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_plaza::ConnectedAgent;
use apz_plaza::tasks::CreateTask;
use serde_json::{Value, json};
use weft::keys::KeyPair;

use super::builder::Pools;
use super::channels;
use super::entity::Release;
use super::flow::{self, Builder, Cut, Deps};
use super::lanes::{self, Agents, Chain};
use super::signing::{Notary, Signing, StandInNotary};
use crate::state::App;

pub(crate) fn deps(app: &App) -> Deps<'_> {
    Deps {
        weft: &app.weft,
        runs: &app.recipe_runs,
        reviews: &app.reviews,
        releases: &app.releases,
        signing: app,
    }
}

/// The release step's keys (9.3): the hub's key is Weft's own, and the stand-in platform keys are kept in the vault
/// beside it (made on first use). Only the release step reads them (`releases::signing`).
impl Signing for App {
    fn hub(&self) -> Result<KeyPair, String> {
        crate::weft_hub::hub_key(self)
    }

    fn platform(&self, scheme: &str) -> Result<KeyPair, String> {
        let (name, label) = match scheme {
            "apksigner-standin" => ("RELEASE_ANDROID_KEY", "Android signing key (stand-in)"),
            "authenticode-standin" => ("RELEASE_WINDOWS_KEY", "Windows signing key (stand-in)"),
            other => return Err(format!("no key for the signature scheme {other}")),
        };
        crate::weft_hub::key(self, name, label)
    }

    fn notary(&self) -> &dyn Notary {
        &StandInNotary
    }
}

/// The production agents: a statement of an agent is signed with the agent's own key (`weft_hub::sign_as_agent`).
struct AppAgents<'a>(&'a App);

impl Agents for AppAgents<'_> {
    fn sign(&self, worker: u64, kind: &str, body: Value) -> Result<Value, String> {
        crate::weft_hub::sign_as_agent(self.0, worker, kind, body)
    }
}

/// The production review chain of a backport (4.2): a plaza task claimed by its worker and bound to the change on the
/// line, which `review_graph::start` reviews like any landing change.
pub(crate) struct AppChain<'a>(pub(crate) &'a App);

impl Chain for AppChain<'_> {
    fn task(&self, project: &str, title: &str, worker: u64) -> Result<String, String> {
        let request = CreateTask {
            goal: title.to_string(),
            acceptance: None,
            role: None,
            after: Vec::new(),
            project: Some(project.to_string()),
        };
        let task = self
            .0
            .plaza
            .tasks
            .create(&request)
            .map_err(|e| format!("no task for the backport: {e:?}"))?;
        // The worker writes the change; it has no VM of its own here, so the claim names only its CID.
        let agent = ConnectedAgent {
            cid: worker,
            role: None,
            run: String::new(),
            vm: 0,
        };
        task.claim(&agent)
            .map_err(|e| format!("the backport's task was not claimed: {e:?}"))?;
        Ok(task.id().to_string())
    }

    fn bind(&self, task: &str, change: &str) {
        self.0.weft.bind_lane(task, change);
    }

    fn review(&self, task: &str) -> Option<String> {
        crate::review_graph::start(self.0, task)
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

/// A landing on a project that releases continuously cuts a draft holding the change (4.6, 22.3); its version is
/// numbered from the change since the project's newest release.
pub fn continuous(app: &App, project: &str, change: &str) -> Result<Release, String> {
    let repo = crate::weft_hub::ensure_repo(app, project)?;
    let cut = Cut {
        changes: vec![change.to_string()],
        base: app.releases.list(project).into_iter().next().map(|r| r.id),
        ..Cut::default()
    };
    flow::draft(&deps(app), project, &repo, &cut)
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
                    return answer(flow::candidate(
                        &d,
                        &drafted.id,
                        &Pools { app },
                        cut.number,
                    ));
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
                    &Pools { app } as &dyn Builder,
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
        .post("/v1/releases/{id}/sync", |app, _, p: &Params<'_>| {
            match super::sync::sync_release(app, p.get("id")) {
                Ok(release) => Response::json(200, &release),
                Err(e) => refusal(&e),
            }
        })
        .doc(
            RouteDoc::new(
                "Sync a release to its git platforms now: its tag, release, assets, checks and yank (22.4)",
            )
            .errors(&[404, 409]),
        )
        .post("/v1/releases/{id}/notes", |app, req: &Request, p: &Params<'_>| {
            let v = match body(req) {
                Ok(v) => v,
                Err(r) => return r,
            };
            let Some(notes) = v["notes"].as_str() else {
                return Response::error(400, "the notes are a string");
            };
            answer(flow::notes(&deps(app), p.get("id"), notes))
        })
        .doc(
            RouteDoc::new("Edit a release's notes; the platforms get them on the next sync")
                .errors(&[400, 404]),
        )
        .post("/v1/releases/{id}/lane", |app, _, p: &Params<'_>| {
            answer(lanes::cut_lane(&deps(app), p.get("id")))
        })
        .doc(
            RouteDoc::new(
                "Cut the release's line (`release/1.4`) at a published release, so its maintenance releases and backports land on it",
            )
            .errors(&[404, 409, 503]),
        )
        .post(
            "/v1/projects/{id}/backports",
            |app, req: &Request, p: &Params<'_>| {
                let project = p.get("id");
                if project_missing(app, project) {
                    return Response::error(404, "no such project");
                }
                let v = match body(req) {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let (Some(fix), Some(line), Some(worker)) =
                    (v["change"].as_str(), v["line"].as_str(), v["worker"].as_u64())
                else {
                    return Response::error(
                        400,
                        "a backport names its change, its release line and the worker that writes it",
                    );
                };
                match lanes::backport(
                    &deps(app),
                    &AppAgents(app),
                    &AppChain(app),
                    project,
                    line,
                    fix,
                    worker,
                ) {
                    Ok(backport) => Response::json(201, &backport),
                    Err(e) => refusal(&e),
                }
            },
        )
        .doc(
            RouteDoc::new(
                "Backport a fix that landed on main to a release line: a change on the line, written by the worker's agent, with its own review",
            )
            .errors(&[400, 404, 409, 503]),
        )
        .post(
            "/v1/projects/{id}/backports/{change}/land",
            |app, _, p: &Params<'_>| {
                if project_missing(app, p.get("id")) {
                    return Response::error(404, "no such project");
                }
                match lanes::land_backport(&deps(app), p.get("change")) {
                    Ok(landing) => Response::json(200, &landing),
                    Err(e) => refusal(&e),
                }
            },
        )
        .doc(
            RouteDoc::new(
                "Land a backport whose review passed: it is queued on its line and the line's queue lands it",
            )
            .errors(&[404, 409, 503]),
        )
        .get("/v1/release-channels", |_, _, _| {
            Response::json(200, &json!({ "channels": channels::builtin() }))
        })
        .doc(
            RouteDoc::new("The built-in release channels, with their qualification depth and store tracks")
                .errors(&[]),
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
