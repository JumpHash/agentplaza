//! The session's two lanes: the reader that sorts the host's lines, and the operator worker that
//! answers the owner's tool calls.
//!
//! One reader thread owns the read half of the connection and parses every host line. A
//! `tool_call` (the owner's, sent by the hub's operator) goes to the operator worker, which runs
//! the tool and writes the `tool_result`. Every other message goes to the agent loop through an
//! [`Inbound`] channel. So an owner call is answered while the agent loop waits on a model or runs
//! a local tool. Every write goes through the one mutex in [`Shared`], so lines never interleave.
//!
//! A call that waited in the operator queue longer than [`OPERATOR_EXPIRY`] is answered `expired`
//! and not run. By then the hub's `tell` has timed out and reported a failure, so running it would
//! land a write the owner was told did not happen.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use apz_proto::line::{Lines, ReadTimeout, Recv};
use apz_proto::wire::{GuestMessage, HostMessage, MAX_TOOL_OUTPUT, cap_output};
use serde_json::Value;

use crate::session::{SessionError, escape_for_transcript, note, send};
use crate::vsock::VsockStream;

/// How long an owner call may wait in the operator queue before it expires. The hub's `tell`
/// gives up after vsockd's `MAX_TELL_TIMEOUT_MS` (5 s), so an answer later than that is wasted.
pub const OPERATOR_EXPIRY: Duration = Duration::from_secs(5);

/// How long the reader waits for a line before it checks whether the session is stopping.
const POLL: Duration = Duration::from_millis(50);

/// A connection the session can split: one handle reads, another writes.
pub trait Duplex: Read + Write + ReadTimeout + Send + Sized {
    /// Another handle to the same connection.
    fn duplicate(&self) -> io::Result<Self>;
}

impl Duplex for UnixStream {
    fn duplicate(&self) -> io::Result<Self> {
        self.try_clone()
    }
}

impl Duplex for VsockStream {
    fn duplicate(&self) -> io::Result<Self> {
        self.try_clone()
    }
}

/// What the reader hands the agent loop.
pub enum Inbound {
    /// A host message the agent loop acts on or a wait may be waiting for.
    Message(HostMessage),
    /// The host closed the connection.
    Closed,
    /// The connection failed.
    Failed(io::Error),
}

/// An owner tool call on its way to the operator worker.
pub struct OperatorCall {
    pub call: u64,
    pub name: String,
    pub args: Value,
    /// When the reader received the call; the expiry clock starts here.
    pub received: Instant,
}

/// The state both lanes share: the one writer, the transcript log, and the session's counters.
pub struct Shared<'log, S, W> {
    writer: Mutex<Lines<S>>,
    log: Mutex<&'log mut W>,
    /// Operator tool calls answered so far.
    calls: AtomicU64,
    /// Set when the agent loop is done; the reader then stops reading.
    stop: AtomicBool,
}

impl<'log, S: Write, W: Write> Shared<'log, S, W> {
    pub fn new(writer: Lines<S>, log: &'log mut W) -> Self {
        Self {
            writer: Mutex::new(writer),
            log: Mutex::new(log),
            calls: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        }
    }

    /// Sends one guest message and logs it. Both lanes write through here, one line at a time.
    pub fn say(&self, message: &GuestMessage) -> Result<(), SessionError> {
        let mut writer = locked(&self.writer);
        let mut log = locked(&self.log);
        send(&mut writer, &mut **log, message)
    }

    /// A transcript line.
    pub fn note(&self, text: &str) {
        note(&mut **locked(&self.log), text);
    }

