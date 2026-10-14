//! Shared helpers for the daemon's integration tests: a tiny blocking HTTP
//! client (the daemon speaks HTTP, so its tests do too, with no client
//! dependency) and a daemon on an ephemeral port.
//!
//! Each test binary uses part of this module, so unused items are expected.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use noded::{Config, Server};

/// How long a test waits for a response before failing.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A path nothing exists at: a run whose VMs boot from it always fails, which
/// is how the tests exercise the create, kill and input paths without needing
/// /dev/kvm or the guest images.
pub fn missing_kernel() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "noded-test-{}-{unique}-missing-kernel",
        std::process::id()
    ))
}

/// A daemon serving on an ephemeral port whose runs boot from
/// [`missing_kernel`].
pub fn start() -> Server {
    Server::start(config()).expect("the test daemon should bind")
}

/// The config [`start`] runs a daemon with.
pub fn config() -> Config {
    Config {
        port: 0,
        kernel: missing_kernel(),
        initrd: missing_kernel(),
        uds_dir: PathBuf::from("/tmp/plaza-vsock"),
        workspace: None,
        ..Config::default()
    }
}

/// One response, read off the socket.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

impl Reply {
    /// The body as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|err| panic!("the body is not JSON ({err}): {}", self.body))
    }
}

/// Sends one request and reads the whole response.
pub fn request(port: u16, method: &str, target: &str, body: Option<&str>) -> Reply {
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: localhost\r\n");
    if let Some(body) = body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut raw = head.into_bytes();
    if let Some(body) = body {
        raw.extend_from_slice(body.as_bytes());
    }
    send(port, &raw)
}

/// Sends `raw` as the whole request (used for malformed requests too) and
/// reads the whole response.
pub fn send(port: u16, raw: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the daemon should accept");
    stream
        .set_read_timeout(Some(TIMEOUT))
        .expect("a read timeout");
    stream
        .write_all(raw)
        .expect("the request should be written");
    stream.flush().expect("the request should be flushed");
    // The tests send exactly one request, so the daemon can be told the
    // request is over: a request whose body is short then fails at once
    // instead of waiting for the read timeout.
    stream
        .shutdown(Shutdown::Write)
        .expect("the write half should close");

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).expect("a status line");
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status code in {status_line:?}"));
    let mut content_type = String::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("a header line");
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Type:") {
            content_type = value.trim().to_string();
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().expect("a numeric Content-Length");
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).expect("the whole body");
    Reply {
        status,
        content_type,
        body: String::from_utf8(body).expect("a UTF-8 body"),
    }
}
