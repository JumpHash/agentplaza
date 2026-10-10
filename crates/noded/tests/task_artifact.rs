//! `POST /api/v0/tasks/{id}/artifact`: the console's read-through to a task's
//! artifact, exercised over real sockets.
//!
//! A stub admin listener - a `TcpListener` in this test serving a fixed raw
//! answer and handing back the request it read - stands in for the hub's admin
//! surface, so every status the hub can answer with, the JSON the daemon sends
//! it, and the parse of the agent's own `read_file` output are asserted end to
//! end without the live `vsockd` (which this suite must never touch). The
//! daemon itself is the real `noded` on an ephemeral port, with a console
//! directory that is empty and runs that boot from a missing kernel; a task is
//! claimed through the daemon's own ledger, so no VM is booted.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use common::{request, send, start_with_hubs};
use noded::hub::HubAdmin;
use noded::registry::ConnectedAgent;
use noded::tasks::CreateTask;
use noded::Server;

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-artifact-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a console directory");
    dir
}

/// The hub's admin surface for a stub on `port`.
fn admin_at(port: u16) -> Option<HubAdmin> {
    Some(HubAdmin::parse(&format!("127.0.0.1:{port}")).expect("a loopback surface"))
}

/// A loopback port nothing is listening on: bound to learn a free one, then
/// closed, so a connect is refused.
fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("a bound address").port();
    drop(listener);
    port
}

/// A complete stub answer with `status`, `body` and a matching length.
fn answer(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A stub admin surface: binds an ephemeral loopback port, reads the whole
/// request (head and body), answers it with `raw`, and hands back the request
/// it read, so the daemon's own `/tell` can be asserted.
fn stub_admin(raw: Vec<u8>) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the stub admin binds");
    let port = listener.local_addr().expect("a bound address").port();
    let handle = thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().expect("the daemon connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("a read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("the stub clones its socket"));
        let mut head = String::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("a header line") == 0 {
                break;
            }
            if line == "\r\n" || line == "\n" {
                head.push_str(&line);
                break;
            }
            if let Some((name, value)) = line.trim_end().split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
            head.push_str(&line);
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader
                .read_exact(&mut body)
                .expect("the daemon's request body");
        }
        stream.write_all(&raw).expect("the stub answers");
        stream.flush().expect("the answer is flushed");
        format!("{head}{}", String::from_utf8_lossy(&body))
    });
    (port, handle)
}

/// An admin surface that accepts the connection and then says nothing: the
/// daemon must not wait on it forever.
fn silent_admin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the stub admin binds");
    let port = listener.local_addr().expect("a bound address").port();
    thread::spawn(move || {
        let (stream, _peer) = listener.accept().expect("the daemon connects");
        // Hold the connection open, silent, past the daemon's own timeout.
        thread::sleep(Duration::from_secs(15));
        drop(stream);
    });
    port
}

/// A `read_file` tool output, exactly as the harness writes it
/// (`{"path","size","bytes","truncated","content"}`), with the size the file
/// actually has.
fn read_output(path: &str, content: &str) -> String {
    serde_json::json!({
        "path": path,
        "size": content.len(),
        "bytes": content.len(),
        "truncated": false,
        "content": content,
    })
    .to_string()
}

/// The `/tell` answer body the hub writes: the agent's own `ok` and `output`.
fn tell_body(ok: bool, output: &str) -> String {
    serde_json::json!({ "ok": ok, "output": output }).to_string()
}

/// A task in `open`, created through the daemon's own ledger.
fn open_task(server: &Server, role: &str) -> String {
    server
        .node()
        .tasks()
        .create(&CreateTask {
            goal: "artifact read-through".to_string(),
            acceptance: None,
            budget: None,
            role: Some(role.to_string()),
        })
        .expect("a task inside the contract")
        .id()
        .to_string()
}

/// A task claimed by an agent connected as `cid`: the ledger's own claim, which
/// is what the endpoint reads the agent's CID from, so no VM is booted.
fn claimed_task(server: &Server, cid: u64, role: &str) -> String {
    let task = server
        .node()
        .tasks()
        .create(&CreateTask {
            goal: "artifact read-through".to_string(),
            acceptance: None,
            budget: None,
            role: Some(role.to_string()),
        })
        .expect("a task inside the contract");
    task.claim(&ConnectedAgent {
        cid,
        role: Some(role.to_string()),
        run: "r1".to_string(),
        vm: 0,
    })
    .expect("an open task claims");
    task.id().to_string()
}

