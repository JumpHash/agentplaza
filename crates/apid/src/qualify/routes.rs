//! The qualification's routes (PLAN 22.12).
//!
//! | Route | Answer |
//! |---|---|
//! | `POST /v1/releases/{id}/qualify` | `{planner?, second?}`: qualifies the candidate (the release goes to qualifying); the models make failures acceptable |
//! | `GET /v1/releases/{id}/qualification` | the release's qualification: its depth and why, its stages, its verdict, its fix tasks |
//! | `GET /v1/releases/{id}/report` | (in `releases/routes.rs`) the release, its run, its life and this report |
//! | `POST /v1/test-lab/runs/{id}/tests` | `{tests: [{name, passed}]}`: the functional tests a lab run reported |
//! | `POST /v1/test-lab/runs/{id}/end` | `{outcome}`: ends a lab run (passed, failed or cancelled) and advances the qualifications waiting on it |
//! | `GET /v1/projects/{id}/tests` | every test's history, its flakiness and quarantine |
//! | `POST /v1/projects/{id}/tests/{name}/fixed` | `{reason}`: clears a test's quarantine (its fix task is the owner's) |
//! | `GET /v1/projects/{id}/trends` | performance, sizes, start times and flake rates across the project's releases |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use crate::releases::entity::Release;
use crate::releases::flow::{self, Deps};
use crate::releases::routes::deps;
use crate::state::App;

use super::accept::{GatewayJudge, Judge};
use super::engine::{self, Setup};
use super::plan;
use super::report;
use super::report::TrendRun;
use super::stages::{AppLab, Cx, Lab};
use super::store::{LabTest, QualifyRun};
use super::testcases::History;

/// A release with at least this many changes is risky: a nightly qualification of it is raised (`plan.rs`).
const RISKY_CHANGES: usize = 5;

fn now() -> u64 {
    apz_plaza::now_ms()
}

fn refusal(e: &str) -> Response {
    let code = if e.starts_with("no such") || e.contains("no qualification") {
        404
    } else {
        409
    };
    Response::error(code, e)
}

fn body(req: &Request) -> Result<Value, Response> {
    if req.body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&req.body).map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

/// The planner's setup for a candidate: the project's depth and soak, its topologies, the release's risk, and the models
/// the body names for acceptances.
fn setup_of(app: &App, project: &str, release: &Release, lab: &dyn Lab, body: &Value) -> Setup {
    let settings = app.project_settings.get(project);
    Setup {
        override_: settings
            .qualify_depth
            .get(&release.channel)
            .and_then(|text| plan::parse_stages(text).ok()),
        has_topologies: !lab.topologies(project).is_empty(),
        risky: release.changes.len() >= RISKY_CHANGES,
        soak_asked: settings.qualify_soak,
        planner: body["planner"].as_str().unwrap_or_default().to_string(),
        second: body["second"].as_str().unwrap_or_default().to_string(),
    }
}

/// Files the plaza tasks of a qualification's fix tasks that have none yet, and returns the run as it is now.
pub fn file_tasks(app: &App, id: &str) -> Option<QualifyRun> {
    let run = app.qualify.run(id)?;
    let mut ids: Vec<(usize, String)> = Vec::new();
    for (i, fix) in run
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.task.is_empty())
    {
        let request = apz_plaza::tasks::CreateTask {
            goal: fix.goal.clone(),
            acceptance: None,
            role: None,
            after: Vec::new(),
            project: Some(run.project.clone()),
        };
        match app.plaza.tasks.create(&request) {
            Ok(task) => ids.push((i, task.id().to_string())),
            Err(e) => eprintln!("apid: qualify: the fix task of {id} was not filed: {e:?}"),
        }
    }
    app.qualify.update_run(id, |r| {
        for (i, task) in &ids {
            if let Some(fix) = r.tasks.get_mut(*i) {
                fix.task = task.clone();
            }
        }
    })
}

/// The judge of the acceptances, when the qualification names its models: the gateway's chat path.
fn with_judge<T>(app: &App, planner: &str, f: impl FnOnce(Option<&dyn Judge>) -> T) -> T {
    if planner.is_empty() {
        return f(None);
    }
    let gateway = app.providers.gateway();
    let judge = GatewayJudge(&gateway);
    f(Some(&judge))
}

