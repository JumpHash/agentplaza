//! The workflows' routes (PLAN 21, 23). The first route that matches wins, so `import` is registered before `{name}`.
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /v1/projects/{id}/workflows` | the project's workflows on main, and the files that do not parse (with why) |
//! | `GET /v1/projects/{id}/workflows/import` | the GitHub Actions workflows on main, converted, each with its report (a preview) |
//! | `POST /v1/projects/{id}/workflows/import` `{file, save}` | one of them: its conversion, or with `save` a change that writes `.plaza/workflows/<name>.toml` |
//! | `GET /v1/projects/{id}/workflows/{name}` | one workflow: its triggers, its jobs and their steps |
//! | `GET /v1/projects/{id}/workflows/{name}/runs` | its runs, newest first (the recipe runs, as `/v1/recipe-runs` lists them) |
//! | `GET /v1/projects/{id}/changes/{change}/workflow-runs` | the workflow runs of one change, newest first |
//! | `POST /v1/projects/{id}/workflows/{name}/runs` `{change?, release?}` | starts a manual run, for a change or a release |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use super::drive::RunInfo;
use super::model::DIR;
use super::{Listed, github};
use crate::recipes::routes::summary;
use crate::state::App;

/// A workflow's line in the list.
fn listed_json(item: &Listed) -> Value {
    json!({
        "name": item.workflow.name,
        "file": item.file,
        "triggers": item.workflow.triggers,
        "cron": item.workflow.cron,
        "jobs": item.workflow.jobs.len(),
        "steps": item.workflow.jobs.iter().map(|j| j.steps.len()).sum::<usize>(),
        "error": Value::Null,
    })
}

/// An import's answer: its file, its name, its TOML and its report.
fn import_json(imported: &github::Imported) -> Value {
    json!({
        "file": imported.file,
        "name": imported.name,
        "toml": imported.workflow.to_toml(),
        "report": imported.report,
    })
}

/// The project's GitHub Actions workflows on main, converted.
fn import_file(app: &App, project: &str, file: &str) -> Result<github::Imported, (u16, String)> {
    let files = super::files_on_main(app, project, ".github/workflows", &[".yml", ".yaml"])
        .map_err(|e| (409, e))?;
    let (path, text) = files
        .into_iter()
        .find(|(path, _)| path == file)
        .ok_or((404, format!("no GitHub workflow {file} on main")))?;
    let text = text.map_err(|e| (409, e))?;
    github::convert(&path, &text).map_err(|e| (409, e))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/workflows", |app, _, p: &Params<'_>| {
            let project = p.get("id");
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            match super::list(app, project) {
                Ok((workflows, errors)) => {
                    if let Some(arc) = app.handle().upgrade() {
                        // A schedule is kept in step with the file the project's repo holds.
                        super::sync_schedules(&arc, project, &workflows);
                    }
                    let errors: Vec<Value> = errors
                        .iter()
                        .map(|(file, error)| json!({ "file": file, "error": error }))
                        .collect();
                    Response::json(
                        200,
                        &json!({
                            "workflows": workflows.iter().map(listed_json).collect::<Vec<_>>(),
                            "errors": errors,
                        }),
                    )
                }
                Err(why) => Response::error(409, &why),
            }
        })
        .doc(
            RouteDoc::new("List a project's workflows (its .plaza/workflows files on main), and the files that do not parse")
                .errors(&[404, 409]),
        )
        .get("/v1/projects/{id}/workflows/import", |app, _, p: &Params<'_>| {
            let project = p.get("id");
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            match super::imports(app, project) {
                Ok((converted, errors)) => {
                    let errors: Vec<Value> = errors
                        .iter()
                        .map(|(file, error)| json!({ "file": file, "error": error }))
                        .collect();
                    Response::json(
                        200,
                        &json!({
                            "imports": converted.iter().map(import_json).collect::<Vec<_>>(),
                            "errors": errors,
                        }),
                    )
                }
                Err(why) => Response::error(409, &why),
            }
        })
        .doc(
            RouteDoc::new("Preview the project's GitHub Actions workflows (.github/workflows on main) converted to plaza's format, each with its report of what did not convert")
                .errors(&[404, 409]),
        )
        .post("/v1/projects/{id}/workflows/import", |app, req: &Request, p: &Params<'_>| {
            let project = p.get("id");
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            let body: Value = match serde_json::from_slice(&req.body) {
                Ok(body) => body,
                Err(err) => return Response::error(400, &format!("bad body: {err}")),
            };
            let Some(file) = body["file"].as_str() else {
                return Response::error(400, "an import names its GitHub workflow file");
            };
            let save = body["save"].as_bool().unwrap_or(false);
            let imported = match import_file(app, project, file) {
                Ok(imported) => imported,
                Err((status, why)) => return Response::error(status, &why),
            };
            if !save {
                return Response::json(200, &import_json(&imported));
            }
            let workflow = &imported.workflow;
            if let Err(why) = workflow.check(&super::known(app, project)) {
                return Response::error(409, &why);
            }
            let path = format!("{DIR}/{}.toml", workflow.name);
            let title = format!("Import {file} as workflow {}", workflow.name);
            match super::save_change(app, project, &path, &workflow.to_toml(), &title) {
                Ok(change) => {
                    let mut item = import_json(&imported);
                    item["change"] = json!(change);
                    Response::json(201, &item)
                }
                Err(why) => Response::error(409, &why),
            }
        })
        .doc(
            RouteDoc::new("Save one converted GitHub Actions workflow as .plaza/workflows/<name>.toml through a change the owner lands (save), or preview it")
                .errors(&[400, 404, 409]),
        )
        .get("/v1/projects/{id}/workflows/{name}", |app, _, p: &Params<'_>| {
            let project = p.get("id");
            match super::load(app, project, p.get("name")) {
                Ok(listed) => {
                    let mut out = listed_json(&listed);
                    out["description"] = json!(listed.workflow.description);
                    out["jobs"] = serde_json::to_value(&listed.workflow.jobs).unwrap_or(Value::Null);
                    Response::json(200, &out)
                }
                Err(why) => Response::error(404, &why),
            }
        })
        .doc(RouteDoc::new("Show one workflow: its triggers, its jobs and their steps").errors(&[404]))
        .get("/v1/projects/{id}/workflows/{name}/runs", |app, _, p: &Params<'_>| {
            let project = p.get("id");
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            let recipe = format!("{}{}", super::PREFIX, p.get("name"));
            let runs: Vec<Value> = app
                .recipe_runs
                .list(None, Some(project))
                .iter()
                .filter(|v| v.run.recipe == recipe)
                .map(summary)
                .collect();
            Response::json(200, &json!({ "runs": runs }))
        })
        .doc(RouteDoc::new("List a workflow's runs, newest first").errors(&[]))
        .get("/v1/projects/{id}/changes/{change}/workflow-runs", |app, _, p: &Params<'_>| {
            let project = p.get("id");
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            Response::json(200, &json!({ "runs": super::runs_of_change(app, project, p.get("change")) }))
        })
        .doc(RouteDoc::new("The workflow runs of one change, newest first").errors(&[404]))
        .post("/v1/projects/{id}/workflows/{name}/runs", |app, req: &Request, p: &Params<'_>| {
            let project = p.get("id");
            let name = p.get("name");
            let body: Value = if req.body.is_empty() {
                json!({})
            } else {
                match serde_json::from_slice(&req.body) {
                    Ok(body) => body,
                    Err(err) => return Response::error(400, &format!("bad body: {err}")),
                }
            };
            let text = |key: &str| body[key].as_str().map(str::trim).filter(|s| !s.is_empty());
            let (change, release) = (text("change"), text("release"));
            if change.is_some() && release.is_some() {
                return Response::error(400, "a manual run is for a change or a release, not both");
            }
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            let Some(arc) = app.handle().upgrade() else {
                return Response::error(503, "the hub is shutting down");
            };
            let info = RunInfo {
                project: project.to_string(),
                change: change.map(str::to_string),
                release: release.map(str::to_string),
            };
            match super::start(&arc, project, name, "manual", &info) {
                Ok(run) => match app.recipe_runs.view(&run) {
                    Some(view) => Response::json(202, &summary(&view)),
                    None => Response::error(409, "the run ended before it was listed"),
                },
                Err(why) => Response::error(409, &why),
            }
        })
        .doc(
            RouteDoc::new("Start a manual run of a workflow, for a change or a release; the run is listed under its runs")
                .errors(&[400, 404, 409, 503]),
        )
}
