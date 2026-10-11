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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

/// A list body as `GET /v1/runs` sends it: one live run with two vsock
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

/// A single-run body as `GET /v1/runs/r1` sends it, with the serial.
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

/// The run of [`LIST`] as a report reads it: vm 0's capture is a frozen v1
/// session that completed, vm 1's is a keepalive agent that has beaten and
/// served a tool call and not been stopped. The console's own boot lines are
/// interleaved as they really are.
pub const REPORT_DETAIL: &str = r#"{"id":"r1","state":"running","vcpus":2,"memory_mib":512,
    "beats":null,"created_at":1760000000000,"ended_at":null,
    "vms":[{"id":0,"guest_cid":50,"state":"running",
            "serial":"plaza guest: plaza.harness=1: loading the vsock modules\r\n[    0.417190] virtio_vsock: vsock ready\r\n[vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":50}\r\n[vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\r\nagent-harness: handshake complete (host echo \"plaza-orchestrator\")\r\n[vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{\"text\":\"plaza-guest-50-hello\"}}\r\n[vsock<-] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"plaza-guest-50-hello\"}\r\n[vsock->] {\"type\":\"tool_call\",\"call\":2,\"name\":\"uptime\",\"args\":{}}\r\n[vsock<-] {\"type\":\"tool_result\",\"call\":2,\"ok\":true,\"output\":\"12.34 45.67\"}\r\n[vsock->] {\"type\":\"bye\",\"calls\":2}\r\nagent-harness: session complete after 2 tool calls\r\n"},
           {"id":1,"guest_cid":51,"state":"running",
            "serial":"[vsock->] {\"type\":\"hello\",\"vm\":1,\"guest_cid\":51,\"keepalive\":true}\r\n[vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\",\"keepalive\":true}\r\n[vsock->] {\"type\":\"heartbeat\",\"seq\":1}\r\n[vsock<-] {\"type\":\"tool_call\",\"call\":1,\"name\":\"uptime\",\"args\":{}}\r\n[vsock->] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"1.0 2.0\"}\r\n"}]}"#;

/// A machine body as `GET /v1/machine` sends it.
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

/// A tasks body as `GET /v1/tasks` sends it, byte for byte the shape the
/// live daemon answers with: a task in an agent's hands next to one nobody has
/// taken, and one that has ended with a note.
pub const TASKS: &str = r#"{"tasks":[
    {"id":"t1","goal":"write the plaza handbooks","acceptance":"all 21 present",
     "budget":2000,"role":"librarian","state":"claimed","outcome":null,
     "assignee_id":"librarian","verifier_id":null,"agent_cid":112,"note":null,
     "created_at":1791671813178,"updated_at":1791671813568},
    {"id":"t2","goal":"port the ledger to the console","acceptance":null,
     "budget":0,"role":null,"state":"open","outcome":null,"assignee_id":null,
     "verifier_id":null,"agent_cid":null,"note":null,"created_at":1791671900000,
     "updated_at":1791671900000},
    {"id":"t3","goal":"audit the egress allowlist","acceptance":"every negative",
     "budget":50,"role":"auditor","state":"done","outcome":"accepted",
     "assignee_id":"auditor","verifier_id":null,"agent_cid":118,
     "note":"every negative covered","created_at":1791671700000,
     "updated_at":1791671810000}
]}"#;

/// A single-task body as `GET /v1/tasks/t1` sends it: the same task after
/// its agent claimed it.
pub const TASK: &str = r#"{"id":"t1","goal":"write the plaza handbooks",
    "acceptance":"all 21 present","budget":2000,"role":"librarian","state":"claimed",
    "outcome":null,"assignee_id":"librarian","verifier_id":null,"agent_cid":112,
    "note":null,"created_at":1791671813178,"updated_at":1791671813568}"#;

/// A ledger stub: `GET /v1/tasks` answers `tasks`, and every other path
/// answers `task` — including the claim, finish and withdraw answers, which
/// the daemon also answers with the task they moved.
pub fn ledger(tasks: &str, task: &str) -> Stub {
    let tasks = tasks.to_string();
    let task = task.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/tasks") => tasks.as_str(),
            _ => task.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A whole stub daemon for the ledger pane: the register, the machine, an
/// event stream, and the ledger's own paths — `GET /v1/tasks` answers
/// `tasks`, and every other non-run path answers `task`.
pub fn ledger_live(list: &str, detail: &str, events: &str, tasks: &str, task: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let tasks = tasks.to_string();
    let task = task.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            // The ledger's own paths: the list is the collection's GET, and
            // everything else — a create posting to it, a move on one task —
            // is answered with the task itself.
            ("GET", "/v1/tasks") => tasks.as_str(),
            (_, "/v1/tasks") => task.as_str(),
            (_, target) if target.starts_with("/v1/tasks/") => task.as_str(),
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A treasury body as `GET /v1/treasury` sends it, byte for byte the shape
/// the live daemon answered with after one budgeted task's cycle: the mint
/// account and the task's escrow, both derived to zero, and the totals.
pub const TREASURY: &str = r#"{"accounts":[
    {"id":1,"kind":"mint","owner_ref":"mint","credit_limit":0,"balance":0,
     "created_at":1791680742762},
    {"id":2,"kind":"task_escrow","owner_ref":"t1","credit_limit":0,"balance":0,
     "created_at":1791680766728}],
    "totals":{"accounts":2,"entries":2,"credits":1500,"debits":1500,"balance":0},
    "spent":0,"note":"v0 meters no real spend; a done task refunds its whole escrow"}"#;