/// A qualification's context: its release, its project, the deps and the lab.
fn context<'a>(
    app: &'a App,
    d: &'a Deps<'a>,
    lab: &'a AppLab<'a>,
    release: &'a Release,
    project: &'a str,
) -> Cx<'a> {
    Cx {
        deps: d,
        lab,
        store: &app.qualify,
        release,
        project,
        now: now(),
        compat: lab,
    }
}

fn qualify_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body = match body(req) {
        Ok(body) => body,
        Err(r) => return r,
    };
    let id = p.get("id");
    let d = deps(app);
    let release = match flow::get(&d, id) {
        Ok(release) => release,
        Err(e) => return refusal(&e),
    };
    let project = release.project.clone();
    let lab = AppLab { app };
    let setup = setup_of(app, &project, &release, &lab, &body);
    let outcome = with_judge(app, &setup.planner, |judge| {
        let cx = context(app, &d, &lab, &release, &project);
        engine::start(&cx, judge, &setup)
    });
    match outcome {
        Ok(run) => {
            let run = file_tasks(app, &run.id).unwrap_or(run);
            Response::json(200, &run)
        }
        Err(e) => refusal(&e),
    }
}

fn qualification_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.releases.get(id).is_none() {
        return Response::error(404, "no such release");
    }
    match app.qualify.of_release(id) {
        Some(run) => Response::json(
            200,
            &json!({ "qualification": run, "acceptances": app.qualify.acceptances(id) }),
        ),
        None => Response::error(404, "no qualification for this release"),
    }
}

/// The lab minutes of a qualification's lab runs (their start to their end).
fn lab_minutes(lab: &dyn Lab, run: &QualifyRun) -> u64 {
    run.lab_runs
        .iter()
        .filter_map(|r| lab.state(r))
        .filter_map(|s| match (s.started_ms, s.ended_ms) {
            (Some(start), Some(end)) if end > start => Some((end - start) / 60_000),
            _ => None,
        })
        .sum()
}

/// The release report's qualification part (PLAN 22.11): the report of the release's newest qualification, compared with
/// the project's qualification before it. None when the release was never qualified.
pub fn report_for(app: &App, release: &Release) -> Option<Value> {
    let run = app.qualify.of_release(&release.id)?;
    let previous = app
        .qualify
        .of_project(&release.project)
        .into_iter()
        .find(|r| r.release != release.id && r.started_ms < run.started_ms);
    let previous_release = previous.as_ref().and_then(|p| app.releases.get(&p.release));
    let acceptances = app.qualify.acceptances(&release.id);
    let histories = app.qualify.histories(&release.project);
    let lab = AppLab { app };
    Some(report::report(&report::Input {
        release,
        run: &run,
        previous: previous.as_ref(),
        previous_release: previous_release.as_ref(),
        acceptances: &acceptances,
        histories: &histories,
        lab_minutes: lab_minutes(&lab, &run),
    }))
}

fn lab_tests_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body = match body(req) {
        Ok(body) => body,
        Err(r) => return r,
    };
    let id = p.get("id");
    if !app.test_lab.runs().iter().any(|r| r.id == id) {
        return Response::error(404, "no such run");
    }
    let Some(list) = body["tests"].as_array() else {
        return Response::error(400, "the tests are a list of {name, passed}");
    };
    let mut tests = Vec::new();
    for t in list {
        match (
            t["name"].as_str().filter(|n| !n.trim().is_empty()),
            t["passed"].as_bool(),
        ) {
            (Some(name), Some(passed)) => tests.push(LabTest {
                run: id.to_string(),
                name: name.trim().to_string(),
                passed,
            }),
            _ => return Response::error(400, "each test has a name and whether it passed"),
        }
    }
    app.qualify.record_lab_tests(id, &tests);
    Response::json(200, &json!({ "run": id, "recorded": tests.len() }))
}

