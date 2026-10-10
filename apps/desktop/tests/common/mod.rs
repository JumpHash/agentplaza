//! Shared helpers for the dashboard's integration tests: a daemon stub that
//! speaks exactly the bytes `noded` speaks, over a real socket.
//!
//! This mirrors `crates/noded/tests/common/mod.rs`, which stubs the *client*
//! side with nothing but `std::net`; this stubs the server side the same way,
//! so the client and the rendered application both talk to real connections
//! rather than a mock.
//!
//! Each test binary uses part of this module, so unused items are expected.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

/// A list body as `GET /api/v0/runs` sends it: one live run with two vsock
/// VMs, next to one killed run without vsock.
pub const LIST: &str = r#"{"runs":[
    {"id":"r1","state":"running","vcpus":2,"memory_mib":512,"beats":null,
     "created_at":1760000000000,"ended_at":null,
     "vms":[{"id":0,"guest_cid":50,"state":"running"},
            {"id":1,"guest_cid":51,"state":"running"}]},
    {"id":"r2","state":"killed","vcpus":1,"memory_mib":128,"beats":3,
     "created_at":1760000001000,"ended_at":1760000009000,
     "vms":[{"id":0,"state":"killed"}]}
]}"#;

/// A single-run body as `GET /api/v0/runs/r1` sends it, with the serial.
pub const DETAIL: &str = r#"{"id":"r1","state":"running","vcpus":2,"memory_mib":512,
    "beats":null,"created_at":1760000000000,"ended_at":null,
    "vms":[{"id":0,"guest_cid":50,"state":"running",
            "serial":"login: plaza\r\n$ uname -a\r\nLinux plaza 7.2.8 x86_64\r\n"}]}"#;

/// One request as the stub read it.
#[derive(Debug)]
pub struct Request {
    /// The method.
    pub method: String,
    /// The request target: path plus any query.
    pub target: String,
    /// The headers, in the order they arrived.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: String,
}

impl Request {
    /// A header's value, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A daemon stub on an ephemeral port.
pub struct Stub {
    /// The port it listens on.
    pub port: u16,
    handle: Option<thread::JoinHandle<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}

impl Stub {
    /// Stops the stub and returns every request it read.
    pub fn requests(mut self) -> Vec<Request> {
        self.stop();
        self.handle
            .take()
            .expect("the stub is joined once")
            .join()
            .expect("the stub should finish")
    }

    /// Stops the stub, unblocking its accept loop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The bytes of one JSON response.
pub fn response(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A stub answering every request with `always`, however many arrive.
pub fn always(status: u16, body: &str) -> Stub {
    let body = body.to_string();
    spawn(Box::new(move |_, _| Some(response(status, &body))))
}

/// A stub answering `replies` in order; a request past the end is a 500.
pub fn replies(replies: Vec<(u16, String)>) -> Stub {
    let mut queue = replies.into_iter();
    spawn(Box::new(move |_, _| match queue.next() {
        Some((status, body)) => Some(response(status, &body)),
        None => Some(response(500, r#"{"error":"no more replies"}"#)),
    }))
}

/// A stub sending exactly `head` and closing, whatever it is asked: for
/// malformed answers.
pub fn raw(head: &str) -> Stub {
    let mut head = Some(head.as_bytes().to_vec());
    spawn(Box::new(move |_, _| head.take()))
}

/// A stub with the daemon's two read endpoints: `GET /api/v0/runs` answers
/// `list`, and any single run answers `detail`.
pub fn fleet(list: &str, detail: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    spawn(Box::new(move |request, _| {
        let body = if request.target == "/api/v0/runs" {
            list.as_str()
        } else {
            detail.as_str()
        };
        Some(response(200, body))
    }))
}

/// How a stub answers: the bytes to send for a request, or `None` to send
/// nothing at all (which is what a malformed answer looks like).
type Answer = Box<dyn FnMut(&Request, &mut BufReader<TcpStream>) -> Option<Vec<u8>> + Send>;

/// Runs a stub on an ephemeral port until it is stopped.
fn spawn(mut answer: Answer) -> Stub {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("the stub should bind");
    let port = listener
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for stream in listener.incoming() {
            if stopping.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { break };
            let mut stream = BufReader::new(stream);
            let request = read_request(&mut stream);
            let answer = answer(&request, &mut stream);
            requests.push(request);
            if let Some(bytes) = answer {
                let _ = stream.get_mut().write_all(&bytes);
                let _ = stream.get_mut().flush();
            }
        }
        requests
    });
    Stub {
        port,
        handle: Some(handle),
        stop,
    }
}

/// Reads one request the way the daemon does: head, then `Content-Length`.
fn read_request(stream: &mut BufReader<TcpStream>) -> Request {
    let mut line = String::new();
    let _ = stream.read_line(&mut line);
    let line = line.trim_end();
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }

    let mut body = vec![0u8; content_length];
    if stream.read_exact(&mut body).is_err() {
        body.clear();
    }
    Request {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}
