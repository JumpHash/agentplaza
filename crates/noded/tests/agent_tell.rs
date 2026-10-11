//! `POST /v1/agents/{cid}/tell`: the operator console's drive of one tool
//! call on a connected agent, exercised over real sockets.
//!
//! A stub admin listener - a `TcpListener` in this test serving a fixed raw
//! answer and handing back the request it read - stands in for the hub's admin
//! surface, so every status the hub can answer with, the JSON the daemon sends
//! it, and the passthrough of the agent's own answer are asserted end to end
//! without the live `vsockd` (which this suite must never touch). The daemon
//! itself is the real `noded` on an ephemeral port; no VM is booted and no task
//! is touched - a `/tell` is addressed by guest CID alone.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use common::{request, start_with};
use noded::hub::Hub;

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-tell-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a console directory");
    dir
}

/// The hub for a stub on `port`.
fn admin_at(port: u16) -> Option<Hub> {
    Some(Hub::new(port))
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

/// The `/tell` answer body the hub writes: the agent's own `ok` and `output`.
fn tell_body(ok: bool, output: &str) -> String {
    serde_json::json!({ "ok": ok, "output": output }).to_string()
}

/// The tell endpoint's URL for one agent.
fn tell_path(cid: &str) -> String {
    format!("/v1/agents/{cid}/tell")
}

#[test]
fn the_tell_is_driven_through_the_hub() {
    let (port, hub) = stub_admin(answer(
        "200 OK",
        &tell_body(true, r#"{"matches":[],"files_scanned":1}"#),
    ));
    let server = start_with(console_dir("drive"), admin_at(port));

    let reply = request(
        server.port(),
        "POST",
        &tell_path("300"),
        Some(r#"{"name":"list","args":{"path":"."},"timeout_ms":2000}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    let json = reply.json();
    assert_eq!(json["cid"], 300);
    assert_eq!(json["ok"], true);
    assert_eq!(json["output"], r#"{"matches":[],"files_scanned":1}"#);

    // The daemon asked the hub to drive exactly this tool, on this CID, with
    // this daemon's own timeout.
    let sent = hub.join().expect("the stub admin finished");
    let (head, body) = sent.split_once("\r\n\r\n").expect("a head and a body");
    assert!(
        head.starts_with("POST /v1/agents/300/tell HTTP/1.1\r\n"),
        "the daemon posted the hub's tell: {head:?}"
    );
    let sent_body: serde_json::Value = serde_json::from_str(body).expect("the /tell body is JSON");
    assert_eq!(sent_body["name"], "list");
    assert_eq!(sent_body["args"]["path"], ".");
    assert_eq!(sent_body["timeout_ms"], 2000);

    // ... and the other routes still answer on the same daemon.
    let machine = request(server.port(), "GET", "/v1/machine", None);
    assert_eq!(machine.status, 200, "{}", machine.body);
}

#[test]
fn an_omitted_args_and_timeout_take_the_hubs_defaults() {
    let (port, hub) = stub_admin(answer("200 OK", &tell_body(true, "up 3 days")));
    let server = start_with(console_dir("defaults"), admin_at(port));

    let reply = request(
        server.port(),
        "POST",
        &tell_path("301"),
        Some(r#"{"name":"uptime"}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.json()["output"], "up 3 days");

    let sent = hub.join().expect("the stub admin finished");
    let body = sent.split_once("\r\n\r\n").expect("a head and a body").1;
    let sent_body: serde_json::Value = serde_json::from_str(body).expect("the /tell body is JSON");
    // An omitted `args` is the empty object the hub's own default fills in, and
    // an omitted `timeout_ms` is the hub's own maximum.
    assert_eq!(sent_body["args"], serde_json::json!({}));
    assert_eq!(sent_body["timeout_ms"], 5000);
}

#[test]
fn the_agents_own_refusal_is_an_answer() {
    // The tool ran and refused: the hub answers 200 with the agent's own
    // `ok: false` and its reason. That is the agent's answer - data, not a
    // transport failure - and the daemon passes it through unchanged.
    let refusal = "denied: read_file is not granted to this role";
    let (port, hub) = stub_admin(answer("200 OK", &tell_body(false, refusal)));
    let server = start_with(console_dir("agentno"), admin_at(port));

    let reply = request(
        server.port(),
        "POST",
        &tell_path("300"),
        Some(r#"{"name":"read_file","args":{"path":"x.txt"}}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    let json = reply.json();
    assert_eq!(json["cid"], 300);
    assert_eq!(json["ok"], false);
    assert_eq!(json["output"], refusal);
    hub.join().expect("the stub admin finished");
}

#[test]
fn the_hubs_refusals_are_surfaced_verbatim() {
    // Every refusal the hub can answer: a role the policy
    // forbids the tool (403), a CID the daemon does not serve (404), a served
    // but not-connected CID (409), and a timeout (504). The daemon passes the
    // hub's own status and its own words through, unchanged.
    //
    // The 403's body is the hub's own, `{"ok":false,"output":"<reason>"}`: this
    // daemon surfaces that body as the error - the hub's words, not an invented
    // message - which is what the operator's console shows.
    let refusal = r#"{"ok":false,"output":"role librarian does not grant write_file"}"#;
    let cases = [
        (403u16, refusal.to_string()),
        (
            404,
            r#"{"error":"no agent with guest CID 300"}"#.to_string(),
        ),
        (
            409,
            r#"{"error":"agent with guest CID 300 is not connected"}"#.to_string(),
        ),
        (
            504,
            r#"{"error":"agent did not answer within 5000 ms"}"#.to_string(),
        ),
    ];
    for (status, body) in cases {
        let (port, hub) = stub_admin(answer(&format!("{status} Refused"), &body));
        let server = start_with(console_dir(&format!("refused-{status}")), admin_at(port),
        );
        let reply = request(
            server.port(),
            "POST",
            &tell_path("300"),
            Some(r#"{"name":"write_file","args":{"path":"a.txt","content":"x"}}"#),
        );
        assert_eq!(reply.status, status, "{}", reply.body);
        // The message the daemon carries: its `error` key when the hub's body
        // has one, else the hub's whole body - either way the hub's own words.
        let expected = if status == 403 {
            refusal
        } else {
            body.as_str()
                .trim_start_matches(r#"{"error":""#)
                .trim_end_matches(r#""}"#)
        };
        assert_eq!(reply.json()["error"], expected, "status {status}");
        hub.join().expect("the stub admin finished");
    }
}

#[test]
fn an_out_of_contract_request_is_a_400() {
    // The hub is a dead port: a 400 proves the request was refused before any
    // connection, not after one failed.
    let server = start_with(console_dir("badreq"), admin_at(dead_port()));
    let long_name = "a".repeat(257);
    let long_body = format!(r#"{{"name":"{long_name}"}}"#);

    let cases = [
        // The CID path segment must be a guest CID this node's transport can
        // carry.
        ("abc", r#"{"name":"list"}"#, "is not a guest CID"),
        ("", r#"{"name":"list"}"#, "no such endpoint"),
        ("0", r#"{"name":"list"}"#, "outside 3..=4294967294"),
        ("2", r#"{"name":"list"}"#, "outside 3..=4294967294"),
        ("4294967295", r#"{"name":"list"}"#, "outside 3..=4294967294"),
        // The body, mirroring the hub's own `/tell` validation.
        ("300", "{}", "invalid tell request"),
        ("300", r#"{"name":""}"#, "a tell needs a tool name"),
        (
            "300",
            long_body.as_str(),
            "the tool name is longer than 256 bytes",
        ),
        (
            "300",
            r#"{"name":"list","args":[]}"#,
            "args must be a JSON object",
        ),
        (
            "300",
            r#"{"name":"list","args":"x"}"#,
            "args must be a JSON object",
        ),
        (
            "300",
            r#"{"name":"list","args":5}"#,
            "args must be a JSON object",
        ),
        (
            "300",
            r#"{"name":"list","args":null}"#,
            "args must be a JSON object",
        ),
        (
            "300",
            r#"{"name":"list","timeout_ms":0}"#,
            "timeout_ms must be 1..=5000",
        ),
        (
            "300",
            r#"{"name":"list","timeout_ms":5001}"#,
            "timeout_ms must be 1..=5000",
        ),
    ];
    for (cid, body, expected) in cases {
        let reply = request(server.port(), "POST", &tell_path(cid), Some(body));
        if expected == "no such endpoint" {
            assert_eq!(reply.status, 404, "{cid} {body}: {}", reply.body);
            continue;
        }
        assert_eq!(reply.status, 400, "{cid} {body}: {}", reply.body);
        let error = reply.json()["error"]
            .as_str()
            .expect("an error string")
            .to_string();
        assert!(error.contains(expected), "{cid} {body}: {error}");
    }

    // The boundaries are inclusive: a 256-byte name and the extreme timeouts
    // are inside the contract (they reach the hub, a dead port here -> a 502).
    let at_limit = "a".repeat(256);
    for body in [
        format!(r#"{{"name":"{at_limit}"}}"#),
        r#"{"name":"list","timeout_ms":1}"#.to_string(),
        r#"{"name":"list","timeout_ms":5000}"#.to_string(),
    ] {
        let reply = request(server.port(), "POST", &tell_path("300"), Some(&body));
        assert_eq!(reply.status, 502, "{body}: {}", reply.body);
    }
}

#[test]
fn an_unconfigured_admin_is_a_503() {
    // `--hub-port off`: the daemon was never told where the hub is,
    // so it says so - and it can do nothing at all, not even for a bad request.
    let server = start_with(console_dir("off"), None);
    for (cid, body) in [("300", r#"{"name":"list"}"#), ("abc", "{")] {
        let reply = request(server.port(), "POST", &tell_path(cid), Some(body));
        assert_eq!(reply.status, 503, "{cid} {body}: {}", reply.body);
        assert_eq!(reply.json()["error"], "the hub is not configured");
    }
}

#[test]
fn an_unreachable_admin_is_a_502_naming_the_reason() {
    let server = start_with(console_dir("unreachable"), admin_at(dead_port()));
    let reply = request(
        server.port(),
        "POST",
        &tell_path("300"),
        Some(r#"{"name":"list"}"#),
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
fn a_malformed_envelope_is_a_502() {
    // A 200 that is not the admin surface's envelope at all is unreadable -
    // never a guessed answer.
    for body in ["not the hub's answer", r#"{"ok":true}"#] {
        let (port, hub) = stub_admin(answer("200 OK", body));
        let server = start_with(console_dir("badenv"), admin_at(port));
        let reply = request(
            server.port(),
            "POST",
            &tell_path("300"),
            Some(r#"{"name":"list"}"#),
        );
        assert_eq!(reply.status, 502, "{body}: {}", reply.body);
        let error = reply.json()["error"]
            .as_str()
            .expect("an error string")
            .to_string();
        assert!(error.contains("not its JSON"), "{error}");
        hub.join().expect("the stub admin finished");
    }
}

/// The stop endpoint's URL for one agent.
fn stop_path(cid: &str) -> String {
    format!("/v1/agents/{cid}/stop")
}

#[test]
fn the_stop_is_driven_through_the_hub_and_passed_through_verbatim() {
    // The hub's own stop view - `stopped: true` with the session's call count -
    // is the answer, byte for byte: this daemon reads no verdict of its own.
    let view = serde_json::json!({ "cid": 300, "stopped": true, "calls": 3 }).to_string();
    let (port, hub) = stub_admin(answer("200 OK", &view));
    let server = start_with(console_dir("stop-ok"), admin_at(port));

    let reply = request(
        server.port(),
        "POST",
        &stop_path("300"),
        Some(r#"{"reason":"the operator stopped this session"}"#),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, view, "the hub's view passes through verbatim");

    let seen = hub.join().expect("the stub answered");
    assert!(seen.starts_with("POST /v1/agents/300/stop "), "the CID is the path's: {seen}");
    assert!(
        seen.contains(r#""reason":"the operator stopped this session""#),
        "the reason rides to the hub: {seen}"
    );
}

#[test]
fn a_stop_the_hub_refuses_keeps_the_hubs_own_words() {
    // An unserved or unconnected CID is the hub's own 404/409, in its own
    // words; and with no reason given, none is sent.
    let refused = serde_json::json!({ "error": "no live agent has guest CID 300" }).to_string();
    let (port, hub) = stub_admin(answer("404 Not Found", &refused));
    let server = start_with(console_dir("stop-refused"), admin_at(port));

    let reply = request(server.port(), "POST", &stop_path("300"), Some("{}"));
    assert_eq!(reply.status, 404, "{}", reply.body);
    assert_eq!(reply.json()["error"], "no live agent has guest CID 300");

    let seen = hub.join().expect("the stub answered");
    assert!(
        !seen.contains("reason"),
        "no reason was given, so none rides: {seen}"
    );
}

#[test]
fn a_long_reason_is_refused_before_the_hub_is_contacted() {
    // 257 bytes is past the hub's own 256-byte bound (`MAX_STOP_REASON`), so
    // the daemon refuses it itself - against a dead admin port, a 502 would
    // mean it had tried anyway.
    let server = start_with(console_dir("stop-long"), admin_at(dead_port()));
    let long = "x".repeat(257);
    let reply = request(
        server.port(),
        "POST",
        &stop_path("300"),
        Some(&format!(r#"{{"reason":"{long}"}}"#)),
    );
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert_eq!(reply.json()["error"], "the reason is longer than 256 bytes");
}

#[test]
fn an_answer_for_another_cid_is_not_this_agents_stop() {
    // A stub that answers for a CID this request did not name is not the
    // answer to it: the daemon refuses to pass it off as one.
    let (port, hub) = stub_admin(answer(
        "200 OK",
        &serde_json::json!({ "cid": 999, "stopped": true, "calls": 0 }).to_string(),
    ));
    let server = start_with(console_dir("stop-other"), admin_at(port));

    let reply = request(server.port(), "POST", &stop_path("300"), Some("{}"));
    assert_eq!(reply.status, 502, "{}", reply.body);
    let error = reply.json()["error"]
        .as_str()
        .expect("an error string")
        .to_string();
    assert!(error.contains("999"), "{error}");
    assert!(error.contains("300"), "{error}");
    hub.join().expect("the stub answered");
}

#[test]
fn the_stop_route_leaves_a_bad_cid_and_a_bad_body_to_its_own_contract() {
    let server = start_with(console_dir("stop-bad"), admin_at(dead_port()));

    // A segment outside the guest CID range is this endpoint's 400.
    let bad_cid = request(server.port(), "POST", &stop_path("nope"), Some("{}"));
    assert_eq!(bad_cid.status, 400, "{}", bad_cid.body);

    // A body that is not the request shape is a 400 naming the parse.
    let bad_body = request(server.port(), "POST", &stop_path("300"), Some("{"));
    assert_eq!(bad_body.status, 400, "{}", bad_body.body);
    assert!(
        bad_body.json()["error"]
            .as_str()
            .expect("an error string")
            .starts_with("invalid stop request:"),
        "{}",
        bad_body.body
    );
}
