//! The triggers' routes (PLAN 14.6): a recipe's triggers, the owner's hand, and the connectors' webhooks (`hooks.rs`).
//!
//! | Route | Answer |
//! |---|---|
//! | `POST /v1/hooks/{connector}/{token}` | a connector's delivery (public; `hooks.rs`) |
//! | `POST /v1/connectors/{id}/webhook` | the connector's webhook token, shown once (`hooks.rs`) |
//! | `GET /v1/recipes/{id}/triggers` | the recipe's triggers |
//! | `POST /v1/recipes/{id}/triggers` `{source, event, label?}` or `{cron \| every_minutes, project?}` | adds a trigger |
//! | `DELETE /v1/recipes/{id}/triggers/{tid}` | removes one |
//! | `POST /v1/recipes/{id}/fire` `{project?}` | the owner's hand: starts the recipe's run now |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use super::{Action, DEFAULT_WINDOW_MS, TriggerRow, cron, engine_only, hooks};
use crate::recipes::routes::summary;
use crate::state::App;

fn body(req: &Request) -> Result<Value, Response> {
    if req.body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&req.body).map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

fn list_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let recipe = p.get("id");
    if app.recipe_runs.recipe(recipe, None).is_none() {
        return Response::error(404, "no such recipe");
    }
    Response::json(200, &json!({"triggers": app.triggers.of_recipe(recipe)}))
}

fn add_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let recipe = p.get("id");
    if app.recipe_runs.recipe(recipe, None).is_none() {
        return Response::error(404, "no such recipe");
    }
    if !engine_only(&app.recipe_runs, recipe) {
        return Response::error(
            409,
            "that recipe's steps wait for a task: a trigger cannot start it",
        );
    }
    let body = match body(req) {
        Ok(body) => body,
        Err(res) => return res,
    };
    let cron_text = body["cron"].as_str().unwrap_or("").trim().to_string();
    let every = body["every_minutes"].as_u64().unwrap_or(0);
    if !cron_text.is_empty()
        && let Err(why) = cron::parse(&cron_text)
    {
        return Response::error(400, &why);
    }
    if every > 10_080 {
        return Response::error(400, "every_minutes is 0 (none) to 10080 (a week)");
    }
    let schedule = !cron_text.is_empty() || every > 0;
    let event = body["event"].as_str().unwrap_or("").to_string();
    let source = body["source"].as_str().unwrap_or("").to_string();
    if schedule && (!event.is_empty() || !source.is_empty()) {
        return Response::error(400, "a trigger is a schedule or an event, not both");
    }
    if !schedule && (event.is_empty() || source.is_empty()) {
        return Response::error(
            400,
            "a trigger needs a schedule (cron or every_minutes) or an event and its source",
        );
    }
    let project = body["project"].as_str().map(str::to_string);
    if project
        .as_deref()
        .is_some_and(|id| app.projects.get(id).is_none())
    {
        return Response::error(404, "no such project");
    }
    let row = TriggerRow {
        id: String::new(),
        recipe: recipe.to_string(),
        project,
        action: Action::Run,
        source: if schedule { "schedule".into() } else { source },
        event: if schedule { String::new() } else { event },
        label: body["label"].as_str().unwrap_or("").to_string(),
        cron: cron_text,
        every_minutes: every as u32,
        window_ms: if schedule {
            0
        } else {
            body["window_ms"].as_u64().unwrap_or(DEFAULT_WINDOW_MS)
        },
        enabled: true,
        next_ms: None,
        last_ms: None,
        created_ms: 0,
    };
    let row = app.triggers.add(row, apz_plaza::now_ms());
    Response::json(201, &row)
}

fn remove_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let (recipe, id) = (p.get("id"), p.get("tid"));
    if app.triggers.get(id).is_none_or(|t| t.recipe != recipe) {
        return Response::error(404, "no such trigger");
    }
    app.triggers.remove(id);
    Response::json(200, &json!({"removed": id}))
}

fn fire_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let recipe = p.get("id");
    if app.recipe_runs.recipe(recipe, None).is_none() {
        return Response::error(404, "no such recipe");
    }
    if !engine_only(&app.recipe_runs, recipe) {
        return Response::error(
            409,
            "that recipe's steps wait for a task: the owner's hand cannot start it",
        );
    }
    let body = match body(req) {
        Ok(body) => body,
        Err(res) => return res,
    };
    let project = body["project"].as_str();
    if project.is_some_and(|id| app.projects.get(id).is_none()) {
        return Response::error(404, "no such project");
    }
    // The change a user test is about (19), when the owner names one.
    let change = body["change"].as_str();
    let now = apz_plaza::now_ms();
    let Some(run) = app
        .recipe_runs
        .start_engine(recipe, "owner", None, project, change, now)
    else {
        return Response::error(404, "no such recipe");
    };
    crate::user_test::queue(&run);
    match app.recipe_runs.view(&run) {
        Some(v) => Response::json(202, &summary(&v)),
        None => Response::error(404, "no such recipe run"),
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    let router = hooks::routes(router);
    router
        .get("/v1/recipes/{id}/triggers", list_route)
        .doc(RouteDoc::new("List a recipe's triggers").errors(&[404]))
        .post("/v1/recipes/{id}/triggers", add_route)
        .doc(
            RouteDoc::new(
                "Add a trigger to a recipe: an event from a source (with an optional label filter and debounce window), or a schedule (cron or every_minutes)",
            )
            .status(201)
            .errors(&[400, 404, 409]),
        )
        .delete("/v1/recipes/{id}/triggers/{tid}", remove_route)
        .doc(RouteDoc::new("Remove a trigger of a recipe").errors(&[404]))
        .post("/v1/recipes/{id}/fire", fire_route)
        .doc(
            RouteDoc::new("The owner's hand: start a recipe's run now (its engine steps; listed under /v1/recipe-runs)")
                .status(202)
                .errors(&[404, 409]),
        )
}
