//! The recipe engine's routes (PLAN 4.1): the runs of recipes.
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /v1/recipe-runs?task=&project=` | the runs, newest first |
//! | `GET /v1/recipe-runs/{id}` | a run: its steps, edges, the recipe it ran |
//! | `GET /v1/recipe-runs/{id}/graph` | a run's graph as the IDE draws it: each step and edge with its state |
//! | `POST /v1/recipe-runs/{id}/cancel` | stops a run; a task's run stops its task |
//! | `POST /v1/recipe-runs/ci` `{project, change}` | queues the CI recipe's checks of a change (4.5); the run is listed when the scheduler starts it |
//!
//! The recipes themselves are on `/v1/recipes` (review_graph/routes.rs answers for them).

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use super::runs::{RunState, View};
use crate::state::App;

/// A run's summary, for lists.
pub fn summary(v: &View) -> Value {
    json!({
        "run": v.run.id, "recipe": v.run.recipe, "version": v.run.version, "source": v.run.source,
        "trigger": v.run.trigger, "task": v.run.task, "project": v.run.project, "subject": v.run.subject,
        "state": v.run.state, "outcome": v.run.outcome,
        "created_ms": v.run.created_ms, "updated_ms": v.run.updated_ms,
    })
}

/// A run with its steps, edges and the recipe it ran.
fn detail(v: &View) -> Value {
    let mut out = summary(v);
    out["steps"] = json!(v.run.steps);
    out["edges"] = json!(v.run.edges);
    out["recipe_toml"] = json!(v.recipe.to_toml());
    out
}

/// The graph the IDE draws: each step with its seat, role, model, fan-out and gate, and each edge with its condition.
fn graph(v: &View) -> Value {
    let nodes: Vec<Value> = v
        .recipe
        .steps
        .iter()
        .map(|def| {
            let state = v.walk.steps.get(&def.id);
            json!({
                "id": def.id,
                "seat": def.seat_name(),
                "role": def.role,
                "engine": def.engine,
                "model": def.model,
                "fan_out": def.fan_out,
                "gate": def.gate_conditions(),
                "state": state.map(|s| s.state),
                "visits": state.map(|s| s.visits),
                "items": state.map(|s| s.items),
                "reported": state.map(|s| s.reported),
                "gate_read": state.map(|s| s.gate.clone()),
            })
        })
        .collect();
    let edges: Vec<Value> = v
        .recipe
        .edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "index": i, "from": e.from, "to": e.to, "when": e.when, "max": e.max,
                "state": v.walk.edges.get(i).map(|s| s.state),
                "taken": v.walk.edges.get(i).map(|s| s.taken),
            })
        })
        .collect();
    json!({ "run": v.run.id, "recipe": v.recipe.key(), "state": v.run.state, "nodes": nodes, "edges": edges })
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/recipe-runs", |app, req: &Request, _| {
            let task = req.query_param("task");
            let project = req.query_param("project");
            let runs: Vec<Value> = app
                .recipe_runs
                .list(task, project)
                .iter()
                .map(summary)
                .collect();
            Response::json(200, &json!({ "runs": runs }))
        })
        .doc(RouteDoc::new(
            "List recipe runs, newest first, optionally of one task or project",
        ))
        .get("/v1/recipe-runs/{id}", |app, _, p: &Params<'_>| {
            match app.recipe_runs.view(p.get("id")) {
                Some(v) => Response::json(200, &detail(&v)),
                None => Response::error(404, "no such recipe run"),
            }
        })
        .doc(
            RouteDoc::new("Show a recipe run: its steps, edges and the recipe it ran")
                .errors(&[404]),
        )
        .get(
            "/v1/recipe-runs/{id}/graph",
            |app, _, p: &Params<'_>| match app.recipe_runs.view(p.get("id")) {
                Some(v) => Response::json(200, &graph(&v)),
                None => Response::error(404, "no such recipe run"),
            },
        )
        .doc(
            RouteDoc::new("Show a recipe run's graph with each step's and edge's state")
                .errors(&[404]),
        )
        .post("/v1/recipe-runs/{id}/cancel", |app, _, p: &Params<'_>| {
            let id = p.get("id");
            let Some(view) = app.recipe_runs.view(id) else {
                return Response::error(404, "no such recipe run");
            };
            if view.run.state != RunState::Running && view.run.state != RunState::Owner {
                return Response::error(409, "that recipe run has ended");
            }
            let now = apz_plaza::now_ms();
            app.recipe_runs.cancel(id, "stopped by the owner", now);
            // A task's run stops the task's work too: its seats close and its sessions end.
            if let Some(task) = view.run.task.as_deref() {
                crate::work_loop::stop_task(
                    app,
                    task,
                    "its recipe run was stopped by the owner",
                    now,
                );
            }
            app.bus
                .publish("recipe_run", &json!({ "run": id, "state": "cancelled" }));
            match app.recipe_runs.view(id) {
                Some(v) => Response::json(200, &detail(&v)),
                None => Response::error(404, "no such recipe run"),
            }
        })
        .doc(RouteDoc::new("Stop a recipe run; a task's run stops its task").errors(&[404, 409]))
        .post("/v1/recipe-runs/ci", |app, req: &Request, _| {
            let body: Value = match serde_json::from_slice(&req.body) {
                Ok(body) => body,
                Err(err) => return Response::error(400, &format!("bad body: {err}")),
            };
            let (Some(project), Some(change)) = (body["project"].as_str(), body["change"].as_str())
            else {
                return Response::error(400, "a CI run needs a project and a change");
            };
            if app.projects.get(project).is_none() {
                return Response::error(404, "no such project");
            }
            crate::checks::request(project, change);
            Response::json(
                202,
                &json!({ "project": project, "change": change, "state": "queued" }),
            )
        })
        .doc(
            RouteDoc::new(
                "Start the CI recipe's checks of a change: lint, build, the selected tests and audit in a test fork, and the secret scan; the run is listed under /v1/recipe-runs",
            )
            .errors(&[400, 404]),
        )
}
