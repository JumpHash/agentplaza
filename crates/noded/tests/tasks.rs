//! The task ledger's HTTP API, exercised over a real socket on an ephemeral
//! port: create, list, get, claim, finish and withdraw.
//!
//! The daemon here boots no VMs, so no agent is connected: the one claim that
//! can be exercised over this surface is the refusal (a claim from a CID the
//! register does not show - which is every CID). The accepted claim, from a
//! real connected agent's CID, is covered by the gated live test in
//! `tests/smoke.rs`, and the claim rules themselves by the ledger's unit tests.

mod common;

use std::fs;
use std::io::BufRead as _;
use std::io::Write as _;
use std::path::PathBuf;

use common::request;

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-tasks-{}-{name}", std::process::id()));
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

#[test]
fn the_task_list_starts_empty() {
    let server = common::start(console_dir("list"));
    let reply = request(server.port(), "GET", "/api/v0/tasks", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(reply.content_type.starts_with("application/json"));
    assert_eq!(reply.json(), serde_json::json!({ "tasks": [] }));
}

#[test]
fn a_created_task_is_open_with_the_schemas_shape() {
    let server = common::start(console_dir("create"));
    let port = server.port();
    let reply = request(
        port,
        "POST",
        "/api/v0/tasks",
        Some(r#"{"goal":"ship the ledger","acceptance":"the suite is green","budget":5,"role":"builder"}"#),
    );
    assert_eq!(reply.status, 201, "{}", reply.body);
    let created = reply.json();
    assert_eq!(created["id"], "t1");
    assert_eq!(created["goal"], "ship the ledger");
    assert_eq!(created["acceptance"], "the suite is green");
    assert_eq!(created["budget"], 5);
    assert_eq!(created["role"], "builder");
    assert_eq!(created["state"], "open");
    // Nothing has happened to it yet: no outcome, no assignee, no verifier.
    assert!(created["outcome"].is_null(), "{created}");
    assert!(created["assignee_id"].is_null(), "{created}");
    assert!(created["verifier_id"].is_null(), "{created}");
    assert!(created["agent_cid"].is_null(), "{created}");
    assert!(created["note"].is_null(), "{created}");
    let created_at = created["created_at"].as_u64().expect("created_at");
    assert!(created_at > 0);
    assert_eq!(created["updated_at"].as_u64(), Some(created_at));

    // The single-task GET is what the create answered, and the list has it.
    let single = request(port, "GET", "/api/v0/tasks/t1", None);
    assert_eq!(single.status, 200, "{}", single.body);
    assert_eq!(single.json(), created);
    let list = request(port, "GET", "/api/v0/tasks", None);
    assert_eq!(list.json()["tasks"][0], created);
    assert_eq!(list.json()["tasks"].as_array().expect("an array").len(), 1);

    // The optional fields have the schema's defaults, and the ids follow
    // creation order with the list newest last.
    let bare = request(port, "POST", "/api/v0/tasks", Some(r#"{"goal":"just a goal"}"#));
    assert_eq!(bare.status, 201, "{}", bare.body);
    let bare = bare.json();
    assert_eq!(bare["id"], "t2");
    assert_eq!(bare["budget"], 0);
    assert!(bare["acceptance"].is_null(), "{bare}");
    assert!(bare["role"].is_null(), "{bare}");
    let listed = request(port, "GET", "/api/v0/tasks", None).json();
    let ids: Vec<&str> = listed["tasks"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|task| task["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, vec!["t1", "t2"], "oldest first");
}

#[test]
fn task_requests_outside_the_contract_are_400() {
    let server = common::start(console_dir("create-400"));
    let port = server.port();
    for body in [
        // A task without a goal is not a task.
        r#"{}"#,
        r#"{"goal":""}"#,
        r#"{"goal":"   "}"#,
        r#"{"goal":123}"#,
        r#"{"goal":null}"#,
        // The budget is the schema's bigint, and must be a number.
        r#"{"goal":"g","budget":-1}"#,
        r#"{"goal":"g","budget":"lots"}"#,
        r#"{"goal":"g","budget":9223372036854775808}"#,
        // An optional field, when present, is a value.
        r#"{"goal":"g","acceptance":""}"#,
        r#"{"goal":"g","role":""}"#,
        r#"{"goal":"g","acceptance":12}"#,
        r#"not json at all"#,
        r#"[1,2,3]"#,
        "",
    ] {
        let reply = request(port, "POST", "/api/v0/tasks", Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }
    // A refused create leaves no task behind, and hands out no id: the first
    // task created afterwards is t1.
    assert_eq!(
        request(port, "GET", "/api/v0/tasks", None).json(),
        serde_json::json!({ "tasks": [] })
    );
    assert_eq!(task(port, r#"{"goal":"the first real task"}"#), "t1");

    // A field over its bound is refused too, at the same edge: exactly the
    // bound is accepted, one byte more is not.
    let at_bound = "g".repeat(noded::tasks::MAX_TEXT);
    let over_bound = "g".repeat(noded::tasks::MAX_TEXT + 1);
    let over_role = "r".repeat(noded::tasks::MAX_ROLE + 1);
    let at_role = "r".repeat(noded::tasks::MAX_ROLE);
    for (body, expect) in [
        (
            format!(r#"{{"goal":"{over_bound}"}}"#),
            400,
        ),
        (
            format!(r#"{{"goal":"g","acceptance":"{over_bound}"}}"#),
            400,
        ),
        (format!(r#"{{"goal":"g","role":"{over_role}"}}"#), 400),
        (format!(r#"{{"goal":"{at_bound}"}}"#), 201),
        (
            format!(r#"{{"goal":"g","acceptance":"{at_bound}","role":"{at_role}"}}"#),
            201,
        ),
    ] {
        let reply = request(port, "POST", "/api/v0/tasks", Some(&body));
        assert_eq!(
            reply.status,
            expect,
            "a {}-byte field: {}",
            body.len(),
            reply.body
        );
    }

    // And the finish note's bound is checked before the transition, so an
    // oversized note is the request's fault (400), not the task's state.
    let id = task(port, r#"{"goal":"g"}"#);
    let note = "n".repeat(noded::tasks::MAX_TEXT + 1);
    let reply = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{id}/finish"),
        Some(&format!(r#"{{"outcome":"done","note":"{note}"}}"#)),
    );
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(
        reply.json()["error"]
            .as_str()
            .expect("an error message")
            .contains("note must be at most"),
        "{}",
        reply.body
    );
}

#[test]
fn task_sub_requests_outside_the_contract_are_400() {
    let server = common::start(console_dir("sub-400"));
    let port = server.port();
    let id = task(port, r#"{"goal":"g"}"#);

    for body in [r#"{}"#, r#"{"agent_cid":"x"}"#, r#"{"agent_cid":-1}"#, "[]"] {
        let reply = request(port, "POST", &format!("/api/v0/tasks/{id}/claim"), Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }
    for body in [r#"{}"#, r#"{"outcome":"maybe"}"#, r#"{"outcome":null}"#, "[]"] {
        let reply = request(port, "POST", &format!("/api/v0/tasks/{id}/finish"), Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }
    // None of those refusals moved the task.
    assert_eq!(
        request(port, "GET", &format!("/api/v0/tasks/{id}"), None).json()["state"],
        "open"
    );
}

#[test]
fn unknown_tasks_are_404() {
    let server = common::start(console_dir("unknown"));
    let port = server.port();
    for (method, target, body) in [
        ("GET", "/api/v0/tasks/t9".to_string(), None),
        ("DELETE", "/api/v0/tasks/t9".to_string(), None),
        (
            "POST",
            "/api/v0/tasks/t9/claim".to_string(),
            Some(r#"{"agent_cid":100}"#),
        ),
        (
            "POST",
            "/api/v0/tasks/t9/finish".to_string(),
            Some(r#"{"outcome":"done"}"#),
        ),
        // A path under /api/v0/tasks/ that names no endpoint at all.
        ("GET", "/api/v0/tasks/".to_string(), None),
        ("GET", "/api/v0/tasks/t1/nope".to_string(), None),
    ] {
        let reply = request(port, method, &target, body);
        assert_eq!(reply.status, 404, "{method} {target}: {}", reply.body);
        assert!(
            reply.json()["error"].is_string(),
            "{method} {target}: {}",
            reply.body
        );
    }
}

#[test]
fn an_unsupported_method_on_a_task_path_is_405() {
    let server = common::start(console_dir("405"));
    let port = server.port();
    let id = task(port, r#"{"goal":"g"}"#);
    let claim = format!("/api/v0/tasks/{id}/claim");
    let finish = format!("/api/v0/tasks/{id}/finish");
    for (method, target, body) in [
        ("PUT", "/api/v0/tasks".to_string(), Some(r#"{"goal":"g"}"#)),
        ("POST", "/api/v0/tasks/t1".to_string(), None),
        ("PUT", "/api/v0/tasks/t1".to_string(), None),
        ("GET", claim.clone(), None),
        ("GET", finish.clone(), None),
        ("DELETE", claim.clone(), None),
    ] {
        let reply = request(port, method, &target, body);
        assert_eq!(reply.status, 405, "{method} {target}: {}", reply.body);
        assert!(
            reply.json()["error"].is_string(),
            "{method} {target}: {}",
            reply.body
        );
    }
}

#[test]
fn a_claim_needs_a_connected_agent() {
    let server = common::start(console_dir("claim"));
    let port = server.port();
    let id = task(port, r#"{"goal":"g"}"#);

    // This daemon has booted no VMs, so no CID is connected - not one it
    // never had, and not one a failed run was given.
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
            &format!("/api/v0/tasks/{id}/claim"),
            Some(&format!(r#"{{"agent_cid":{cid}}}"#)),
        );
        assert_eq!(reply.status, 409, "cid {cid}: {}", reply.body);
        let error = reply.json()["error"]
            .as_str()
            .expect("an error message")
            .to_string();
        assert!(error.contains(&cid.to_string()), "cid {cid}: {error}");
        // The refusal left the task open and unassigned.
        let task = request(port, "GET", &format!("/api/v0/tasks/{id}"), None).json();
        assert_eq!(task["state"], "open", "{task}");
        assert!(task["assignee_id"].is_null(), "{task}");
    }
}

#[test]
fn finish_and_withdraw_only_move_a_task_that_can_move() {
    let server = common::start(console_dir("transitions"));
    let port = server.port();

    // An open task cannot be finished: nobody claimed it to do the work.
    let open = task(port, r#"{"goal":"never claimed"}"#);
    let refused = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{open}/finish"),
        Some(r#"{"outcome":"done"}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        "a task may not go from open to done"
    );
    assert_eq!(
        request(port, "GET", &format!("/api/v0/tasks/{open}"), None).json()["state"],
        "open"
    );

    // It can be withdrawn, once: a withdrawn task is final.
    let withdrawn = request(port, "DELETE", &format!("/api/v0/tasks/{open}"), None);
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    assert_eq!(withdrawn.json()["state"], "withdrawn");
    assert_eq!(withdrawn.json()["outcome"], "cancelled");
    let refused = request(port, "DELETE", &format!("/api/v0/tasks/{open}"), None);
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        "a task may not go from withdrawn to withdrawn"
    );
    let refused = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{open}/finish"),
        Some(r#"{"outcome":"failed"}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        "a task may not go from withdrawn to failed"
    );

    // A terminal task stays readable in the list.
    let list = request(port, "GET", "/api/v0/tasks", None).json();
    assert_eq!(list["tasks"][0]["state"], "withdrawn");
}

#[test]
fn an_events_stream_carries_a_tasks_life() {
    let server = common::start(console_dir("events"));
    let port = server.port();

    // Subscribe before the task exists: the events a stream carries start at
    // the subscription, and the head write is the subscription's confirmation.
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .expect("the events stream connects");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("the stream gets a read timeout");
    stream
        .write_all(b"GET /api/v0/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("the events request is sent");
    let mut stream = std::io::BufReader::new(stream);
    let mut head = String::new();
    stream
        .read_line(&mut head)
        .expect("the stream head is read");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    for _ in 0..3 {
        let mut header = String::new();
        stream.read_line(&mut header).expect("a header line");
    }
    let mut blank = String::new();
    stream.read_line(&mut blank).expect("the head ends");
    assert_eq!(blank, "\r\n");

    let id = task(port, r#"{"goal":"a task over the stream"}"#);
    let withdrawn = request(port, "DELETE", &format!("/api/v0/tasks/{id}"), None);
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);

    let mut sock = stream.into_inner();
    let mut raw = Vec::new();
    for _ in 0..64 {
        let mut chunk = [0u8; 4096];
        match std::io::Read::read(&mut sock, &mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(err) => panic!("the stream failed: {err}"),
        }
        if String::from_utf8_lossy(&raw).contains("\"state\":\"withdrawn\"") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert_eq!(
        text,
        format!(
            "event: task\ndata: {{\"id\":\"{id}\",\"state\":\"open\",\"assignee\":null}}\n\n\
             event: task\ndata: {{\"id\":\"{id}\",\"state\":\"withdrawn\",\"assignee\":null}}\n\n"
        ),
        "the stream carries the task's whole short life"
    );

    // The stream does not end with the events: the API still answers.
    assert_eq!(
        request(port, "GET", &format!("/api/v0/tasks/{id}"), None).json()["state"],
        "withdrawn"
    );
}