/// The artifact endpoint's URL for one task.
fn artifact_path(id: &str) -> String {
    format!("/api/v0/tasks/{id}/artifact")
}

#[test]
fn the_artifact_is_read_through_the_hub() {
    let raw = answer(
        "200 OK",
        &tell_body(true, &read_output("notes.txt", "hello\n")),
    );
    let (port, hub) = stub_admin(raw);
    let server = start_with_hubs(console_dir("read"), None, admin_at(port));
    let id = claimed_task(&server, 100, "builder");

    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"notes.txt"}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    let json = reply.json();
    assert_eq!(json["task"], id);
    assert_eq!(json["agent_cid"], 100);
    assert_eq!(json["path"], "notes.txt");
    assert_eq!(json["content"], "hello\n");
    assert_eq!(json["size"], 6);
    assert_eq!(json["truncated"], false);

    // The daemon asked the hub to drive exactly `read_file` on the task's
    // agent, with this daemon's own timeout.
    let sent = hub.join().expect("the stub admin finished");
    let (head, body) = sent.split_once("\r\n\r\n").expect("a head and a body");
    assert!(
        head.starts_with("POST /tell HTTP/1.1\r\n"),
        "the daemon posted /tell: {head:?}"
    );
    let sent_body: serde_json::Value =
        serde_json::from_str(body).expect("the /tell body is JSON");
    assert_eq!(sent_body["cid"], 100);
    assert_eq!(sent_body["name"], "read_file");
    assert_eq!(sent_body["args"]["path"], "notes.txt");
    assert_eq!(sent_body["timeout_ms"], 5000);

    // ... and the task routes still answer on the same daemon.
    let task = request(server.port(), "GET", &format!("/api/v0/tasks/{id}"), None);
    assert_eq!(task.status, 200, "{}", task.body);
    assert_eq!(task.json()["state"], "claimed");
}

#[test]
fn a_truncated_read_is_reported() {
    let output = serde_json::json!({
        "path": "big.txt",
        "size": 1_048_576u64,
        "bytes": 4,
        "truncated": true,
        "content": "abcd",
    })
    .to_string();
    let (port, hub) = stub_admin(answer("200 OK", &tell_body(true, &output)));
    let server = start_with_hubs(console_dir("truncated"), None, admin_at(port));
    let id = claimed_task(&server, 100, "builder");

    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"big.txt"}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    let json = reply.json();
    assert_eq!(json["content"], "abcd");
    assert_eq!(json["size"], 1_048_576);
    assert_eq!(json["truncated"], true);
    hub.join().expect("the stub admin finished");
}

#[test]
fn an_unknown_task_is_a_404() {
    // Nothing is listening: an unknown task must be refused before the hub is
    // ever contacted, so the 404 cannot be mistaken for the hub's own 404.
    let server = start_with_hubs(console_dir("unknown"), None, admin_at(dead_port()));
    let reply = request(
        server.port(),
        "POST",
        &artifact_path("t99"),
        Some(r#"{"path":"notes.txt"}"#),
    );
    assert_eq!(reply.status, 404, "{}", reply.body);
    assert_eq!(reply.json()["error"], "no task with id t99");
}

#[test]
fn an_unclaimed_task_is_a_409() {
    let server = start_with_hubs(console_dir("unclaimed"), None, admin_at(dead_port()));
    let id = open_task(&server, "builder");
    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"notes.txt"}"#),
    );
    assert_eq!(reply.status, 409, "{}", reply.body);
    assert_eq!(reply.json()["error"], format!("task {id} has no assignee yet"));
}

#[test]
fn an_unconfigured_admin_is_a_503() {
    // `--hub-admin off`: the daemon was never told where the admin surface is,
    // so it says so - before the task is resolved, and without an empty answer.
    let server = start_with_hubs(console_dir("off"), None, None);
    let id = claimed_task(&server, 100, "builder");
    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"notes.txt"}"#),
    );
    assert_eq!(reply.status, 503, "{}", reply.body);
    assert_eq!(reply.json()["error"], "hub admin is not configured");

    // Even an unknown task: the endpoint cannot do anything at all.
    let reply = request(
        server.port(),
        "POST",
        &artifact_path("t99"),
        Some(r#"{"path":"notes.txt"}"#),
    );
    assert_eq!(reply.status, 503, "{}", reply.body);
}