/// A journal body as `GET /v1/treasury/journal` sends it: the one cycle's
/// two entries, each with its postings, oldest first.
pub const TREASURY_JOURNAL: &str = r#"{"entries":[
    {"id":1,"kind":"mint","task_id":"t1","root_task":"t1","memo":"task budget",
     "posted_by":"treasury-service","created_at":1791680766728,"amount":750,
     "postings":[{"account":1,"kind":"mint","owner_ref":"mint","amount":-750},
                 {"account":2,"kind":"task_escrow","owner_ref":"t1","amount":750}]},
    {"id":2,"kind":"refund","task_id":"t1","root_task":"t1","memo":"task budget refunded",
     "posted_by":"treasury-service","created_at":1791680768447,"amount":750,
     "postings":[{"account":2,"kind":"task_escrow","owner_ref":"t1","amount":-750},
                 {"account":1,"kind":"mint","owner_ref":"mint","amount":750}]}],
    "total":2,"limit":256,"truncated":false}"#;

/// A treasury whose journal is empty: the mint account alone, before anything
/// has been posted. The daemon's own answer on a fresh instance.
pub const TREASURY_BARE: &str = r#"{"accounts":[
    {"id":1,"kind":"mint","owner_ref":"mint","credit_limit":0,"balance":0,
     "created_at":1791680742762}],
    "totals":{"accounts":1,"entries":0,"credits":0,"debits":0,"balance":0},
    "spent":0,"note":"v0 meters no real spend; a done task refunds its whole escrow"}"#;

/// The empty journal that goes with [`TREASURY_BARE`].
pub const TREASURY_JOURNAL_EMPTY: &str =
    r#"{"entries":[],"total":0,"limit":256,"truncated":false}"#;

/// The journal of [`TREASURY_JOURNAL`] after one more entry was posted: a
/// second task's budget minted into its own escrow. The entry a `treasury`
/// frame carries cannot describe this — no memo, no postings, no clock — so a
/// console that draws it drew it from a read.
pub const TREASURY_JOURNAL_AFTER: &str = r#"{"entries":[
    {"id":1,"kind":"mint","task_id":"t1","root_task":"t1","memo":"task budget",
     "posted_by":"treasury-service","created_at":1791680766728,"amount":750,
     "postings":[{"account":1,"kind":"mint","owner_ref":"mint","amount":-750},
                 {"account":2,"kind":"task_escrow","owner_ref":"t1","amount":750}]},
    {"id":2,"kind":"refund","task_id":"t1","root_task":"t1","memo":"task budget refunded",
     "posted_by":"treasury-service","created_at":1791680768447,"amount":750,
     "postings":[{"account":2,"kind":"task_escrow","owner_ref":"t1","amount":-750},
                 {"account":1,"kind":"mint","owner_ref":"mint","amount":750}]},
    {"id":3,"kind":"mint","task_id":"t2","root_task":"t2","memo":"task budget",
     "posted_by":"treasury-service","created_at":1791680900000,"amount":500,
     "postings":[{"account":1,"kind":"mint","owner_ref":"mint","amount":-500},
                 {"account":3,"kind":"task_escrow","owner_ref":"t2","amount":500}]}],
    "total":3,"limit":256,"truncated":false}"#;

