//! The hub proxy: `GET /api/v0/hub` and `GET /api/v0/hub/healthz`, exercised
//! over real sockets.
//!
//! A stub hub - a `TcpListener` in this test serving a fixed raw answer - is
//! what the daemon proxies, so the byte-for-byte pass-through and every
//! failure mode are asserted end to end, without the live `vsockd` (which this
//! suite must never touch). The daemon itself is the real `noded` on an
//! ephemeral port, with a console directory that is empty and runs that boot
//! from a missing kernel.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use common::{request, start_with};
use noded::hub::HubStatus;

/// The frozen `/status` body, exactly as `vsockd --status-port` serves it with
/// a keepalive guest connected: the shape the console parses, and the bytes
/// the proxy must pass through unchanged.
const FROZEN_STATUS: &str = concat!(
    r#"{"node":{"cids":[10,11,12,13,14,15],"port":5000,"uptime_s":673,"#,
    r#""max_sessions":64,"keepalive_idle_s":300},"#,
    r#""agents":[{"cid":10,"role":"builder","connected_at_ms":1760000000000,"#,
    r#""last_heartbeat_ms":1760000009000,"calls":7}],"sessions_active":1}"#
);

/// The `/status` body with nobody connected: still an answer, never an error.
const FROZEN_STATUS_EMPTY: &str = concat!(
    r#"{"node":{"cids":[10,11,12,13,14,15],"port":5000,"uptime_s":673,"#,
    r#""max_sessions":64,"keepalive_idle_s":300},"agents":[],"sessions_active":0}"#
);

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-hub-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a console directory");
    dir
}

/// A hub's status surface url for a stub on `port`.
fn hub_at(port: u16) -> Option<HubStatus> {
    Some(HubStatus::parse(&format!("127.0.0.1:{port}")).expect("a loopback surface"))
}

/// A stub hub: binds an ephemeral loopback port, answers one connection with
/// `raw`, and hands back the request line it read, so the proxy's own request
/// can be asserted.
fn stub_hub(raw: &'static [u8]) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the stub hub binds");
    let port = listener.local_addr().expect("a bound address").port();
    let handle = thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().expect("the proxy connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("a read timeout");
        let mut reader =
            BufReader::new(stream.try_clone().expect("the stub clones its socket"));
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .expect("the proxy's request line");
        // Drain the rest of the head so the stub is not answering mid-request.
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("a header line") == 0 || line == "\r\n" {
                break;
            }
        }
        stream.write_all(raw).expect("the stub answers");
        stream.flush().expect("the answer is flushed");
        request_line.trim_end().to_string()
    });
    (port, handle)
}

/// A hub that accepts the connection and then says nothing: the proxy must not
/// wait on it forever.
fn silent_hub() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the stub hub binds");
    let port = listener.local_addr().expect("a bound address").port();
    thread::spawn(move || {
        let (stream, _peer) = listener.accept().expect("the proxy connects");
        // Hold the connection open, silent, past the proxy's own timeout.
        thread::sleep(Duration::from_secs(7));
        drop(stream);
    });
    port
}

/// A loopback port nothing is listening on: bound to learn a free one, then
/// closed, so a connect is refused.
fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("a bound address").port();
    drop(listener);
    port
}

