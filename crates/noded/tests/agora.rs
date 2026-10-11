//! The agora's HTTP API, exercised over a real socket on an ephemeral port:
//! the call board, and the bid and award rules that can be reached without a
//! live agent.
//!
//! The daemon here boots no VMs, so no agent is connected: the refusals a bid
//! or an award answers with at this surface are the ones that come *first* —
//! an unknown task (404), a body outside the contract (400), an agent the
//! register does not show (409). What needs a connected agent — the accepted
//! bid, the role rule, the re-bid that replaces, the bid cap, the award that
//! performs the claim — is covered by the agora's own unit tests
//! (`crates/noded/src/agora.rs`, where a `ConnectedAgent` is built directly,
//! exactly as the claim rules are), and by the gated live test in
//! `tests/smoke.rs`, which boots a real one-role fleet over this same HTTP
//! surface.

mod common;

use std::fs;
use std::path::PathBuf;

use common::request;

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-agora-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a console directory");
    dir
}

/// Creates a task and returns its id.
fn task(port: u16, body: &str) -> String {
    let reply = request(port, "POST", "/api/v0/tasks", Some(body));
    assert_eq!(reply.status, 201, "{body}: {}", reply.body);
    reply.json()["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// The board the daemon serves.
fn board(port: u16) -> serde_json::Value {
    let reply = request(port, "GET", "/api/v0/agora", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("application/json"),
        "{}",
        reply.content_type
    );
    reply.json()
}

#[test]
fn the_call_board_starts_empty() {
    let server = common::start(console_dir("empty"));
    assert_eq!(
        board(server.port()),
        serde_json::json!({
            "calls": [],
            "total": 0,
            "limit": noded::agora::AGORA_LIMIT,
            "truncated": false,
        })
    );
}

#[test]
fn a_roled_open_task_is_a_call_and_an_unroled_one_is_not() {
    let server = common::start(console_dir("calls"));
    let port = server.port();

    // An unroled task is not a call: the board is derived from the ledger, not
    // a second registry of everything.
    let plain = task(port, r#"{"goal":"just a goal","budget":9}"#);
    assert_eq!(board(port)["total"], 0);
    // A bid on it is refused — by the register first, since this daemon has no
    // connected agent, exactly as the claim is refused; the call's own rule
    // (`task t1 names no role, so it is not a call`) is behind the register
    // check, and the agora's unit tests pin it there.
    let refused = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{plain}/bid"),
        Some(r#"{"agent_cid":100,"price":1}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        "no live agent has guest CID 100"
    );

    // A roled, budgeted task is a call, derived: capability, spec, ceiling,
    // state and creation all read off the ledger's own row.
    let called = task(
        port,
        r#"{"goal":"ship the agora","acceptance":"the suite is green","budget":500,"role":"builder"}"#,
    );
    let listed = request(port, "GET", &format!("/api/v0/tasks/{called}"), None).json();
    let view = board(port);
    assert_eq!(view["total"], 1);
    assert_eq!(view["truncated"], false);
    assert_eq!(
        view["calls"][0],
        serde_json::json!({
            "task": "t2",
            "capability": "builder",
            "spec_ref": "the suite is green",
            "max_price": 500,
            "state": "open",
            "created_at": listed["created_at"],
            "bids": [],
            "award": null,
        }),
        "{view}"
    );

    // An unbudgeted roled task is a call too — with a ceiling of zero, which
    // only a zero bid can meet.
    let free = task(port, r#"{"goal":"free work","role":"reviewer"}"#);
    let view = board(port);
    assert_eq!(view["total"], 2);
    assert_eq!(view["calls"][0]["task"], "t2", "oldest first");
    assert_eq!(view["calls"][1]["task"], free);
    assert_eq!(view["calls"][1]["capability"], "reviewer");
    assert_eq!(view["calls"][1]["max_price"], 0);
    assert_eq!(view["calls"][1]["spec_ref"], serde_json::Value::Null);

    // A roled task that ends before anything was offered through the agora
    // leaves the board: its offer is over and the agora never saw a bid.
    let withdrawn = task(port, r#"{"goal":"unwanted","role":"builder"}"#);
    assert_eq!(board(port)["total"], 3);
    let reply = request(port, "DELETE", &format!("/api/v0/tasks/{withdrawn}"), None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        board(port)["total"],
        2,
        "the withdrawn task's call is gone: {}",
        board(port)
    );
}

#[test]
fn bid_and_award_requests_outside_the_contract_are_400() {
    let server = common::start(console_dir("400"));
    let port = server.port();
    let id = task(
        port,
        r#"{"goal":"ship the agora","budget":500,"role":"builder"}"#,
    );

    // A bid needs the agent and a price, and the price is a non-negative
    // integer: the schema's floor is the wire type.
    for body in [
        r#"{}"#,
        r#"{"agent_cid":100}"#,
        r#"{"price":5}"#,
        r#"{"agent_cid":"x","price":5}"#,
        r#"{"agent_cid":100,"price":"free"}"#,
        r#"{"agent_cid":100,"price":-1}"#,
        r#"{"agent_cid":100,"price":1.5}"#,
        "not json at all",
        "[]",
        "",
    ] {
        let reply = request(port, "POST", &format!("/api/v0/tasks/{id}/bid"), Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }
    // An award needs the agent and nothing else: a price would have nothing to
    // match the bid with.
    for body in [r#"{}"#, r#"{"price":5}"#, r#"{"agent_cid":"x"}"#, "[]"] {
        let reply = request(port, "POST", &format!("/api/v0/tasks/{id}/award"), Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }

    // None of those refusals recorded anything or moved the task.
    assert_eq!(board(port)["calls"][0]["bids"], serde_json::json!([]));
    assert_eq!(board(port)["calls"][0]["award"], serde_json::Value::Null);
    assert_eq!(
        request(port, "GET", &format!("/api/v0/tasks/{id}"), None).json()["state"],
        "open"
    );
}

#[test]
fn a_bid_or_an_award_needs_a_connected_agent_of_the_role() {
    let server = common::start(console_dir("register"));
    let port = server.port();
    let id = task(
        port,
        r#"{"goal":"ship the agora","budget":500,"role":"builder"}"#,
    );

    // This daemon has booted no VMs, so no CID is connected — not one it never
    // had, and not one a failed run was given. A well-formed bid is refused by
    // the register, with the claim's own words and status.
    let failed = request(
        port,
        "POST",
        "/api/v0/runs",
        Some(r#"{"n":1,"vcpus":1,"memory_mib":128,"vsock_cid":100}"#),
    );
    assert_eq!(failed.status, 201, "{}", failed.body);
    assert_eq!(failed.json()["state"], "failed", "{}", failed.body);

    for cid in [100, 7, 0, u64::MAX] {
        let reply = request(
            port,
            "POST",
            &format!("/api/v0/tasks/{id}/bid"),
            Some(&format!(r#"{{"agent_cid":{cid},"price":5}}"#)),
        );
        assert_eq!(reply.status, 409, "cid {cid}: {}", reply.body);
        assert_eq!(
            reply.json()["error"],
            format!("no live agent has guest CID {cid}"),
            "cid {cid}"
        );
        let reply = request(
            port,
            "POST",
            &format!("/api/v0/tasks/{id}/award"),
            Some(&format!(r#"{{"agent_cid":{cid}}}"#)),
        );
        assert_eq!(reply.status, 409, "cid {cid}: {}", reply.body);
        assert_eq!(
            reply.json()["error"],
            format!("no live agent has guest CID {cid}"),
            "cid {cid}"
        );
    }
    // The register refused before the call was touched.
    assert_eq!(board(port)["calls"][0]["bids"], serde_json::json!([]));
    assert_eq!(board(port)["calls"][0]["award"], serde_json::Value::Null);
}

#[test]
fn unknown_calls_are_404_and_wrong_methods_are_405() {
    let server = common::start(console_dir("404-405"));
    let port = server.port();
    let id = task(
        port,
        r#"{"goal":"ship the agora","budget":500,"role":"builder"}"#,
    );

    for (method, target, body) in [
        (
            "POST",
            "/api/v0/tasks/t9/bid".to_string(),
            Some(r#"{"agent_cid":100,"price":5}"#),
        ),
        (
            "POST",
            "/api/v0/tasks/t9/award".to_string(),
            Some(r#"{"agent_cid":100}"#),
        ),
        // Paths under the agora's own names that are not endpoints.
        ("GET", "/api/v0/agora/calls".to_string(), None),
        ("GET", "/api/v0/tasks/t1/bids".to_string(), None),
    ] {
        let reply = request(port, method, &target, body);
        assert_eq!(reply.status, 404, "{method} {target}: {}", reply.body);
        assert!(
            reply.json()["error"].is_string(),
            "{method} {target}: {}",
            reply.body
        );
    }

    let bid = format!("/api/v0/tasks/{id}/bid");
    let award = format!("/api/v0/tasks/{id}/award");
    for (method, target, body) in [
        ("GET", bid.as_str(), None),
        ("PUT", bid.as_str(), Some(r#"{"agent_cid":100,"price":5}"#)),
        ("DELETE", bid.as_str(), None),
        ("GET", award.as_str(), None),
        ("PUT", award.as_str(), Some(r#"{"agent_cid":100}"#)),
        ("DELETE", award.as_str(), None),
        ("POST", "/api/v0/agora", Some("{}")),
        ("PUT", "/api/v0/agora", Some("{}")),
        ("DELETE", "/api/v0/agora", None),
    ] {
        let reply = request(port, method, target, body);
        assert_eq!(reply.status, 405, "{method} {target}: {}", reply.body);
        assert!(
            reply.json()["error"].is_string(),
            "{method} {target}: {}",
            reply.body
        );
    }
}

#[test]
fn the_call_board_is_bounded_to_the_newest_window() {
    let server = common::start(console_dir("bounded"));
    let port = server.port();

    // One call more than the endpoint carries: the response is cut to the
    // limit, holds the *newest* window of calls, oldest first within it, and
    // says so — exactly the bounding the journal endpoint gives its entries.
    let calls = noded::agora::AGORA_LIMIT + 1;
    for index in 0..calls {
        task(
            port,
            &format!(r#"{{"goal":"call {index}","budget":10,"role":"builder"}}"#),
        );
    }
    let view = board(port);
    assert_eq!(view["total"], calls);
    assert_eq!(view["limit"], noded::agora::AGORA_LIMIT);
    assert_eq!(view["truncated"], true);
    let listed = view["calls"].as_array().expect("an array");
    assert_eq!(listed.len(), noded::agora::AGORA_LIMIT);
    assert_eq!(listed[0]["task"], "t2", "the oldest call the window left out");
    assert_eq!(
        listed[listed.len() - 1]["task"],
        format!("t{calls}"),
        "the newest call is last"
    );
    // Oldest first, and every call is a call.
    let ids: Vec<&str> = listed
        .iter()
        .map(|call| call["task"].as_str().expect("a task id"))
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_by_key(|id| id.trim_start_matches('t').parse::<u64>().expect("a sequence"));
    assert_eq!(ids, sorted, "oldest first");
    assert!(listed.iter().all(|call| call["state"] == "open"));
}