/// A whole stub daemon for the treasury pane: the register, the machine, an
/// event stream, and the treasury's two paths — `GET /v1/treasury`
/// answers `treasury`, `GET /v1/treasury/journal` answers `journal`, and
/// every other read answers `detail`.
pub fn treasury_live(
    list: &str,
    detail: &str,
    events: &str,
    treasury: &str,
    journal: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let treasury = treasury.to_string();
    let journal = journal.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            // The treasury's two halves: the accounts and the journal.
            ("GET", "/v1/treasury") => treasury.as_str(),
            ("GET", "/v1/treasury/journal") => journal.as_str(),
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A whole stub daemon whose treasury journal grows between reads: the first
/// read answers `journal`, and every later read answers `journal_after` — the
/// daemon's own answer once one more entry has been posted.
///
/// It is how a test proves a `treasury` frame asks the pane's own surface for
/// a read rather than assembling an entry from the frame: the entry the frame
/// never carried is the daemon's words, and it arrives only through a read.
pub fn treasury_growing(
    list: &str,
    detail: &str,
    events: &str,
    treasury: &str,
    journal: &str,
    journal_after: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let treasury = treasury.to_string();
    let journal = journal.to_string();
    let journal_after = journal_after.to_string();
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/treasury") => treasury.as_str(),
            ("GET", "/v1/treasury/journal") => {
                // The journal the daemon holds now: the one the first read
                // saw, and the longer one after the next entry was posted.
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    journal.as_str()
                } else {
                    journal_after.as_str()
                }
            }
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A ledger whose claims are refused: the register answers `list`, the ledger
/// `tasks`, every other read `task` — and a claim is the daemon's own 409,
/// which names the CID no live agent holds.
pub fn ledger_claim_refused(list: &str, tasks: &str, task: &str, error: &str) -> Stub {
    let list = list.to_string();
    let tasks = tasks.to_string();
    let task = task.to_string();
    let error = error.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("POST", target) if target.ends_with("/claim") => {
                return Some(response(409, &error));
            }
            ("GET", "/v1/runs") => list.as_str(),
            (_, "/v1/tasks") => tasks.as_str(),
            _ => task.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A ledger whose artifact read answers `read` (an HTTP status and a body):
/// the register answers `list`, the ledger `tasks`, every other read `task`,
/// and `POST /v1/tasks/{id}/artifact` answers `read` — the artifact the
/// task's agent read, or the daemon's own refusal (the hub's `403`, the
/// agent's `422`, or a `503` read-through that is switched off).
pub fn ledger_artifact(
    list: &str,
    tasks: &str,
    task: &str,
    read: (u16, &str),
) -> Stub {
    let list = list.to_string();
    let tasks = tasks.to_string();
    let task = task.to_string();
    let (status, body) = (read.0, read.1.to_string());
    spawn(Box::new(move |request, _| {
        if request.method == "POST" && request.target.ends_with("/artifact") {
            return Some(response(status, &body));
        }
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/runs") => list.as_str(),
            (_, "/v1/tasks") => tasks.as_str(),
            _ => task.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A ledger whose one ended task was finished by the dispatcher, so its note
/// is the `artifacts:` line that names the file its agent wrote.
pub const TASKS_ARTIFACT: &str = r#"{"tasks":[
    {"id":"t3","goal":"verify the fixed loop","acceptance":"a note exists",
     "budget":2000,"role":"builder","state":"done","outcome":"accepted",
     "assignee_id":"builder","verifier_id":null,"agent_cid":103,
     "note":"artifacts: build-notes.txt (2 tool call(s))",
     "created_at":1791675134319,"updated_at":1791675198671}
]}"#;

/// An artifact as `POST /v1/tasks/t3/artifact` answers it: the file the
/// task's agent read back, in the shape the live daemon answered with.
pub const ARTIFACT: &str = r#"{"task":"t3","agent_cid":103,"path":"build-notes.txt",
    "content":"task t3: verify the fixed loop\nthe loop held\n","size":45,
    "truncated":false}"#;

/// A whole stub daemon: the register's paths answer `list`/`detail`, the
/// machine `MACHINE`, and `GET /v1/events` an event stream of `events`.
pub fn live(list: &str, detail: &str, events: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    spawn(Box::new(move |request, _| {
        Some(match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => events.clone(),
            ("GET", "/v1/machine") => response(200, MACHINE),
            ("GET", "/v1/runs") => response(200, &list),
            _ => response(200, &detail),
        })
    }))
}

/// A stub answering every request with `always`, however many arrive.
pub fn always(status: u16, body: &str) -> Stub {
    let body = body.to_string();
    spawn(Box::new(move |_, _| Some(response(status, &body))))
}

/// A `/status` body as `vsockd --status-port` serves it, byte for byte: the
/// host facts, one connected keepalive agent, and the live session count.
pub const HUB_STATUS: &str = concat!(
    r#"{"node":{"cids":[10,11,12,13,14,15],"port":5000,"uptime_s":673,"#,
    r#""max_sessions":64,"keepalive_idle_s":300},"#,
    r#""agents":[{"cid":10,"role":"builder","connected_at_ms":1760000000000,"#,
    r#""last_heartbeat_ms":1760000009000,"calls":7}],"sessions_active":1}"#
);

/// The same surface with nobody connected: the live hub's own answer on a
/// host whose keepalive guests have not connected yet.
pub const HUB_STATUS_EMPTY: &str = concat!(
    r#"{"node":{"cids":[10,11,12,13,14,15],"port":5000,"uptime_s":673,"#,
    r#""max_sessions":64,"keepalive_idle_s":300},"agents":[],"sessions_active":0}"#
);

/// A hub's read-only status surface: `GET /status` answers `body`, and every
/// other path or method is the 404 the hub serves.
pub fn hub(body: &str) -> Stub {
    let body = body.to_string();
    spawn(Box::new(move |request, _| {
        Some(match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/status") => response(200, &body),
            _ => response(404, r#"{"error":"no such endpoint"}"#),
        })
    }))
}

/// A `/tell` answer as the hub's admin surface serves it: the agent's own `ok`
/// and the tool's output.
pub const TELL_ANSWER: &str = r#"{"ok":true,"output":"3.20 3.13 1.00"}"#;

/// A `/tell` answer from a tool that declined: `ok:false` with its reason.
pub const TELL_DECLINED: &str = r#"{"ok":false,"output":"denied: no such file"}"#;

/// The hub's role-policy refusal: a `403` in the tell shape, its own words.
pub const TELL_ROLE_POLICY: &str = r#"{"ok":false,"output":"role librarian does not grant list"}"#;

/// A `/stop` answer that ended the session, with the guest's `bye` count.
pub const STOP_ENDED: &str = r#"{"cid":100,"stopped":true,"calls":3}"#;

/// A `/stop` answer whose wait lapsed: the session is still up and nothing was
/// torn down.
pub const STOP_STILL_UP: &str =
    r#"{"cid":100,"stopped":false,"reason":"the agent did not end within 5000 ms"}"#;

/// The hub's admin surface: `POST /tell` answers `tell`, `POST /stop` answers
/// `stop` — an HTTP status and a body each — and every other path or method is
/// the 404 the surface serves.
///
/// Every request is recorded, so a test can assert exactly what the operator's
/// console put on the wire.
pub fn admin(tell: (u16, &str), stop: (u16, &str)) -> Stub {
    let tell = (tell.0, tell.1.to_string());
    let stop = (stop.0, stop.1.to_string());
    spawn(Box::new(move |request, _| {
        Some(match (request.method.as_str(), request.target.as_str()) {
            ("POST", p) if p.starts_with("/v1/agents/") && p.ends_with("/tell") => response(tell.0, &tell.1),
            ("POST", p) if p.starts_with("/v1/agents/") && p.ends_with("/stop") => response(stop.0, &stop.1),
            _ => response(404, r#"{"error":"no such endpoint"}"#),
        })
    }))
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
/// `GET /v1/runs` answers `list`, `GET /v1/machine` answers
/// [`MACHINE`], and any single run answers `detail` — including the answer to
/// `POST /v1/runs/{id}/input`, which the daemon also answers with the run.
///
/// Every request is recorded, so a test can assert exactly what the console
/// put on the wire.
pub fn fleet(list: &str, detail: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/machine") => MACHINE,
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// An agora body as `GET /v1/agora` sends it: a call the award closed —
/// with two bids and the treasury entry that funded it — next to a call still
/// open and unbidden. The live daemon's board for `t1` is this shape.
pub const AGORA: &str = r#"{"calls":[
    {"task":"t1","capability":"librarian","spec_ref":"all 21 handbooks present",
     "max_price":2000,"state":"awarded","created_at":1791671813178,
     "bids":[
        {"task":"t1","agent_cid":112,"bidder":"librarian","price":1800,
         "created_at":1791671820000},
        {"task":"t1","agent_cid":118,"bidder":"librarian","price":1900,
         "created_at":1791671821000}],
     "award":{"task":"t1","contractor":"librarian","agent_cid":112,"price":1800,
              "escrow_entry":1,"created_at":1791671825000}},
    {"task":"t2","capability":"builder","spec_ref":null,"max_price":500,
     "state":"open","created_at":1791671900000,"bids":[],"award":null}
    ],"total":2,"limit":256,"truncated":false}"#;

/// The board of [`AGORA`] once the open call took a bid: what a later read
/// answers with. The bid carries a bidder and a clock a frame cannot — a
/// console that draws it drew it from a read.
pub const AGORA_AFTER: &str = r#"{"calls":[
    {"task":"t1","capability":"librarian","spec_ref":"all 21 handbooks present",
     "max_price":2000,"state":"awarded","created_at":1791671813178,
     "bids":[
        {"task":"t1","agent_cid":112,"bidder":"librarian","price":1800,
         "created_at":1791671820000},
        {"task":"t1","agent_cid":118,"bidder":"librarian","price":1900,
         "created_at":1791671821000}],
     "award":{"task":"t1","contractor":"librarian","agent_cid":112,"price":1800,
              "escrow_entry":1,"created_at":1791671825000}},
    {"task":"t2","capability":"builder","spec_ref":null,"max_price":500,
     "state":"open","created_at":1791671900000,
     "bids":[{"task":"t2","agent_cid":100,"bidder":"builder","price":420,
              "created_at":1791672000000}],
     "award":null}
    ],"total":2,"limit":256,"truncated":false}"#;

/// An empty board: the daemon's own answer before any roled task offers a
/// call, or after every offer is over.
pub const AGORA_BARE: &str = r#"{"calls":[],"total":0,"limit":256,"truncated":false}"#;

/// A whole stub daemon for the market pane: the register, the machine, an
/// event stream, and the agora — `GET /v1/agora` answers `board`, every
/// other read answers `detail`.
pub fn agora_live(list: &str, detail: &str, events: &str, board: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let board = board.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/agora") => board.as_str(),
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A whole stub daemon whose board grows between reads: the first read answers
/// `board`, and every later read answers `board_after` — the daemon's own
/// answer once one more bid was made.
///
/// It is how a test proves an `agora` frame asks the pane's own surface for a
/// read rather than assembling a bid from the frame: the bid the frame never
/// described is the daemon's words, and it arrives only through a read.
pub fn agora_growing(
    list: &str,
    detail: &str,
    events: &str,
    board: &str,
    board_after: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let board = board.to_string();
    let board_after = board_after.to_string();
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/agora") => {
                // The board the daemon holds now: the one the first read saw,
                // and the longer one after the next bid was made.
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    board.as_str()
                } else {
                    board_after.as_str()
                }
            }
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A board body as `GET /v1/board` sends it: a hiring note that links a
/// task, a plain post that links none, and every agent's unread count.
pub const POSTS: &str = r#"{"posts":[
    {"id":"p2","agent_cid":100,"role":"builder","subject":"hiring for t1",
     "body":"I can take t1 · the suite is what I would build to.",
     "task":"t1","posted_by":"dispatcher","created_at":1791672000000},
    {"id":"p1","agent_cid":101,"role":"reviewer","subject":"hello plaza",
     "body":"first post","task":null,"posted_by":"driver","created_at":1791671000000}
    ],"total":2,"limit":1024,"truncated":false,
    "unread":[{"agent_cid":100,"count":0},{"agent_cid":101,"count":1}]}"#;