    /// How many operator tool calls have been answered.
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    fn count_call(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    /// Asks the reader to stop after its current line.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The reader: owns the read half, parses each line and sorts it. Returns when the host closes,
/// the connection fails, the session stops, or the other side has gone away.
pub fn read_host<S: Duplex, W: Write>(
    lines: &mut Lines<S>,
    shared: &Shared<'_, S, W>,
    inbox: Sender<Inbound>,
    operator: Sender<OperatorCall>,
) {
    if let Err(err) = lines.get_ref().set_read_timeout(Some(POLL)) {
        let _ = inbox.send(Inbound::Failed(err));
        return;
    }
    while !shared.stopping() {
        let line = match lines.recv() {
            Ok(Recv::Line(line)) => line,
            Ok(Recv::Idle) => continue,
            Ok(Recv::Closed) => {
                let _ = inbox.send(Inbound::Closed);
                return;
            }
            Err(err) => {
                let _ = inbox.send(Inbound::Failed(err));
                return;
            }
        };
        shared.note(&format!("[vsock<-] {}", escape_for_transcript(&line)));
        match HostMessage::from_line(&line) {
            Ok(HostMessage::ToolCall { call, name, args }) => {
                let queued = OperatorCall {
                    call,
                    name,
                    args,
                    received: Instant::now(),
                };
                if operator.send(queued).is_err() {
                    return;
                }
            }
            Ok(message) => {
                if inbox.send(Inbound::Message(message)).is_err() {
                    return;
                }
            }
            Err(err) => shared.note(&format!(
                "agent-harness: ignoring malformed host line ({err})"
            )),
        }
    }
}

/// The operator worker: runs the owner's calls in the order they came and answers each. A call
/// that waited longer than `expiry` is answered `expired` without running. Returns once the reader
/// has gone and every queued call is answered.
pub fn serve_operators<S: Write, W: Write>(
    calls: Receiver<OperatorCall>,
    shared: &Shared<'_, S, W>,
    exec: impl Fn(&str, &Value) -> Result<String, String>,
    expiry: Duration,
) {
    for OperatorCall {
        call,
        name,
        args,
        received,
    } in calls
    {
        let waited = received.elapsed();
        let (ok, output) = if waited > expiry {
            shared.note(&format!(
                "agent-harness: tool call {call} expired after {} ms queued; not run",
                waited.as_millis()
            ));
            (false, "expired".to_string())
        } else {
            match exec(&name, &args) {
                Ok(output) => (true, output),
                Err(err) => {
                    let shown = escape_for_transcript(&name);
                    shared.note(&format!(
                        "agent-harness: tool {shown} call {call} refused: {err}"
                    ));
                    (false, err)
                }
            }
        };
        shared.count_call();
        let answer = GuestMessage::ToolResult {
            call,
            ok,
            output: cap_output(&output, MAX_TOOL_OUTPUT),
        };
        if let Err(err) = shared.say(&answer) {
            // The connection is broken; the reader sees that too and ends the session.
            shared.note(&format!(
                "agent-harness: tool call {call} answer not sent: {err}"
            ));
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::{next, pair};
    use serde_json::json;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn the_reader_sends_operator_calls_apart_from_the_agents_messages() {
        let (mut guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let (inbox_tx, inbox_rx) = mpsc::channel();
        let (operator_tx, operator_rx) = mpsc::channel();
        let hub = thread::spawn(move || {
            host.send(r#"{"type":"tool_call","call":5,"name":"echo","args":{"text":"hi"}}"#)
                .unwrap();
            host.send(r#"{"type":"stop","reason":"done"}"#).unwrap();
            host.send("not json").unwrap();
        });
        read_host(&mut guest, &shared, inbox_tx, operator_tx);
        hub.join().unwrap();
        drop(shared);

        let call = operator_rx
            .try_recv()
            .expect("the operator call was queued");
        assert_eq!(
            (call.call, call.name.as_str(), call.args),
            (5, "echo", json!({"text": "hi"}))
        );
        assert!(matches!(
            inbox_rx.try_recv(),
            Ok(Inbound::Message(HostMessage::Stop { .. }))
        ));
        assert!(matches!(inbox_rx.try_recv(), Ok(Inbound::Closed)));
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.contains("agent-harness: ignoring malformed host line"),
            "{log}"
        );
    }

    #[test]
    fn a_call_queued_past_the_expiry_is_answered_expired_and_not_run() {
        let (guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let (tx, rx) = mpsc::channel();
        // Both calls were received at the same moment; the first runs for longer than the expiry.
        let received = Instant::now();
        for (call, name) in [(1, "slow"), (2, "write_file")] {
            tx.send(OperatorCall {
                call,
                name: name.to_string(),
                args: json!({}),
                received,
            })
            .unwrap();
        }
        drop(tx);
        let ran = Mutex::new(Vec::new());
        serve_operators(
            rx,
            &shared,
            |name, _args| {
                ran.lock().unwrap().push(name.to_string());
                if name == "slow" {
                    thread::sleep(Duration::from_millis(500));
                }
                Ok(format!("ran {name}"))
            },
            Duration::from_millis(200),
        );
        assert_eq!(
            *ran.lock().unwrap(),
            ["slow"],
            "the expired call never runs"
        );
        let first: Value = serde_json::from_str(&next(&mut host)).unwrap();
        assert_eq!(
            first,
            json!({"type": "tool_result", "call": 1, "ok": true, "output": "ran slow"})
        );
        let second: Value = serde_json::from_str(&next(&mut host)).unwrap();
        assert_eq!(
            second,
            json!({"type": "tool_result", "call": 2, "ok": false, "output": "expired"})
        );
        assert_eq!(shared.calls(), 2);
    }

    #[test]
    fn concurrent_writers_never_interleave_their_lines() {
        let (guest, mut host) = pair();
        // The peer reads on its own thread: 1.2 MB of lines is more than the socket holds, so the
        // writers would block forever if nothing drained the other end while they wrote.
        let collector = thread::spawn(move || {
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..400 {
                let line: Value =
                    serde_json::from_str(&next(&mut host)).expect("every line is whole JSON");
                seen.insert(line["call"].as_u64().unwrap());
            }
            seen
        });
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let long = "y".repeat(3000);
        thread::scope(|scope| {
            for writer in 0..4u64 {
                let shared = &shared;
                let long = &long;
                scope.spawn(move || {
                    for seq in 0..100u64 {
                        let message = GuestMessage::ToolResult {
                            call: writer * 1000 + seq,
                            ok: true,
                            output: format!("{seq}-{long}"),
                        };
                        shared.say(&message).expect("the line is written");
                    }
                });
            }
        });
        drop(shared);
        assert_eq!(
            collector.join().unwrap().len(),
            400,
            "every call's line arrived whole, exactly once"
        );
    }
}
