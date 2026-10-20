//! The review graph's routes: recipes, a task's runs, a run and its scored tree.
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /v1/recipes` | every recipe version, and the orchestrator's proposals |
//! | `GET /v1/recipes/{id}?version=` | one recipe, with its TOML |
//! | `PUT /v1/recipes/{id}` | a new version (the owner's), `{toml}` |
//! | `POST /v1/recipes/proposals/{id}` | the owner's decision on a proposal, `{approve}` |
//! | `GET /v1/tasks/{id}/reviews` | the task's runs, newest first |
//! | `GET /v1/reviews/{run}` | a run: its nodes, scores, findings, the gate |
//! | `GET /v1/reviews/{run}/tree` | the change tree with each definition's scores, findings and touches |

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use crate::state::App;

fn run_summary(r: &super::Run) -> Value {
    json!({
        "run": r.id, "task": r.task, "change": r.change, "revision": r.revision, "recipe": r.recipe, "round": r.round, "rounds": r.rounds,
        "kind": r.kind, "state": r.state, "verdict": r.sentence(), "outcome": r.outcome,
        "readings": r.readings().into_iter().map(|(n, v)| json!({ "name": n, "level": v })).collect::<Vec<_>>(),
        "nodes": r.nodes.iter().map(|n| json!({ "id": n.def.id, "kind": n.def.kind, "role": n.def.role, "state": n.state, "assignee": n.assignee, "parent": n.parent })).collect::<Vec<_>>(),
        "created_ms": r.created_ms, "updated_ms": r.updated_ms,
    })
}

fn run_view(r: &super::Run) -> Value {
    let mut v = run_summary(r);
    v["nodes"] = json!(r.nodes);
    v["lows"] = json!(r.lows());
    v["rules"] = json!(r.rules);
    v
}

/// The tree the views draw: each definition with its scores, findings and touches merged in.
fn scored_tree(app: &App, r: &super::Run) -> Value {
    let mut tree = r.tree.clone();
    let scores: Vec<&super::Score> = r.scores().collect();
    let findings: Vec<super::Finding> = r
        .nodes
        .iter()
        .flat_map(|n| n.findings.clone())
        .chain(super::carried_findings(app, r))
        .collect();
    let touches: Vec<&super::Touch> = r.nodes.iter().flat_map(|n| n.touches.iter()).collect();
    if let Some(files) = tree["files"].as_array_mut() {
        for f in files {
            let path = f["path"].as_str().unwrap_or("").to_string();
            let file_scores: Vec<&&super::Score> =
                scores.iter().filter(|s| s.symbol == path).collect();
            f["scores"] = json!(file_scores);
            if let Some(syms) = f["symbols"].as_array_mut() {
                for s in syms {
                    let id = s["id"].as_str().unwrap_or("").to_string();
                    let lineage = s["lineage"].as_str().map(str::to_string);
                    let mine = |sym: &str, lin: &Option<String>| {
                        sym == id || (lin.is_some() && *lin == lineage)
                    };
                    s["scores"] = json!(
                        scores
                            .iter()
                            .filter(|x| mine(&x.symbol, &x.lineage))
                            .collect::<Vec<_>>()
                    );
                    s["findings"] = json!(
                        findings
                            .iter()
                            .filter(|x| mine(&x.symbol, &x.lineage))
                            .collect::<Vec<_>>()
                    );
                    s["worded_touches"] =
                        json!(touches.iter().filter(|t| t.from == id).collect::<Vec<_>>());
                }
            }
        }
    }
    json!({ "run": r.id, "verdict": r.sentence(), "readings": run_summary(r)["readings"], "lows": r.lows(), "tree": tree })
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/recipes", |app, _, _| {
            let recipes: Vec<Value> = app.reviews.recipes().into_iter().map(|r| json!({ "id": r.id, "version": r.version, "title": r.title, "applies": r.applies, "axes": r.axes, "rounds": r.rounds, "nodes": r.nodes.len() })).collect();
            Response::json(200, &json!({ "recipes": recipes, "proposals": app.reviews.proposals() }))
        })
        .doc(RouteDoc::new("List the review recipes and the orchestrator's proposals"))
        .get("/v1/recipes/{id}", |app, req: &Request, p: &Params<'_>| {
            let version = req.query_param("version").and_then(|v| v.parse().ok());
            match app.reviews.recipe(p.get("id"), version) {
                Some(r) => Response::json(200, &json!({ "recipe": r, "toml": r.to_toml() })),
                None => Response::error(404, "no such recipe"),
            }
        })
        .doc(RouteDoc::new("Show a review recipe").errors(&[404]))
        .route("PUT", "/v1/recipes/{id}", |app, req: &Request, p: &Params<'_>| {
            let body: Value = req.json().unwrap_or(Value::Null);
            let parsed = match body["toml"].as_str() {
                Some(t) => super::recipe::Recipe::parse(t),
                None => serde_json::from_value::<super::recipe::Recipe>(body.clone()).map_err(|e| e.to_string()),
            };
            match parsed.and_then(|r| if r.id == p.get("id") { app.reviews.put_recipe(r) } else { Err("the recipe's id is not the route's".into()) }) {
                Ok(r) => {
                    app.bus.publish("recipe", &json!({ "recipe": r.key() }));
                    Response::json(200, &json!({ "recipe": r }))
                }
                Err(e) => Response::error(400, &e),
            }
        })
        .doc(RouteDoc::new("Save a new version of a review recipe").errors(&[400]))
        .post("/v1/recipes/proposals/{id}", |app, req: &Request, p: &Params<'_>| {
            let approve = req.json::<Value>().ok().and_then(|v| v["approve"].as_bool()).unwrap_or(false);
            match app.reviews.decide_proposal(p.get("id"), approve) {
                Ok(pr) => Response::json(200, &json!(pr)),
                Err(e) => Response::error(409, &e),
            }
        })
        .doc(RouteDoc::new("Approve or decline a proposed recipe").errors(&[409]))
        .get("/v1/tasks/{id}/reviews", |app, _, p: &Params<'_>| {
            // A task nothing has reviewed yet lists no runs; a task that is not there is a 404.
            if app.plaza.tasks.find(p.get("id")).is_none() {
                return Response::error(404, "no such task");
            }
            let runs: Vec<Value> = app.reviews.runs_of(p.get("id")).iter().map(run_summary).collect();
            Response::json(200, &json!({ "task": p.get("id"), "runs": runs }))
        })
        .doc(RouteDoc::new("List a task's review runs, newest first").errors(&[404]))
        .get("/v1/reviews/{run}", |app, _, p: &Params<'_>| match app.reviews.run(p.get("run")) {
            Some(r) => Response::json(200, &run_view(&r)),
            None => Response::error(404, "no such review"),
        })
        .doc(RouteDoc::new("Show a review run: its steps, scores, findings and gate").errors(&[404]))
        .get("/v1/reviews/{run}/tree", |app, _, p: &Params<'_>| match app.reviews.run(p.get("run")) {
            Some(r) => Response::json(200, &scored_tree(app, &r)),
            None => Response::error(404, "no such review"),
        })
        .doc(RouteDoc::new("Show a review run's change tree with scores, findings and touches").errors(&[404]))
}