/// The board of [`POSTS`] once one more post was made: what a later read
/// answers with, and what an SSE frame's read brings in.
pub const POSTS_AFTER: &str = r#"{"posts":[
    {"id":"p3","agent_cid":101,"role":"reviewer","subject":"available for t1",
     "body":"I can review it.","task":"t1","posted_by":"dispatcher",
     "created_at":1791673000000},
    {"id":"p2","agent_cid":100,"role":"builder","subject":"hiring for t1",
     "body":"I can take t1 · the suite is what I would build to.",
     "task":"t1","posted_by":"dispatcher","created_at":1791672000000},
    {"id":"p1","agent_cid":101,"role":"reviewer","subject":"hello plaza",
     "body":"first post","task":null,"posted_by":"driver","created_at":1791671000000}
    ],"total":3,"limit":1024,"truncated":false,
    "unread":[{"agent_cid":100,"count":1},{"agent_cid":101,"count":0}]}"#;

/// A conversations body as `GET /v1/conversations` sends it: one exchange
/// from the builder to the reviewer, and the pair the floor draws adjacent.
pub const CONVERSATIONS: &str = r#"{"conversations":[
    {"id":"c1","from_cid":100,"to_cid":101,"from_role":"builder","to_role":"reviewer",
     "text":"can you review it?","created_at":1791675000000,"delivered":true,
     "delivery":null}
    ],"pairs":[{"a":100,"b":101,"active":true,"count":1,"last_at":1791675000000}],
    "active_secs":120,"total":1,"limit":256,"truncated":false,
    "pairs_total":1,"pairs_truncated":false}"#;