fn end_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body = match body(req) {
        Ok(body) => body,
        Err(r) => return r,
    };
    let id = p.get("id");
    let outcome = body["outcome"].as_str().unwrap_or_default();
    if let Err(e) = crate::test_lab::finish(app, id, outcome) {
        return if e == "no such run" {
            Response::error(404, &e)
        } else {
            Response::error(409, &e)
        };
    }
    // The qualifications waiting on this run move on now.
    let waiting: Vec<QualifyRun> = app
        .qualify
        .all_runs()
        .into_iter()
        .filter(|q| !q.done() && q.lab_runs.iter().any(|r| r == id))
        .collect();
    let lab = AppLab { app };
    for q in &waiting {
        let Some(release) = app.releases.get(&q.release) else {
            continue;
        };
        let d = deps(app);
        let advanced = with_judge(app, &q.planner, |judge| {
            let cx = context(app, &d, &lab, &release, &q.project);
            engine::advance(&cx, judge, &q.id)
        });
        if let Err(e) = advanced {
            return Response::error(
                500,
                &format!("the qualification {} did not advance: {e}", q.id),
            );
        }
        file_tasks(app, &q.id);
    }
    Response::json(
        200,
        &json!({ "run": id, "advanced": waiting.iter().map(|q| q.id.clone()).collect::<Vec<_>>() }),
    )
}

fn tests_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    let histories: Vec<History> = app.qualify.histories(project);
    Response::json(200, &json!({ "tests": histories }))
}

fn fixed_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body = match body(req) {
        Ok(body) => body,
        Err(r) => return r,
    };
    let Some(reason) = body["reason"].as_str().filter(|r| !r.trim().is_empty()) else {
        return Response::error(400, "a test is fixed with the reason");
    };
    let project = p.get("id");
    let name = p.get("name");
    if app.qualify.history(project, name).is_none() {
        return Response::error(404, "no such test");
    }
    let updated = app
        .qualify
        .update_history(project, name, |h| h.fixed(now(), reason.trim()));
    Response::json(200, &updated)
}

fn trends_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    let lab = AppLab { app };
    let mut releases: Vec<(Release, QualifyRun)> = Vec::new();
    for run in app.qualify.of_project(project).into_iter().rev() {
        if let Some(release) = app.releases.get(&run.release) {
            releases.push((release, run));
        }
    }
    let starts: Vec<Vec<u64>> = releases
        .iter()
        .map(|(_, run)| {
            run.lab_runs
                .iter()
                .filter_map(|r| lab.state(r))
                .filter_map(|s| {
                    s.started_ms
                        .map(|start| start.saturating_sub(s.enqueued_ms))
                })
                .collect()
        })
        .collect();
    let trend_runs: Vec<TrendRun> = releases
        .iter()
        .zip(starts)
        .map(|((release, run), start_ms)| TrendRun {
            release,
            run,
            start_ms,
        })
        .collect();
    Response::json(200, &report::trends(&trend_runs))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/releases/{id}/qualify", qualify_route)
        .doc(
            RouteDoc::new(
                "Qualify a candidate: its depth is planned for its channel, and the stages run as the qualify recipe's steps; a verdict moves the release on or fails it (body: planner and second, the models that may accept a failure)",
            )
            .errors(&[400, 404, 409]),
        )
        .get("/v1/releases/{id}/qualification", qualification_route)
        .doc(RouteDoc::new("A release's qualification: its depth, its stages, its verdict and its acceptances").errors(&[404]))
        .post("/v1/test-lab/runs/{id}/tests", lab_tests_route)
        .doc(RouteDoc::new("Record the functional tests a lab run reported: each with whether it passed").errors(&[400, 404]))
        .post("/v1/test-lab/runs/{id}/end", end_route)
        .doc(RouteDoc::new("End a lab run (passed, failed or cancelled) and advance the qualifications waiting on it").errors(&[400, 404, 409]))
        .get("/v1/projects/{id}/tests", tests_route)
        .doc(RouteDoc::new("Every test of a project with its history, its flakiness and its quarantine").errors(&[404]))
        .post("/v1/projects/{id}/tests/{name}/fixed", fixed_route)
        .doc(RouteDoc::new("Clear a test's quarantine: it is fixed, with the reason").errors(&[400, 404]))
        .get("/v1/projects/{id}/trends", trends_route)
        .doc(RouteDoc::new("Performance, sizes, start times and flake rates across a project's qualified releases").errors(&[404]))
}
