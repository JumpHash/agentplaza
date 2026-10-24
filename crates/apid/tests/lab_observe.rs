//! A lab run's observations over the hub's routes (PLAN 22.7): the run's collector posts its logs with the run's token,
//! a wrong token and a finished run are refused, the owner reads what was kept, and the topology's no-errors step over
//! the done run fails on the seeded error and cites its line.

mod common;

use std::time::Duration;

use serde_json::{Value, json};

use common::{Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(30);

const TOKEN: &str = "x-apz-lab-token";
const TOPOLOGY: &str = "name = \"fake-run\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\n[[assert]]\nkind = \"no_errors\"\n";

/// The OTLP logs request a collector posts for one ERROR line of service `api-1`.
fn seeded_error() -> Vec<u8> {
    json!({"resourceLogs": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "api-1"}}]},
        "scopeLogs": [{"logRecords": [{
            "timeUnixNano": "1700000000000000000",
            "severityText": "ERROR",
            "body": {"stringValue": "ERROR seeded failure in the cache"},
        }]}],
    }]})
    .to_string()
    .into_bytes()
}

#[test]
fn a_seeded_error_from_a_collector_fails_the_topology_step_over_the_routes() {
    let scratch = Scratch::new("lab-observe");
    let hub = Hub::start(&scratch, None, None);

    let (status, run) = hub.call(
        "POST",
        "/v1/test-lab/runs",
        Some(
            json!({"project": "p", "channel": "stable", "cpus": 1, "memory_mib": 512,
            "change": "c1", "topology": "fake-run"}),
        ),
    );
    assert_eq!(status, 201, "{run}");
    let id = text(&run["id"]);
    let token = text(&run["ingest_token"]);
    let post = |token: &str, signal: &str, body: &[u8]| {
        hub.send_with_headers(
            "POST",
            &format!("/v1/test-lab/runs/{id}/otlp/{signal}"),
            &[(TOKEN, token)],
            body,
        )
    };

    // Assessing waits for the run to end.
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/test-lab/runs/{id}/assess"),
        Some(json!({"topology": TOPOLOGY})),
    );
    assert_eq!(status, 409);

    // Another token is refused; the run's own is kept.
    let (status, _) = post("not-the-token", "logs", &seeded_error());
    assert_eq!(status, 401);
    let (status, accepted) = post(&token, "logs", &seeded_error());
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["records"], json!(1));

    let (status, shown) = hub.call("GET", &format!("/v1/test-lab/runs/{id}/observations"), None);
    assert_eq!(status, 200, "{shown}");
    assert_eq!(shown["logs"], json!(1));
    assert_eq!(shown["errors"], json!(1));

    // The run ends: it takes no more observations, and its topology's step is checked.
    let (status, _) = hub.call("POST", &format!("/v1/test-lab/runs/{id}/cancel"), None);
    assert_eq!(status, 200);
    let (status, _) = post(&token, "logs", &seeded_error());
    assert_eq!(status, 409);

    let (status, assessed) = hub.call(
        "POST",
        &format!("/v1/test-lab/runs/{id}/assess"),
        Some(json!({"topology": TOPOLOGY})),
    );
    assert_eq!(status, 200, "{assessed}");
    let verdict: &Value = &assessed["verdicts"][0];
    assert_eq!(verdict["id"], json!("no_errors"));
    assert_eq!(verdict["passed"], json!(false), "{assessed}");
    assert!(
        text(&verdict["cites"][0]).contains("seeded failure in the cache"),
        "{assessed}"
    );

    // A run the hub does not know has no observations, and a topology it does not name is refused.
    let (status, _) = hub.call("GET", "/v1/test-lab/runs/q999/observations", None);
    assert_eq!(status, 404);
    let (status, _) = hub.call(
        "POST",
        "/v1/test-lab/runs/q999/assess",
        Some(json!({"topology": TOPOLOGY})),
    );
    assert_eq!(status, 404);
}

/// Lands `files` on a repository's main through the hub's Weft.
fn land(hub: &Hub, repo: &str, title: &str, files: &[(&str, &str)]) {
    let files: Vec<Value> = files
        .iter()
        .map(|(path, text)| json!({ "path": path, "text": text }))
        .collect();
    let (status, body) = hub.call(
        "POST",
        &format!("/v1/weft/repos/{repo}/edits"),
        Some(json!({ "title": title, "files": files })),
    );
    assert!(
        status == 200 || status == 201,
        "{status} {body}\n{}",
        hub.log()
    );
}

#[test]
fn a_run_is_assessed_as_it_ends_so_its_verdicts_are_there_without_a_manual_assessment() {
    let scratch = Scratch::new("lab-observe-ended");
    let hub = Hub::start(&scratch, None, None);
    hub.wait_for("/v1/weft", WAIT, |w| w["running"] == true);

    // The project declares its topology in its repository, as a project does.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Ended", "goal": "Its lab runs", "kind": "code" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let project = hub.wait_for(&format!("/v1/projects/{pid}"), WAIT, |p| {
        p["repo"].is_string()
    });
    land(
        &hub,
        &text(&project["repo"]),
        "Declare the topology",
        &[(".plaza/topologies/fake-run.toml", TOPOLOGY)],
    );

    let (status, run) = hub.call(
        "POST",
        "/v1/test-lab/runs",
        Some(
            json!({"project": pid, "channel": "stable", "cpus": 1, "memory_mib": 512,
            "change": "c1", "topology": "fake-run"}),
        ),
    );
    assert_eq!(status, 201, "{run}");
    let id = text(&run["id"]);
    let token = text(&run["ingest_token"]);
    let (status, accepted) = hub.send_with_headers(
        "POST",
        &format!("/v1/test-lab/runs/{id}/otlp/logs"),
        &[(TOKEN, &token)],
        &seeded_error(),
    );
    assert_eq!(status, 200, "{accepted}");

    // The run ends: nothing else is asked, and its topology's verdicts are there.
    let (status, ended) = hub.call(
        "POST",
        &format!("/v1/test-lab/runs/{id}/end"),
        Some(json!({"outcome": "passed"})),
    );
    assert_eq!(status, 200, "{ended}");
    let (status, shown) = hub.call("GET", &format!("/v1/test-lab/runs/{id}/observations"), None);
    assert_eq!(status, 200, "{shown}");
    let verdict = shown["verdicts"]
        .as_array()
        .and_then(|list| list.iter().find(|v| v["id"] == "no_errors"))
        .unwrap_or_else(|| panic!("the run was not assessed at its end: {shown}"));
    assert_eq!(verdict["passed"], json!(false), "{shown}");
    assert!(
        text(&verdict["cites"][0]).contains("seeded failure in the cache"),
        "{shown}"
    );

    // An ended run is judged once: it is not ended again.
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/test-lab/runs/{id}/end"),
        Some(json!({"outcome": "passed"})),
    );
    assert_eq!(status, 409);
}