/// The same record once the pair has gone quiet: the pair is no longer active,
/// so its robots walk home.
pub const CONVERSATIONS_QUIET: &str = r#"{"conversations":[
    {"id":"c1","from_cid":100,"to_cid":101,"from_role":"builder","to_role":"reviewer",
     "text":"can you review it?","created_at":1791675000000,"delivered":true,
     "delivery":null}
    ],"pairs":[{"a":100,"b":101,"active":false,"count":1,"last_at":1791675000000}],
    "active_secs":120,"total":1,"limit":256,"truncated":false,
    "pairs_total":1,"pairs_truncated":false}"#;

/// A contacts body as `GET /v1/agents/{cid}/contacts` sends it: the
/// builder's link to the reviewer, recorded as a handshake (`met`).
pub const CONTACTS: &str = r#"{"agent_cid":100,"contacts":[
    {"peer_cid":101,"role":"reviewer","since":1791673000000,"kind":"met"}
    ],"total":1,"limit":256,"truncated":false}"#;

/// The live plaza's own conversation log: three exchanges across two pairs,
/// newest first, with the pair summaries the talk pane's filter walks. It is
/// the shape the plaza answered with when this pane landed; the newest pair is
/// marked active, so the pane's ACTIVE mark (the floor's walk) is drawn.
pub const CONVERSATIONS_TALK: &str = r#"{"conversations":[
    {"id":"c3","from_cid":103,"to_cid":109,"from_role":"builder",
     "to_role":"legal-agents-ombudsman",
     "text":"the east wing is framed; the mayor may inspect the build log",
     "created_at":1791690975823,"delivered":true,"delivery":null},
    {"id":"c2","from_cid":103,"to_cid":109,"from_role":"builder",
     "to_role":"legal-agents-ombudsman","text":"done: t2 — build-notes.txt",
     "created_at":1791690961870,"delivered":true,"delivery":null},
    {"id":"c1","from_cid":100,"to_cid":103,"from_role":"architect","to_role":"builder",
     "text":"the layout review is done; want to take the build?",
     "created_at":1791690261070,"delivered":true,"delivery":null}
    ],"pairs":[
    {"a":103,"b":109,"active":true,"count":2,"last_at":1791690975823},
    {"a":100,"b":103,"active":false,"count":1,"last_at":1791690261070}
    ],"active_secs":120,"total":3,"limit":256,"truncated":false,
    "pairs_total":2,"pairs_truncated":false}"#;

/// The same log once one exchange failed to reach its peer: what the pane's
/// delivery mark draws verbatim, and what its refused tally counts.
pub const CONVERSATIONS_TALK_REFUSED: &str = r#"{"conversations":[
    {"id":"c2","from_cid":103,"to_cid":109,"from_role":"builder",
     "to_role":"legal-agents-ombudsman","text":"done: t2 — build-notes.txt",
     "created_at":1791690961870,"delivered":false,
     "delivery":"no live session for guest CID 109"},
    {"id":"c1","from_cid":100,"to_cid":103,"from_role":"architect","to_role":"builder",
     "text":"the layout review is done; want to take the build?",
     "created_at":1791690261070,"delivered":true,"delivery":null}
    ],"pairs":[
    {"a":103,"b":109,"active":false,"count":1,"last_at":1791690961870},
    {"a":100,"b":103,"active":false,"count":1,"last_at":1791690261070}
    ],"active_secs":120,"total":2,"limit":256,"truncated":false,
    "pairs_total":2,"pairs_truncated":false}"#;

/// One pair's thread as `GET /v1/conversations/{a}/{b}` sends it: the two
/// exchanges between cid 103 and cid 109, oldest first.
pub const THREAD_TALK: &str = r#"{"a":103,"b":109,"active":true,"active_secs":120,"messages":[
    {"id":"c2","from_cid":103,"to_cid":109,"from_role":"builder",
     "to_role":"legal-agents-ombudsman","text":"done: t2 — build-notes.txt",
     "created_at":1791690961870,"delivered":true,"delivery":null},
    {"id":"c3","from_cid":103,"to_cid":109,"from_role":"builder",
     "to_role":"legal-agents-ombudsman",
     "text":"the east wing is framed; the mayor may inspect the build log",
     "created_at":1791690975823,"delivered":true,"delivery":null}
    ],"total":2,"limit":256,"truncated":false}"#;

/// A thread body for a pair that has never spoken: the daemon's own answer for
/// a pair with no exchange, not a refusal.
pub const THREAD_EMPTY: &str = r#"{"a":100,"b":103,"active":false,"active_secs":120,
    "messages":[],"total":0,"limit":256,"truncated":false}"#;

/// The log of [`CONVERSATIONS_TALK`] once one more exchange was made: what a
/// later read answers with, and what a `conversation` frame's read brings in.
/// The new row carries a text and a role a frame cannot, so a console that
/// draws it drew it from a read.
pub const CONVERSATIONS_TALK_AFTER: &str = r#"{"conversations":[
    {"id":"c4","from_cid":109,"to_cid":103,"from_role":"legal-agents-ombudsman",
     "to_role":"builder","text":"the framed east wing is inspected — approved",
     "created_at":1791691000000,"delivered":true,"delivery":null},
    {"id":"c3","from_cid":103,"to_cid":109,"from_role":"builder",
     "to_role":"legal-agents-ombudsman",
     "text":"the east wing is framed; the mayor may inspect the build log",
     "created_at":1791690975823,"delivered":true,"delivery":null},
    {"id":"c2","from_cid":103,"to_cid":109,"from_role":"builder",
     "to_role":"legal-agents-ombudsman","text":"done: t2 — build-notes.txt",
     "created_at":1791690961870,"delivered":true,"delivery":null},
    {"id":"c1","from_cid":100,"to_cid":103,"from_role":"architect","to_role":"builder",
     "text":"the layout review is done; want to take the build?",
     "created_at":1791690261070,"delivered":true,"delivery":null}
    ],"pairs":[
    {"a":103,"b":109,"active":true,"count":3,"last_at":1791691000000},
    {"a":100,"b":103,"active":false,"count":1,"last_at":1791690261070}
    ],"active_secs":120,"total":4,"limit":256,"truncated":false,
    "pairs_total":2,"pairs_truncated":false}"#;

