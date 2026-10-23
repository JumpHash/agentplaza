//! A lab run's observations over the hub's routes (PLAN 22.7): the run's collector posts its logs with the run's token,
//! a wrong token and a finished run are refused, the owner reads what was kept, and the topology's no-errors step over
//! the done run fails on the seeded error and cites its line.

mod common;

use serde_json::{Value, json};

use common::{Hub, Scratch, text};

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
