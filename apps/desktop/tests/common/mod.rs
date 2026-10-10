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

/// A single-run body whose VMs have both left the running state: vm 0 slept
/// through ACPI S5, vm 1 was reset by the guest's own reboot.
pub const EXITS: &str = r#"{"id":"r1","state":"done","vcpus":1,"memory_mib":256,
    "beats":3,"created_at":1760000000000,"ended_at":1760000060000,
    "vms":[{"id":0,"guest_cid":40,"state":"exited",
            "serial":"$ poweroff\r\n[   12.4] reboot: Power down\r\n"},
           {"id":1,"guest_cid":41,"state":"killed",
            "serial":"$ reboot\r\n[   12.4] machine restart\r\n"}]}"#;

/// A machine body as `GET /api/v0/machine` sends it.
pub const MACHINE: &str = r#"{"name":"plaza-node","arch":"x86_64","backend":"kvm",
    "irqchip":"ioapic","class":"server","cpus":24,"memory_mib":128320,
    "vsock_mode":"uds-muxer","started_at_ms":1760000000000}"#;

/// A register as the daemon reports it once the service has booted its
/// initial setup: one long-running run whose VM rows each carry their role,
/// next to an ordinary run whose VMs carry none.
pub const ROLE_LIST: &str = r#"{"runs":[
    {"id":"r1","state":"running","vcpus":1,"memory_mib":128,"beats":null,
     "created_at":1760000000000,"ended_at":null,
     "vms":[{"id":0,"guest_cid":100,"role":"builder","state":"running"},
            {"id":1,"guest_cid":101,"role":"reviewer","state":"running"},
            {"id":2,"guest_cid":102,"role":"legal-gov-prosecutor","state":"exited"}]},
    {"id":"r2","state":"done","vcpus":1,"memory_mib":128,"beats":3,
     "created_at":1760000001000,"ended_at":1760000060000,
     "vms":[{"id":0,"guest_cid":50,"state":"exited"}]}
]}"#;

/// The same role run as a single-run GET reports it, with each agent's serial.
pub const ROLE_DETAIL: &str = r#"{"id":"r1","state":"running","vcpus":1,"memory_mib":128,
    "beats":null,"created_at":1760000000000,"ended_at":null,
    "vms":[{"id":0,"guest_cid":100,"role":"builder","state":"running",
            "serial":"agent-harness: role builder accepted\r\n$ uname\r\nLinux plaza\r\n"},
           {"id":1,"guest_cid":101,"role":"reviewer","state":"running",
            "serial":"agent-harness: role reviewer accepted\r\n$ uptime\r\n"},
           {"id":2,"guest_cid":102,"role":"legal-gov-prosecutor","state":"exited",
            "serial":"$ halted\r\n"}]}"#;

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

/// The bytes of one event-stream answer: exactly the head `noded` writes
/// (`crates/noded/src/events.rs`), then `body`'s frames.
pub fn stream_head(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}"
    )
    .into_bytes()
}

/// A stub answering every event-stream connection with the next body in
/// `bodies` — the last one repeating — and closing it, which is how the
/// daemon's stream ends.
///
/// The stub reads the request first, like the daemon, so a console's own
/// request is on the wire before the frames are.
pub fn streams(bodies: Vec<&'static str>) -> Stub {
    let mut next = 0usize;
    spawn(Box::new(move |_, _| {
        let body = bodies
            .get(next)
            .or_else(|| bodies.last())
            .copied()
            .unwrap_or("");
        next += 1;
        Some(stream_head(body))
    }))
}

/// A whole stub daemon: the register's paths answer `list`/`detail`, the
/// machine `MACHINE`, and `GET /api/v0/events` an event stream of `events`.
pub fn live(list: &str, detail: &str, events: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    spawn(Box::new(move |request, _| {
        Some(match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/api/v0/events") => events.clone(),
            ("GET", "/api/v0/machine") => response(200, MACHINE),
            ("GET", "/api/v0/runs") => response(200, &list),
            _ => response(200, &detail),
        })
    }))
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

/// A stub with the daemon's whole read path and its console write path:
/// `GET /api/v0/runs` answers `list`, `GET /api/v0/machine` answers
/// [`MACHINE`], and any single run answers `detail` — including the answer to
/// `POST /api/v0/runs/{id}/input`, which the daemon also answers with the run.
///
/// Every request is recorded, so a test can assert exactly what the console
/// put on the wire.
pub fn fleet(list: &str, detail: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/api/v0/runs") => list.as_str(),
            ("GET", "/api/v0/machine") => MACHINE,
            _ => detail.as_str(),
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