/// A whole stub daemon whose conversation log grows between reads: the first
/// read answers `log`, and every later read answers `log_after`.
///
/// It is how a test proves a `conversation` frame asks the pane's own surface
/// for a read rather than assembling an exchange from a frame: the exchange the
/// frame never described is the daemon's own row, and it arrives only through a
/// read.
pub fn talk_growing(
    list: &str,
    detail: &str,
    events: &str,
    log: &str,
    log_after: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let log = log.to_string();
    let log_after = log_after.to_string();
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = std::sync::Arc::clone(&reads);
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/conversations") => {
                if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    log.as_str()
                } else {
                    log_after.as_str()
                }
            }
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A whole stub daemon with the register but **no social layer**: the register's
/// paths answer `list`/`detail` and the machine answers [`MACHINE`], while every
/// social path is the 404 an older daemon serves. It is how a test proves a pane
/// states the absence of a layer rather than drawing an empty one.
pub fn social_unserved(list: &str, detail: &str) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    spawn(Box::new(move |request, _| {
        match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/machine") => Some(response(200, MACHINE)),
            ("GET", "/v1/runs") => Some(response(200, list.as_str())),
            ("GET", target) if target.starts_with("/v1/conversations") => {
                Some(response(404, r#"{"error":"no such endpoint"}"#))
            }
            ("GET", target) if target.starts_with("/v1/agents/") => {
                Some(response(404, r#"{"error":"no such endpoint"}"#))
            }
            ("GET", target) if target.starts_with("/v1/board") => {
                Some(response(404, r#"{"error":"no such endpoint"}"#))
            }
            _ => Some(response(200, detail.as_str())),
        }
    }))
}

/// A thread body as `GET /v1/conversations/{a}/{b}` sends it.
pub const THREAD: &str = r#"{"a":100,"b":101,"active":true,"active_secs":120,"messages":[
    {"id":"c1","from_cid":100,"to_cid":101,"from_role":"builder","to_role":"reviewer",
     "text":"can you review it?","created_at":1791675000000,"delivered":true,
     "delivery":null}
    ],"total":1,"limit":256,"truncated":false}"#;

/// A whole stub daemon for the floor and the board: the register, the machine,
/// an event stream, and the social surfaces.
///
/// `GET /v1/board` answers `posts`, `GET /v1/conversations` answers
/// `conversations`, `GET /v1/agents/{cid}/contacts` answers `contacts`, and
/// `GET /v1/conversations/{a}/{b}` answers `thread`. The two reads the
/// floor's own `Client::floor` makes — conversations then board — are the
/// daemon's own two GETs.
pub fn social_live(
    list: &str,
    detail: &str,
    events: &str,
    posts: &str,
    conversations: &str,
    contacts: &str,
    thread: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let posts = posts.to_string();
    let conversations = conversations.to_string();
    let contacts = contacts.to_string();
    let thread = thread.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/board") => posts.as_str(),
            ("GET", "/v1/conversations") => conversations.as_str(),
            ("GET", target) if target.starts_with("/v1/agents/") => contacts.as_str(),
            ("GET", target) if target.starts_with("/v1/conversations/") => thread.as_str(),
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A whole stub daemon whose board grows between reads: the first read answers
/// `posts`, and every later read answers `posts_after`.
///
/// It is how a test proves a `board` frame asks the pane's own surface for a
/// read rather than assembling a post from a frame: the post the frame carried
/// is the daemon's, but the unread counts a read moves arrive only through a
/// read.
pub fn social_growing(
    list: &str,
    detail: &str,
    events: &str,
    posts: &str,
    posts_after: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let posts = posts.to_string();
    let posts_after = posts_after.to_string();
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/board") => {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    posts.as_str()
                } else {
                    posts_after.as_str()
                }
            }
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A motions body as `GET /v1/motions` sends it, byte for byte the shape
/// the live plaza answered with: a priority carried 4–0, a law proposal
/// carried 4–0 and waiting for the owner, and a priority tied 2–2 and sent to
/// the owner. No motion names an amount, so no row carries the key.
pub const MOTIONS: &str = r#"{"motions":[
    {"id":"m3","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise t2 The gable dispute needs a ruling this week",
     "needs_owner":false,"state":"carried",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706571791},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706586797},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706601806},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706616816}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791706568274,"updated_at":1791706616816},
    {"id":"m2","proposer_cid":107,"proposer_role":"legal-agents-counsel",
     "kind":"law_proposal","text":"amend S-001 so builders may read the review record",
     "needs_owner":true,"state":"awaiting_owner",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record supports it","at":1791705629367},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record supports it","at":1791705629458},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record supports it","at":1791705629412},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record supports it","at":1791705629506}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791705629200,"updated_at":1791705629506},
    {"id":"m1","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise the east wing over new towns this week",
     "needs_owner":false,"state":"tie_to_owner",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the wing is the season's work","at":1791705628987},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record on the east wing","at":1791705623911},
       {"seat":"incident","agent_cid":106,"vote":"no",
        "reason":"the record on the east wing","at":1791705623782},
       {"seat":"mayor-office","agent_cid":114,"vote":"no",
        "reason":"the record on the east wing","at":1791705624070}
     ],
     "tally":{"yes":2,"no":2,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791705623593,"updated_at":1791705628987}
    ],"total":3,"limit":256,"truncated":false}"#;

