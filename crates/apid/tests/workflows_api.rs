//! The workflows' routes on a scratch hub (PLAN 21, 23): a project that is not there answers 404 on every workflow route, a
//! project's workflow list reads its repo on main (the list is empty for a repo with no `.plaza/workflows`), a run with both
//! a change and a release is refused, and a workflow that is not there is not started.

mod common;

use common::{Hub, Scratch};
use serde_json::json;

#[test]
fn the_workflow_routes_answer_on_a_scratch_hub() {
    let scratch = Scratch::new("workflows-api");
    let hub = Hub::start(&scratch, None, None);

    // Every route is 404 for a project that is not there.
    let missing = [
        ("GET", "/v1/projects/no-such-project/workflows", None),
        ("GET", "/v1/projects/no-such-project/workflows/import", None),
        ("GET", "/v1/projects/no-such-project/workflows/ship", None),
        (
            "GET",
            "/v1/projects/no-such-project/workflows/ship/runs",
            None,
        ),
        (
            "POST",
            "/v1/projects/no-such-project/workflows/ship/runs",
            Some(json!({ "change": "c1" })),
        ),
        (
            "POST",
            "/v1/projects/no-such-project/workflows/import",
            Some(json!({ "file": ".github/workflows/ci.yml", "save": true })),
        ),
    ];
    for (method, path, body) in missing {
        let (status, reply) = hub.call(method, path, body);
        assert_eq!(status, 404, "{method} {path}: {reply}");
    }

    // A project: its workflows are read from its repo on main.
    let (status, created) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Shop", "goal": "a shop", "kind": "code" })),
    );
    assert!(
        status == 200 || status == 201,
        "the project is made: {created}"
    );
    let id = created["id"]
        .as_str()
        .expect("the project has an id")
        .to_string();

    let (status, list) = hub.call("GET", &format!("/v1/projects/{id}/workflows"), None);
    if status == 200 {
        assert!(
            list["workflows"].as_array().is_some_and(|w| w.is_empty()),
            "a repo with no .plaza/workflows lists none: {list}"
        );
    } else {
        // A project with no repo yet cannot be read: the route says so rather than failing.
        assert_eq!(status, 409, "{list}");
    }

    // A run for a change and a release together is refused before anything is read.
    let (status, reply) = hub.call(
        "POST",
        &format!("/v1/projects/{id}/workflows/ship/runs"),
        Some(json!({ "change": "c1", "release": "1.0.0" })),
    );
    assert_eq!(status, 400, "{reply}");

    // A workflow the project does not have is not started.
    let (status, reply) = hub.call(
        "POST",
        &format!("/v1/projects/{id}/workflows/ship/runs"),
        Some(json!({})),
    );
    assert_eq!(status, 409, "{reply}");

    // A name that is not a workflow's is not found.
    let (status, _) = hub.call(
        "GET",
        &format!("/v1/projects/{id}/workflows/no%20such"),
        None,
    );
    assert_eq!(status, 404);
}
