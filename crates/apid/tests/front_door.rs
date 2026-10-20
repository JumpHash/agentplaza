//! The hub's front door, end to end on a scratch hub. A caller from another machine (here a connection that opens
//! with the relay preface, which the server serves as one from elsewhere, peer 0.0.0.0) gets no loopback trust:
//! owner routes refuse it without the owner token or with a wrong one, while the public routes stay open. Every
//! other test calls from loopback, where the caller is the owner, so without this one a gate that admitted everyone
//! would pass the whole suite.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use common::{Hub, Scratch};

/// The status of one request sent as a caller from elsewhere.
fn from_elsewhere(hub: &Hub, method: &str, path: &str, headers: &[(&str, &str)]) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: hub.example:{}\r\nContent-Length: 0\r\n",
        hub.port
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream
        .write_all(apz_http::RELAY_PREFACE)
        .expect("send the preface");
    stream.write_all(head.as_bytes()).expect("send the request");
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    reply
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

#[test]
fn a_caller_from_elsewhere_gets_owner_routes_only_with_the_owner_token() {
    let scratch = Scratch::new("front-door");
    let hub = Hub::start(&scratch, None, None);
    // Public routes answer anyone.
    assert_eq!(from_elsewhere(&hub, "GET", "/v1/hub/identity", &[]), 200);
    // Owner routes refuse a caller with no token, a wrong token or the internal token.
    assert_eq!(from_elsewhere(&hub, "GET", "/v1/settings", &[]), 401);
    assert_eq!(from_elsewhere(&hub, "POST", "/v1/plaza/resume", &[]), 401);
    let wrong = [("authorization", "Bearer apzo_not-the-owner-token")];
    assert_eq!(from_elsewhere(&hub, "GET", "/v1/settings", &wrong), 401);
    let internal = format!("Bearer {}", hub.token());
    assert_eq!(
        from_elsewhere(&hub, "GET", "/v1/settings", &[("authorization", &internal)]),
        401
    );
}