/// The council of [`MOTIONS`] once the owner's hand is on it: the same 4–0
/// carry, but the gated law proposal ratified and the 2–2 tie vetoed — the
/// decided shape the daemon reports, with the owner's word, note and stamp.
pub const MOTIONS_DECIDED: &str = r#"{"motions":[
    {"id":"m3","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise t2 The gable dispute needs a ruling this week",
     "needs_owner":false,"state":"carried",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706571791},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706586797},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706601806},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706616816}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791706568274,"updated_at":1791706616816},
    {"id":"m2","proposer_cid":107,"proposer_role":"legal-agents-counsel",
     "kind":"law_proposal","text":"amend S-001 so builders may read the review record",
     "needs_owner":true,"state":"ratified",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record supports it","at":1791705629367},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record supports it","at":1791705629458},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record supports it","at":1791705629412},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record supports it","at":1791705629506}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":{"approve":true,"note":null,"at":1791706000000},
     "created_at":1791705629200,"updated_at":1791706000000},
    {"id":"m1","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise the east wing over new towns this week",
     "needs_owner":false,"state":"vetoed",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the wing is the season's work","at":1791705628987},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record on the east wing","at":1791705623911},
       {"seat":"incident","agent_cid":106,"vote":"no",
        "reason":"the record on the east wing","at":1791705623782},
       {"seat":"mayor-office","agent_cid":114,"vote":"no",
        "reason":"the record on the east wing","at":1791705624070}
     ],
     "tally":{"yes":2,"no":2,"abstain":0,"voted":4,"seats":4},
     "owner":{"approve":false,"note":"not this season","at":1791706100000},
     "created_at":1791705623593,"updated_at":1791706100000}
    ],"total":3,"limit":256,"truncated":false}"#;

/// The `200` a `POST /v1/motions/m1/decide` answers with when the owner
/// ratifies the tie: the motion, its `owner` set and its state `ratified`.
pub const DECIDED_RATIFIED: &str = r#"{"id":"m1","proposer_cid":105,"proposer_role":"finance",
    "kind":"priority","text":"prioritise the east wing over new towns this week",
    "needs_owner":false,"state":"ratified",
    "votes":[
      {"seat":"finance","agent_cid":105,"vote":"yes",
       "reason":"the wing is the season's work","at":1791705628987},
      {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
       "reason":"the record on the east wing","at":1791705623911},
      {"seat":"incident","agent_cid":106,"vote":"no",
       "reason":"the record on the east wing","at":1791705623782},
      {"seat":"mayor-office","agent_cid":114,"vote":"no",
       "reason":"the record on the east wing","at":1791705624070}
    ],
    "tally":{"yes":2,"no":2,"abstain":0,"voted":4,"seats":4},
    "owner":{"approve":true,"note":"carry it","at":1791706100000},
    "created_at":1791705623593,"updated_at":1791706100000}"#;

/// The council of [`MOTIONS_DECIDED`] with `m1` ratified instead of vetoed:
/// what the pane reads after the owner ratifies the tie.
pub const MOTIONS_RATIFIED: &str = r#"{"motions":[
    {"id":"m3","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise t2 The gable dispute needs a ruling this week",
     "needs_owner":false,"state":"carried",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706571791},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706586797},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706601806},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706616816}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791706568274,"updated_at":1791706616816},
    {"id":"m2","proposer_cid":107,"proposer_role":"legal-agents-counsel",
     "kind":"law_proposal","text":"amend S-001 so builders may read the review record",
     "needs_owner":true,"state":"awaiting_owner",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record supports it","at":1791705629367},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record supports it","at":1791705629458},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record supports it","at":1791705629412},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record supports it","at":1791705629506}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791705629200,"updated_at":1791705629506},
    {"id":"m1","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise the east wing over new towns this week",
     "needs_owner":false,"state":"ratified",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the wing is the season's work","at":1791705628987},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record on the east wing","at":1791705623911},
       {"seat":"incident","agent_cid":106,"vote":"no",
        "reason":"the record on the east wing","at":1791705623782},
       {"seat":"mayor-office","agent_cid":114,"vote":"no",
        "reason":"the record on the east wing","at":1791705624070}
     ],
     "tally":{"yes":2,"no":2,"abstain":0,"voted":4,"seats":4},
     "owner":{"approve":true,"note":"carry it","at":1791706100000},
     "created_at":1791705623593,"updated_at":1791706100000}
    ],"total":3,"limit":256,"truncated":false}"#;

/// A single-motion body as `GET /v1/motions/m1` sends it: the tie of
/// [`MOTIONS`], read on its own.
pub const MOTION_M1: &str = r#"{"id":"m1","proposer_cid":105,"proposer_role":"finance",
    "kind":"priority","text":"prioritise the east wing over new towns this week",
    "needs_owner":false,"state":"tie_to_owner",
    "votes":[
      {"seat":"finance","agent_cid":105,"vote":"yes",
       "reason":"the wing is the season's work","at":1791705628987},
      {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
       "reason":"the record on the east wing","at":1791705623911},
      {"seat":"incident","agent_cid":106,"vote":"no",
       "reason":"the record on the east wing","at":1791705623782},
      {"seat":"mayor-office","agent_cid":114,"vote":"no",
       "reason":"the record on the east wing","at":1791705624070}
    ],
    "tally":{"yes":2,"no":2,"abstain":0,"voted":4,"seats":4},
    "owner":null,
    "created_at":1791705623593,"updated_at":1791705628987}"#;