/// A complete stub answer with `content_type`, `body` and a matching length.
fn answer(status: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[test]
fn the_hub_status_is_proxied_byte_for_byte() {
    let raw = answer("200 OK", "application/json; charset=utf-8", FROZEN_STATUS);
    let (port, hub) = stub_hub(raw.leak());
    let server = start_with(console_dir("verbatim"), hub_at(port));

    let reply = request(server.port(), "GET", "/api/v0/hub", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.content_type, "application/json; charset=utf-8");
    // The body is the hub's, byte for byte: not a re-serialization of it.
    assert_eq!(reply.body, FROZEN_STATUS);
    // The shape the console parses field for field.
    let json = reply.json();
    assert_eq!(json["node"]["cids"], serde_json::json!([10, 11, 12, 13, 14, 15]));
    assert_eq!(json["agents"][0]["role"], "builder");
    assert_eq!(json["sessions_active"], 1);

    // The proxy asked the hub for exactly `/status`.
    assert_eq!(hub.join().expect("the stub hub finished"), "GET /status HTTP/1.1");

    // ... and the run API still answers on the same daemon.
    let runs = request(server.port(), "GET", "/api/v0/runs", None);
    assert_eq!(runs.status, 200, "{}", runs.body);
    assert_eq!(runs.json(), serde_json::json!({ "runs": [] }));
}

#[test]
fn an_empty_registry_is_the_hubs_own_answer_not_an_error() {
    let raw = answer("200 OK", "application/json; charset=utf-8", FROZEN_STATUS_EMPTY);
    let (port, hub) = stub_hub(raw.leak());
    let server = start_with(console_dir("empty"), hub_at(port));

    let reply = request(server.port(), "GET", "/api/v0/hub", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, FROZEN_STATUS_EMPTY);
    assert_eq!(reply.json()["agents"], serde_json::json!([]));
    hub.join().expect("the stub hub finished");
}

#[test]
fn the_hub_healthz_is_proxied() {
    let raw = answer("200 OK", "text/plain; charset=utf-8", "ok");
    let (port, hub) = stub_hub(raw.leak());
    let server = start_with(console_dir("healthz"), hub_at(port));

    let reply = request(server.port(), "GET", "/api/v0/hub/healthz", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, "ok");
    assert_eq!(hub.join().expect("the stub hub finished"), "GET /healthz HTTP/1.1");
}

#[test]
fn an_unreachable_hub_is_a_502_naming_the_reason() {
    // Nothing is listening: the console must be able to tell this from an
    // empty registry.
    let server = start_with(console_dir("unreachable"), hub_at(dead_port()));
    for path in ["/api/v0/hub", "/api/v0/hub/healthz"] {
        let reply = request(server.port(), "GET", path, None);
        assert_eq!(reply.status, 502, "{path}: {}", reply.body);
        let error = reply.json()["error"]
            .as_str()
            .expect("an error string")
            .to_string();
        assert!(error.starts_with("hub unreachable: "), "{path}: {error}");
        assert!(error.contains("cannot connect"), "{path}: {error}");
    }
}

#[test]
fn a_malformed_hub_answer_is_a_502() {
    let (port, hub) = stub_hub(b"not an HTTP response at all\r\n\r\n");
    let server = start_with(console_dir("malformed"), hub_at(port));

    let reply = request(server.port(), "GET", "/api/v0/hub", None);
    assert_eq!(reply.status, 502, "{}", reply.body);
    let error = reply.json()["error"]
        .as_str()
        .expect("an error string")
        .to_string();
    assert!(error.starts_with("hub unreachable: "), "{error}");
    assert!(error.contains("not HTTP/1.x"), "{error}");
    hub.join().expect("the stub hub finished");
}

#[test]
fn a_silent_hub_is_a_502_not_a_hang() {
    let server = start_with(console_dir("silent"), hub_at(silent_hub()));

    let started = Instant::now();
    let reply = request(server.port(), "GET", "/api/v0/hub", None);
    let waited = started.elapsed();
    assert_eq!(reply.status, 502, "{}", reply.body);
    let error = reply.json()["error"]
        .as_str()
        .expect("an error string")
        .to_string();
    assert!(error.contains("did not answer within"), "{error}");
    // The daemon's own read timeout is 5s; the request must not have waited on
    // the silent hub beyond it.
    assert!(waited < Duration::from_secs(7), "waited {waited:?}");
}

#[test]
fn an_unconfigured_hub_is_a_503() {
    // `--hub-status off`: the daemon was never told where the hub is, so it
    // must say so rather than report an empty registry.
    let server = start_with(console_dir("off"), None);
    for path in ["/api/v0/hub", "/api/v0/hub/healthz"] {
        let reply = request(server.port(), "GET", path, None);
        assert_eq!(reply.status, 503, "{path}: {}", reply.body);
        assert_eq!(reply.json()["error"], "hub status is not configured", "{path}");
    }
}

#[test]
fn an_unsupported_method_on_the_hub_routes_is_405() {
    let server = start_with(console_dir("405"), None);
    for (method, path) in [
        ("POST", "/api/v0/hub"),
        ("DELETE", "/api/v0/hub"),
        ("PUT", "/api/v0/hub/healthz"),
    ] {
        let reply = request(server.port(), method, path, Some("{}"));
        assert_eq!(reply.status, 405, "{method} {path}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{method} {path}");
    }
}

#[test]
fn the_hub_routes_leave_the_other_endpoints_untouched() {
    // Off and unreachable: in neither case do the run or machine endpoints
    // answer any differently than they did before the proxy existed.
    for (name, hub) in [
        ("unreachable", hub_at(dead_port())),
        ("off", None),
    ] {
        let server = start_with(console_dir(&format!("kept-{name}")), hub);
        let port = server.port();
        let runs = request(port, "GET", "/api/v0/runs", None);
        assert_eq!(runs.status, 200, "{name}: {}", runs.body);
        assert_eq!(runs.json(), serde_json::json!({ "runs": [] }), "{name}");

        let machine = request(port, "GET", "/api/v0/machine", None);
        assert_eq!(machine.status, 200, "{name}: {}", machine.body);
        assert!(machine.json()["vsock_mode"].is_string(), "{name}");

        let created = request(
            port,
            "POST",
            "/api/v0/runs",
            Some(r#"{"n":1,"vcpus":1,"memory_mib":128}"#),
        );
        assert_eq!(created.status, 201, "{name}: {}", created.body);
        assert_eq!(created.json()["state"], "failed", "{name}");

        let missing = request(port, "GET", "/api/v0/nope", None);
        assert_eq!(missing.status, 404, "{name}: {}", missing.body);
    }
}
