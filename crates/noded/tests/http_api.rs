//! The daemon's HTTP API, exercised over a real socket on an ephemeral port.
//!
//! The runs here boot from a kernel path nothing exists at, so a create
//! request always ends as `failed` - which is what lets the create, kill and
//! input paths be tested without /dev/kvm or the guest images. What that
//! cannot reach is a run whose VMs are up: the running-run paths (serial
//! captures, console input reaching a VM, a run ending as `done`) are covered
//! by the gated live smoke test in `tests/smoke.rs`.

mod common;

use common::request;

/// A create request for one 128 MiB VM, which always fails to boot here.
const UNBOOTABLE: &str = r#"{"n":1,"vcpus":1,"memory_mib":128}"#;

/// Creates a run that fails to boot, and returns its id.
fn failed_run(port: u16) -> String {
    let reply = request(port, "POST", "/v1/runs", Some(UNBOOTABLE));
    assert_eq!(reply.status, 201, "{}", reply.body);
    reply.json()["id"].as_str().expect("an id").to_string()
}

#[test]
fn the_run_list_starts_empty() {
    let server = common::start();
    let reply = request(server.port(), "GET", "/v1/runs", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(reply.content_type.starts_with("application/json"));
    assert_eq!(reply.json(), serde_json::json!({ "runs": [] }));
}

#[test]
fn unknown_runs_are_404() {
    let server = common::start();
    let port = server.port();
    for (method, target, body) in [
        ("GET", "/v1/runs/r7", None),
        ("DELETE", "/v1/runs/r7", None),
        (
            "POST",
            "/v1/runs/r7/input",
            Some(r#"{"data":"cpu","vm":0}"#),
        ),
    ] {
        let reply = request(port, method, target, body);
        assert_eq!(reply.status, 404, "{method} {target}: {}", reply.body);
        assert!(
            reply.json()["error"].is_string(),
            "{method} {target}: {}",
            reply.body
        );
    }
}

#[test]
fn create_requests_outside_the_contract_are_400() {
    let server = common::start();
    let port = server.port();
    for body in [
        r#"{"n":0,"vcpus":1,"memory_mib":128}"#,
        r#"{"n":65,"vcpus":1,"memory_mib":128}"#,
        r#"{"n":1,"vcpus":0,"memory_mib":128}"#,
        r#"{"n":1,"vcpus":65,"memory_mib":128}"#,
        r#"{"n":1,"vcpus":1,"memory_mib":127}"#,
        r#"{"n":1,"vcpus":1}"#,
        r#"{"n":1,"vcpus":1,"memory_mib":"lots"}"#,
        r#"{"n":-1,"vcpus":1,"memory_mib":128}"#,
        r#"not json at all"#,
        r#"[1,2,3]"#,
        r#"{"n":1,"vcpus":1,"memory_mib":128,"beats":"two"}"#,
        "",
    ] {
        let reply = request(port, "POST", "/v1/runs", Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }
    // A refused create leaves no run behind.
    assert_eq!(
        request(port, "GET", "/v1/runs", None).json(),
        serde_json::json!({ "runs": [] })
    );
}

#[test]
fn the_event_socket_is_a_websocket_only() {
    // A plain GET (no upgrade headers) is refused with a 4xx and no upgrade,
    // and the old SSE path is gone. A full WebSocket round trip is covered by
    // apz-http's own tests.
    let server = common::start();
    let port = server.port();
    let reply = request(port, "GET", "/v1/ws", None);
    assert!(
        (400..500).contains(&reply.status),
        "a plain GET is refused: {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(request(port, "GET", "/v1/events", None).status, 404);
}

#[test]
fn a_run_whose_vms_cannot_boot_is_failed_and_readable() {
    let server = common::start();
    let port = server.port();
    let reply = request(
        port,
        "POST",
        "/v1/runs",
        Some(r#"{"n":2,"vcpus":2,"memory_mib":256,"beats":2}"#),
    );
    assert_eq!(reply.status, 201, "{}", reply.body);
    let created = reply.json();
    assert_eq!(created["id"], "r1");
    assert_eq!(created["state"], "failed");
    assert_eq!(created["vcpus"], 2);
    assert_eq!(created["memory_mib"], 256);
    assert_eq!(created["beats"], 2);
    assert!(created["created_at"].as_u64().expect("created_at") > 0);
    assert!(created["ended_at"].as_u64().expect("ended_at") > 0);
    assert_eq!(created["vms"], serde_json::json!([]));
    // The list is what the create answered: no serial captures.
    assert!(created.get("serial").is_none());

    let single = request(port, "GET", "/v1/runs/r1", None);
    assert_eq!(single.status, 200, "{}", single.body);
    assert_eq!(single.json(), created);

    let list = request(port, "GET", "/v1/runs", None);
    assert_eq!(list.json()["runs"][0]["id"], "r1");
    assert_eq!(list.json()["runs"].as_array().expect("an array").len(), 1);

    // A second run gets the next id.
    assert_eq!(failed_run(port), "r2");
    assert_eq!(
        request(port, "GET", "/v1/runs", None).json()["runs"]
            .as_array()
            .expect("an array")
            .len(),
        2
    );
}

#[test]
fn a_run_without_beats_reports_null() {
    let server = common::start();
    let port = server.port();
    failed_run(port);
    let run = request(port, "GET", "/v1/runs/r1", None).json();
    assert!(run["beats"].is_null(), "{run}");
}

#[test]
fn a_finished_run_is_idempotent_to_kill_and_refuses_input() {
    let server = common::start();
    let port = server.port();
    let id = failed_run(port);
    let ended = request(port, "GET", &format!("/v1/runs/{id}"), None).json()["ended_at"]
        .as_u64()
        .expect("ended_at");

    // Already finished: killing it again changes nothing and is not an error.
    for _ in 0..2 {
        let reply = request(port, "DELETE", &format!("/v1/runs/{id}"), None);
        assert_eq!(reply.status, 200, "{}", reply.body);
        assert_eq!(reply.json()["state"], "failed");
        assert_eq!(reply.json()["ended_at"].as_u64(), Some(ended));
    }

    // A run that has ended has no console to write to.
    let input = request(
        port,
        "POST",
        &format!("/v1/runs/{id}/input"),
        Some(r#"{"data":"cpu","vm":0}"#),
    );
    assert_eq!(input.status, 409, "{}", input.body);
    assert!(input.json()["error"].is_string());

    // ... and a bad input request is still a 400.
    let bad = request(
        port,
        "POST",
        &format!("/v1/runs/{id}/input"),
        Some(r#"{"data":123,"vm":0}"#),
    );
    assert_eq!(bad.status, 400, "{}", bad.body);
}

#[test]
fn a_kill_racing_a_boot_leaves_a_consistent_run() {
    let server = common::start();
    let port = server.port();
    for round in 0..8 {
        // The run ids are the registry's, in creation order, so the round's
        // run is known before the create has answered.
        let id = format!("r{}", round + 1);
        let creating =
            std::thread::spawn(move || request(port, "POST", "/v1/runs", Some(UNBOOTABLE)));
        let killing = std::thread::spawn({
            let id = id.clone();
            move || request(port, "DELETE", &format!("/v1/runs/{id}"), None)
        });
        let created = creating.join().expect("the create thread");
        let killed = killing.join().expect("the kill thread");
        assert_eq!(created.status, 201, "{}", created.body);
        assert!(
            killed.status == 200 || killed.status == 404,
            "a kill racing a boot answered {}: {}",
            killed.status,
            killed.body
        );

        // Whatever the interleaving, the run is over, it ended once, and no VM
        // it managed to boot was left behind.
        let run = request(port, "GET", &format!("/v1/runs/{id}"), None).json();
        let state = run["state"].as_str().expect("a state");
        assert!(state == "failed" || state == "killed", "{run}");
        assert!(run["ended_at"].is_u64(), "{run}");
        assert_eq!(run["vms"], serde_json::json!([]), "{run}");
    }
}