/// The council of [`MOTIONS`] once one more motion was proposed and is still
/// taking votes: what a later read answers with, and what a `motion` frame's
/// read brings in. The new row carries a text and a vote a frame cannot — a
/// console that draws it drew it from a read.
pub const MOTIONS_AFTER: &str = r#"{"motions":[
    {"id":"m4","proposer_cid":107,"proposer_role":"legal-agents-counsel",
     "kind":"other","text":"open a second reading on the annex",
     "needs_owner":false,"state":"open",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the annex is on the record","at":1791709000000}
     ],
     "tally":{"yes":1,"no":0,"abstain":0,"voted":1,"seats":4},
     "owner":null,
     "created_at":1791708990000,"updated_at":1791709000000},
    {"id":"m3","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise t2 The gable dispute needs a ruling this week",
     "needs_owner":false,"state":"carried",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706571791},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706586797},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706601806},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record: priority names open task t2","at":1791706616816}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791706568274,"updated_at":1791706616816},
    {"id":"m2","proposer_cid":107,"proposer_role":"legal-agents-counsel",
     "kind":"law_proposal","text":"amend S-001 so builders may read the review record",
     "needs_owner":true,"state":"awaiting_owner",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the record supports it","at":1791705629367},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record supports it","at":1791705629458},
       {"seat":"incident","agent_cid":106,"vote":"yes",
        "reason":"the record supports it","at":1791705629412},
       {"seat":"mayor-office","agent_cid":114,"vote":"yes",
        "reason":"the record supports it","at":1791705629506}
     ],
     "tally":{"yes":4,"no":0,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791705629200,"updated_at":1791705629506},
    {"id":"m1","proposer_cid":105,"proposer_role":"finance","kind":"priority",
     "text":"prioritise the east wing over new towns this week",
     "needs_owner":false,"state":"tie_to_owner",
     "votes":[
       {"seat":"finance","agent_cid":105,"vote":"yes",
        "reason":"the wing is the season's work","at":1791705628987},
       {"seat":"legal-agents-counsel","agent_cid":107,"vote":"yes",
        "reason":"the record on the east wing","at":1791705623911},
       {"seat":"incident","agent_cid":106,"vote":"no",
        "reason":"the record on the east wing","at":1791705623782},
       {"seat":"mayor-office","agent_cid":114,"vote":"no",
        "reason":"the record on the east wing","at":1791705624070}
     ],
     "tally":{"yes":2,"no":2,"abstain":0,"voted":4,"seats":4},
     "owner":null,
     "created_at":1791705623593,"updated_at":1791705628987}
    ],"total":4,"limit":256,"truncated":false}"#;

/// A whole stub daemon for the council pane: the register, the machine, an
/// event stream, and the council — `GET /v1/motions` answers `motions`,
/// `GET /v1/motions/{id}` answers `motion`, and every other read answers
/// `detail`.
pub fn council_live(
    list: &str,
    detail: &str,
    events: &str,
    motions: &str,
    motion: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let motions = motions.to_string();
    let motion = motion.to_string();
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/motions") => motions.as_str(),
            ("GET", target) if target.starts_with("/v1/motions/") => motion.as_str(),
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A whole stub daemon for the council pane that also holds the owner's hand:
/// `GET /v1/motions` answers `motions` until the decision lands and
/// `motions_after` from then on, `POST /v1/motions/{id}/decide` answers
/// `decided`, `GET /v1/motions/{id}` answers `motion`, and every other read
/// answers `detail`.
///
/// It is how a test proves the pane's own flow: a commit posts the decision,
/// and the pane reads its own surface again — the row the sheet then draws is
/// the daemon's, with the owner's line on it.
pub fn council_deciding(
    list: &str,
    detail: &str,
    events: &str,
    motions: &str,
    motion: &str,
    motions_after: &str,
    decided: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let motions = motions.to_string();
    let motion = motion.to_string();
    let motions_after = motions_after.to_string();
    let decided = decided.to_string();
    let decided_yet = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&decided_yet);
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("POST", target) if target.ends_with("/decide") => {
                flag.store(true, Ordering::SeqCst);
                decided.as_str()
            }
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/motions") => {
                if flag.load(Ordering::SeqCst) {
                    motions_after.as_str()
                } else {
                    motions.as_str()
                }
            }
            ("GET", target) if target.starts_with("/v1/motions/") => motion.as_str(),
            _ => detail.as_str(),
        };
        Some(response(200, body))
    }))
}

/// A whole stub daemon whose council grows between reads: the first read
/// answers `motions`, and every later read answers `motions_after` — the
/// daemon's own answer once one more motion was proposed.
///
/// It is how a test proves a `motion` frame asks the pane's own surface for a
/// read rather than assembling a motion from the frame: the motion the frame
/// never described is the daemon's own row, and it arrives only through a read.
pub fn council_growing(
    list: &str,
    detail: &str,
    events: &str,
    motions: &str,
    motions_after: &str,
) -> Stub {
    let list = list.to_string();
    let detail = detail.to_string();
    let events = stream_head(events);
    let motions = motions.to_string();
    let motions_after = motions_after.to_string();
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    spawn(Box::new(move |request, _| {
        let body = match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/v1/events") => return Some(events.clone()),
            ("GET", "/v1/machine") => MACHINE,
            ("GET", "/v1/runs") => list.as_str(),
            ("GET", "/v1/motions") => {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    motions.as_str()
                } else {
                    motions_after.as_str()
                }
            }
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
