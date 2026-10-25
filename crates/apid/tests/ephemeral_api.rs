//! The ephemeral routes on a booted hub (`/v1/projects/{id}/ephemeral`, `/v1/ephemeral/{instance}`): an environment that is not
//! ephemeral is refused, a body without its parts is a 400, an unknown project and instance are 404s, and an instance whose
//! platform is not connected fails in the background with its reason, which the list shows. The platforms themselves are
//! tested against fakes in `crates/apid/src/ephemeral/tests.rs`.

mod common;

use std::time::{Duration, Instant};

use common::{Hub, Scratch};
use serde_json::{Value, json};

fn project(hub: &Hub) -> String {
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Shop", "goal": "Sell things" })),
    );
    assert_eq!(status, 201, "{project}");
    project["id"].as_str().expect("an id").to_string()
}

fn put_environment(hub: &Hub, pid: &str, name: &str, variables: Value) {
    let (status, env) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/environments/{name}"),
        Some(json!({ "kind": "development", "track": "manual", "variables": variables })),
    );
    assert_eq!(status, 200, "{env}");
}

#[test]
fn the_ephemeral_routes_refuse_what_is_not_ephemeral_or_not_there() {
    let scratch = Scratch::new("ephemeral-api");
    let hub = Hub::start(&scratch, None, None);
    let pid = project(&hub);
    put_environment(
        &hub,
        &pid,
        "plain",
        json!({ "KUBERNETES_DEPLOYMENT": "web" }),
    );

    let (status, body) = hub.call("GET", &format!("/v1/projects/{pid}/ephemeral"), None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!([]), "a new project has no instances");

    let (status, body) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/ephemeral"),
        Some(json!({ "environment": "plain", "subject": { "lane": "ch-1" } })),
    );
    assert_eq!(status, 409, "{body}");
    assert!(body.to_string().contains("EPHEMERAL"), "{body}");

    let (status, _) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/ephemeral"),
        Some(json!({ "environment": "plain" })),
    );
    assert_eq!(status, 400, "a subject is named");
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/ephemeral"),
        Some(json!({ "environment": "plain", "subject": { "branch": "main" } })),
    );
    assert_eq!(status, 400, "only a lane or a release is a subject");

    let (status, _) = hub.call(
        "POST",
        "/v1/projects/nope/ephemeral",
        Some(json!({ "environment": "plain", "subject": { "lane": "ch-1" } })),
    );
    assert_eq!(status, 404);
    let (status, _) = hub.call("GET", "/v1/projects/nope/ephemeral", None);
    assert_eq!(status, 404);
    for (method, path) in [
        ("GET", "/v1/ephemeral/eph-nope"),
        ("DELETE", "/v1/ephemeral/eph-nope"),
    ] {
        let (status, body) = hub.call(method, path, None);
        assert_eq!(status, 404, "{method} {path}: {body}");
    }
}

#[test]
fn an_instance_whose_platform_is_not_connected_fails_in_the_background_with_its_reason() {
    let scratch = Scratch::new("ephemeral-fail");
    let hub = Hub::start(&scratch, None, None);
    let pid = project(&hub);
    put_environment(
        &hub,
        &pid,
        "pr",
        json!({ "EPHEMERAL": "true", "KUBERNETES_DEPLOYMENT": "web", "EPHEMERAL_TTL_MINUTES": "30" }),
    );

    let (status, started) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/ephemeral"),
        Some(json!({ "environment": "pr", "subject": { "lane": "ch-1" }, "ttl_minutes": 10 })),
    );
    assert_eq!(status, 202, "{started}");
    let id = started["id"].as_str().expect("an id").to_string();
    assert_eq!(started["state"]["state"], json!("creating"), "{started}");
    assert_eq!(started["target"]["platform"], json!("kubernetes"));
    assert_eq!(
        started["target"]["namespace"],
        json!(format!("apz-{pid}-pr-lane-ch-1-{}", &id[4..12]))
    );
    assert_eq!(
        started["expires_ms"].as_u64().unwrap() - started["created_ms"].as_u64().unwrap(),
        10 * 60_000
    );

    // The rollout has no cluster to reach: the instance is Failed, and says why.
    let deadline = Instant::now() + Duration::from_secs(20);
    let failed = loop {
        let (status, listed) = hub.call("GET", &format!("/v1/projects/{pid}/ephemeral"), None);
        assert_eq!(status, 200, "{listed}");
        let found = listed
            .as_array()
            .and_then(|list| list.iter().find(|i| i["id"] == json!(id)).cloned());
        if let Some(instance) = found.filter(|i| i["state"]["state"] == json!("failed")) {
            break instance;
        }
        assert!(
            Instant::now() < deadline,
            "the instance never failed: {listed}\n{}",
            hub.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        !failed["state"]["reason"].as_str().unwrap_or("").is_empty(),
        "{failed}"
    );

    let (status, one) = hub.call("GET", &format!("/v1/ephemeral/{id}"), None);
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["id"], json!(id));
    assert_eq!(one["subject"], json!({ "lane": "ch-1" }));
}
