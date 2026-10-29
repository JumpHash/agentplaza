//! Projects opened while STOP ALL prevents Weft writes still have an honest, readable build plan.

mod common;

use std::time::Duration;

use common::{Hub, Scratch, text};
use serde_json::{Value, json};

#[test]
fn a_project_opened_while_stopped_does_not_claim_a_missing_repo_or_fail_its_build_plan() {
    let scratch = Scratch::new("held-project-repo");
    let hub = Hub::start(&scratch, None, None);
    let health = hub.wait_for("/v1/weft/health", Duration::from_secs(20), |h| {
        h["ok"] == true
    });
    assert_eq!(health["ok"], true, "{}", hub.log());

    let (status, _) = hub.call("POST", "/v1/plaza/stop", Some(json!({"reason": "hold"})));
    assert_eq!(status, 200);
    let (status, health) = hub.call("GET", "/v1/weft/health", None);
    assert_eq!(status, 200);
    assert_eq!(health["stopped"], true);

    let (status, opened) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({"title": "Held project", "goal": "Opened while stopped"})),
    );
    assert_eq!(status, 201, "{opened}");
    let id = text(&opened["id"]);
    let (status, saved) = hub.call("GET", &format!("/v1/projects/{id}"), None);
    assert_eq!(status, 200);
    assert_eq!(
        saved["repo"],
        Value::Null,
        "a refused creation is not an attachment"
    );
    assert_eq!(opened["repo"], saved["repo"]);

    let plan = format!("/v1/projects/{id}/build-plan");
    let (status, body) = hub.call("GET", &plan, None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["targets"], json!([]));

    assert_eq!(hub.call("POST", "/v1/plaza/resume", None).0, 200);
    assert_eq!(
        hub.call("POST", &format!("/v1/projects/{id}/archive"), None)
            .0,
        200
    );
    let (status, body) = hub.call("GET", &plan, None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["targets"], json!([]));

    // Once writes are allowed again, opening a project still creates and attaches its real repo.
    let (status, opened) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({"title": "Released project", "goal": "Opened after resume"})),
    );
    assert_eq!(status, 201, "{opened}");
    let repo = opened["repo"].as_str().expect("an attached repo");
    let (status, detail) = hub.call("GET", &format!("/v1/weft/repos/{repo}"), None);
    assert_eq!(status, 200, "{detail}");
    assert_eq!(
        hub.call("GET", "/v1/projects/no-such-project/build-plan", None)
            .0,
        404
    );
}
