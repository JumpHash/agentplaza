//! The recipe engine's routes on a scratch hub (PLAN 4.1): the engine's recipes are listed beside the review recipes, the
//! change recipe answers with its steps and its loop, an owner's version must be newer than the built-in one and must
//! check, and the run routes answer 404 for a run that is not there.

mod common;

use common::{Hub, Scratch};
use serde_json::{Value, json};

#[test]
fn the_engine_recipes_and_their_saved_versions_answer_on_a_scratch_hub() {
    let scratch = Scratch::new("recipes-api");
    let hub = Hub::start(&scratch, None, None);

    let (status, list) = hub.call("GET", "/v1/recipes", None);
    assert_eq!(status, 200, "{list}");
    let ids: Vec<&str> = list["engine"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    for id in ["change", "review", "ci", "base", "distill"] {
        assert!(ids.contains(&id), "{id} is listed: {ids:?}");
    }

    let (status, change) = hub.call("GET", "/v1/recipes/change", None);
    assert_eq!(status, 200, "{change}");
    assert_eq!(change["engine"], json!(true));
    assert_eq!(change["recipe"]["steps"][1]["id"], json!("review"));
    assert_eq!(change["recipe"]["edges"][1]["max"], json!(2));

    // A version must be newer than the one there is, and must check.
    let same = json!({ "toml": "id = \"change\"\nversion = 1\n[[steps]]\nid = \"build\"\nrole = \"builder\"\n" });
    let (status, body) = hub.call("PUT", "/v1/recipes/change", Some(same));
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("higher"), "{body}");

    let pinned = json!({ "toml": "id = \"change\"\nversion = 2\n[[steps]]\nid = \"build\"\nrole = \"builder\"\nmodel = []\n" });
    let (status, body) = hub.call("PUT", "/v1/recipes/change", Some(pinned));
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("model list is empty"), "{body}");

    let newer = json!({ "toml": concat!(
        "id = \"change\"\nversion = 2\n",
        "[[steps]]\nid = \"build\"\nrole = \"builder\"\n",
        "[[steps]]\nid = \"review\"\nseat = \"reviewer\"\nrole = \"reviewer\"\nmodel = \"differ\"\n",
        "[[edges]]\nfrom = \"build\"\nto = \"review\"\nwhen = \"passed\"\n",
    ) });
    let (status, body) = hub.call("PUT", "/v1/recipes/change", Some(newer));
    assert_eq!(status, 200, "{body}");
    let (status, now) = hub.call("GET", "/v1/recipes/change", None);
    assert_eq!(status, 200, "{now}");
    assert_eq!(now["recipe"]["version"], json!(2));
    assert_eq!(now["recipe"]["edges"].as_array().unwrap().len(), 1);

    // Runs: none yet, and the routes for a run that is not there are 404.
    let (status, runs) = hub.call("GET", "/v1/recipe-runs", None);
    assert_eq!(status, 200, "{runs}");
    assert_eq!(runs["runs"], json!([]));
    let (status, _) = hub.call("GET", "/v1/recipe-runs/rr-0", None);
    assert_eq!(status, 404);
    let (status, _) = hub.call("GET", "/v1/recipe-runs/rr-0/graph", None);
    assert_eq!(status, 404);
    let (status, _) = hub.call("POST", "/v1/recipe-runs/rr-0/cancel", None);
    assert_eq!(status, 404);
}

fn error_of(body: &Value) -> String {
    body["error"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
}