#[test]
fn a_path_outside_the_contract_is_a_400() {
    // The hub is a dead port: a 400 proves the request was refused before any
    // connection, not after one failed.
    let server = start_with_hubs(console_dir("badpath"), None, admin_at(dead_port()));
    let id = claimed_task(&server, 100, "builder");
    let long_path = "a".repeat(1025);
    let cases = [
        ("{}".to_string(), "invalid artifact request"),
        (r#"{"path":""}"#.to_string(), "path must not be empty"),
        (
            format!(r#"{{"path":"{long_path}"}}"#),
            "path must be at most 1024 bytes",
        ),
        (
            r#"{"path":"a\u0000b"}"#.to_string(),
            "path must not contain a NUL byte",
        ),
    ];
    for (body, expected) in cases {
        let reply = request(server.port(), "POST", &artifact_path(&id), Some(&body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        let error = reply.json()["error"]
            .as_str()
            .expect("an error string")
            .to_string();
        assert!(error.contains(expected), "{body}: {error}");
    }

    // The boundary is inclusive: 1024 bytes is inside the contract (it reaches
    // the hub, which is a dead port here -> a 502, not a 400).
    let at_limit = "a".repeat(1024);
    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(&format!(r#"{{"path":"{at_limit}"}}"#)),
    );
    assert_eq!(reply.status, 502, "{}", reply.body);
}

#[test]
fn the_hubs_refusal_is_surfaced_verbatim() {
    // Every refusal the hub's admin surface can answer: a role the policy
    // forbids the tool (403), a CID the daemon does not serve (404), a served
    // but not-connected CID (409), and a timeout (504). The daemon passes the
    // hub's own status and its own words through, unchanged.
    let cases = [
        (403u16, "role librarian does not grant read_file"),
        (404, "no agent with guest CID 100"),
        (409, "agent with guest CID 100 is not connected"),
        (504, "agent did not answer within 5000 ms"),
    ];
    for (status, message) in cases {
        let raw = answer(
            &format!("{status} Refused"),
            &serde_json::json!({ "error": message }).to_string(),
        );
        let (port, hub) = stub_admin(raw);
        let server = start_with_hubs(console_dir(&format!("refused-{status}")), None, admin_at(port));
        let id = claimed_task(&server, 100, "librarian");

        let reply = request(
            server.port(),
            "POST",
            &artifact_path(&id),
            Some(r#"{"path":"index.txt"}"#),
        );
        assert_eq!(reply.status, status, "{}", reply.body);
        assert_eq!(reply.json()["error"], message, "status {status}");
        hub.join().expect("the stub admin finished");
    }
}

#[test]
fn a_tool_refusal_is_a_422_with_the_agents_own_words() {
    let refusal = "denied: ../escape is outside the workspace";
    let (port, hub) = stub_admin(answer("200 OK", &tell_body(false, refusal)));
    let server = start_with_hubs(console_dir("toolrefused"), None, admin_at(port));
    let id = claimed_task(&server, 100, "builder");

    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"../escape"}"#),
    );
    assert_eq!(reply.status, 422, "{}", reply.body);
    assert_eq!(reply.json()["error"], refusal);
    hub.join().expect("the stub admin finished");
}

#[test]
fn a_malformed_tool_output_is_a_502_naming_it() {
    for output in [
        // Not JSON at all.
        "not json".to_string(),
        // JSON, but not the tool's object: no content, size or truncation.
        serde_json::json!({ "path": "notes.txt" }).to_string(),
        // A field of the wrong type.
        serde_json::json!({ "size": 6, "truncated": false, "content": 7 }).to_string(),
    ] {
        let (port, hub) = stub_admin(answer("200 OK", &tell_body(true, &output)));
        let server = start_with_hubs(console_dir("malformed"), None, admin_at(port));
        let id = claimed_task(&server, 100, "builder");

        let reply = request(
            server.port(),
            "POST",
            &artifact_path(&id),
            Some(r#"{"path":"notes.txt"}"#),
        );
        assert_eq!(reply.status, 502, "{output}: {}", reply.body);
        let error = reply.json()["error"]
            .as_str()
            .expect("an error string")
            .to_string();
        assert!(
            error.contains("the agent's read_file answer is not the tool's JSON"),
            "{output}: {error}"
        );
        hub.join().expect("the stub admin finished");
    }

    // A 200 that is not the admin surface's envelope at all is unreadable too.
    let (port, hub) = stub_admin(answer("200 OK", "not the hub's answer"));
    let server = start_with_hubs(console_dir("bad-envelope"), None, admin_at(port));
    let id = claimed_task(&server, 100, "builder");
    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"notes.txt"}"#),
    );
    assert_eq!(reply.status, 502, "{}", reply.body);
    let error = reply.json()["error"]
        .as_str()
        .expect("an error string")
        .to_string();
    assert!(
        error.contains("not the admin surface's JSON"),
        "{error}"
    );
    hub.join().expect("the stub admin finished");
}

#[test]
fn an_unreachable_admin_is_a_502_naming_the_reason() {
    let server = start_with_hubs(console_dir("unreachable"), None, admin_at(dead_port()));
    let id = claimed_task(&server, 100, "builder");

    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"notes.txt"}"#),
    );
    assert_eq!(reply.status, 502, "{}", reply.body);
    let error = reply.json()["error"]
        .as_str()
        .expect("an error string")
        .to_string();
    assert!(error.starts_with("hub admin unreachable: "), "{error}");
    assert!(error.contains("cannot connect"), "{error}");
}

#[test]
fn a_silent_admin_is_a_502_not_a_hang() {
    let server = start_with_hubs(console_dir("silent"), None, admin_at(silent_admin()));
    let id = claimed_task(&server, 100, "builder");

    let started = Instant::now();
    let reply = request(
        server.port(),
        "POST",
        &artifact_path(&id),
        Some(r#"{"path":"notes.txt"}"#),
    );
    let waited = started.elapsed();
    assert_eq!(reply.status, 502, "{}", reply.body);
    let error = reply.json()["error"]
        .as_str()
        .expect("an error string")
        .to_string();
    // The admin budget is 10s: it must have named its own bound, and it must
    // have given up on it rather than waiting on the silent hub forever.
    assert!(error.contains("did not answer within 10s"), "{error}");
    assert!(waited < Duration::from_secs(14), "waited {waited:?}");
}

#[test]
fn the_artifact_endpoint_is_post_only() {
    let server = start_with_hubs(console_dir("postonly"), None, None);
    let id = claimed_task(&server, 100, "builder");
    for method in ["GET", "PUT", "DELETE"] {
        let reply = request(
            server.port(),
            method,
            &artifact_path(&id),
            Some(r#"{"path":"notes.txt"}"#),
        );
        assert_eq!(reply.status, 405, "{method}: {}", reply.body);
        assert_eq!(reply.json()["error"], "this endpoint accepts POST", "{method}");
    }
}

#[test]
fn the_loopback_host_guard_applies_to_the_artifact_route() {
    // A page served from any hostname that resolves to 127.0.0.1 can post a
    // simple request here; a Host that names anything but loopback is refused.
    let server = start_with_hubs(console_dir("host"), None, None);
    let id = claimed_task(&server, 100, "builder");
    let body = br#"{"path":"notes.txt"}"#;
    let raw = format!(
        "POST {} HTTP/1.1\r\nHost: evil.example\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        artifact_path(&id),
        body.len()
    );
    let mut request_bytes = raw.into_bytes();
    request_bytes.extend_from_slice(body);
    let reply = send(server.port(), &request_bytes);
    assert_eq!(reply.status, 403, "{}", reply.body);
    assert_eq!(reply.json()["error"], "the Host header must name loopback");
}

#[test]
fn the_artifact_route_leaves_the_task_routes_untouched() {
    // With a live admin surface and an off one alike, the task routes answer
    // exactly as they did before this endpoint existed.
    for (name, admin) in [("live", admin_at(dead_port())), ("off", None)] {
        let server = start_with_hubs(console_dir(&format!("kept-{name}")), None, admin);
        let port = server.port();

        let created = request(
            port,
            "POST",
            "/api/v0/tasks",
            Some(r#"{"goal":"write an index","role":"builder"}"#),
        );
        assert_eq!(created.status, 201, "{name}: {}", created.body);
        let id = created.json()["id"].as_str().expect("a task id").to_string();
        assert_eq!(created.json()["state"], "open", "{name}");

        let list = request(port, "GET", "/api/v0/tasks", None);
        assert_eq!(list.status, 200, "{name}: {}", list.body);
        assert_eq!(list.json()["tasks"].as_array().expect("an array").len(), 1, "{name}");

        let one = request(port, "GET", &format!("/api/v0/tasks/{id}"), None);
        assert_eq!(one.status, 200, "{name}: {}", one.body);
        assert_eq!(one.json()["goal"], "write an index", "{name}");

        let withdrawn = request(port, "DELETE", &format!("/api/v0/tasks/{id}"), None);
        assert_eq!(withdrawn.status, 200, "{name}: {}", withdrawn.body);
        assert_eq!(withdrawn.json()["state"], "withdrawn", "{name}");

        let machine = request(port, "GET", "/api/v0/machine", None);
        assert_eq!(machine.status, 200, "{name}: {}", machine.body);
    }
}
