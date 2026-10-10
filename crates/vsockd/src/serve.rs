//! The orchestrator side of one guest session.
//!
//! The host is the decision-maker: it accepts the handshake, runs the tools
//! the guest asks for, and answers each `tool_call` with exactly one
//! `tool_result`. A tool that is asked something it cannot do answers
//! `ok: false` with the reason — a refused call is data for the guest, not a
//! broken connection. A line that is not a valid guest message *is* a broken
//! connection: it is logged and the session is dropped, because framing this
//! side does not understand cannot be answered safely.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::time::Instant;

use crate::line::LineTransport;
use crate::proto::{GuestMessage, HostMessage, WELCOME_ECHO};

/// Where the session's log lines go.
pub trait SessionLog {
    /// Writes one line.
    fn line(&self, text: &str);
}

/// The [`SessionLog`] that writes to this process's standard output, one line
/// at a time so concurrent guest sessions interleave whole lines.
pub struct StdoutLog;

impl SessionLog for StdoutLog {
    fn line(&self, text: &str) {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{text}");
    }
}

/// What one completed session did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSummary {
    /// The CID the accepted connection reported.
    pub peer_cid: u32,
    /// The CID the guest's `hello` reported.
    pub guest_cid: u32,
    /// The VM index the guest's `hello` reported.
    pub vm: u32,
    /// How many tool calls were answered.
    pub calls: u64,
    /// How many tool calls the guest's `bye` counted.
    pub guest_calls: u64,
}

/// The bounds one guest session is held to, enforced in
/// [`handle_connection`].
#[derive(Debug, Clone, Copy)]
pub struct SessionLimits {
    /// The most tool calls this session answers before it is dropped.
    pub max_calls: u64,
    /// The moment the session may not run past; `None` means no deadline.
    pub deadline: Option<Instant>,
}

/// Why a session ended without a `bye`.
#[derive(Debug)]
pub enum HostError {
    /// The connection failed: {0}
    Io(io::Error),
    /// The guest broke the protocol: {0}
    Protocol(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "connection failed: {err}"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<io::Error> for HostError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Runs one guest session to its `bye`, answering every tool call.
///
/// `peer_cid` is the guest CID this session was accepted for — the one the
/// listener's socket path encodes (`{uds_dir}/vm{cid}_{port}`). The guest
/// reports its own CID again in `hello`, and the two are checked against each
/// other: identity is by configuration, because the UDS muxer carries no peer
/// address on the wire. `limits` bounds the session: a tool-call budget the
/// host will not exceed, and a deadline the session may not run past.
pub fn handle_connection<T: LineTransport>(
    transport: &mut T,
    peer_cid: u32,
    limits: SessionLimits,
    log: &dyn SessionLog,
) -> Result<SessionSummary, HostError> {
    let mut guest: Option<(u32, u32)> = None; // (cid, vm) once `hello` arrived
    let mut calls = 0u64;

    loop {
        if limits.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(HostError::Protocol("session deadline exceeded".to_string()));
        }
        let inbound = match transport.recv_line() {
            Ok(Some(line)) => line,
            Ok(None) if guest.is_some() => {
                return Err(HostError::Protocol(
                    "the guest closed before sending bye".to_string(),
                ));
            }
            Ok(None) => {
                return Err(HostError::Protocol(
                    "the guest closed before sending hello".to_string(),
                ));
            }
            Err(err) => return Err(HostError::from(err)),
        };
        log.line(&format!(
            "{} {}",
            inbound_prefix(guest, peer_cid),
            sanitize_for_log(&inbound)
        ));

        match GuestMessage::from_line(&inbound) {
            Ok(GuestMessage::Hello {
                vm,
                guest_cid,
            }) => {
                if let Some((known_cid, known_vm)) = guest {
                    return Err(HostError::Protocol(format!(
                        "a second hello from guest {guest_cid} vm {vm} (already greeted guest {known_cid} vm {known_vm})"
                    )));
                }
                // The guest's claim about its own CID is checked against the
                // CID the accepted socket path encodes. The muxer carries no
                // peer address, so this is identity by configuration: a
                // disagreement means the guest is not who it says it is, and
                // its session is dropped rather than served under a name it
                // has not earned.
                if guest_cid != peer_cid {
                    return Err(HostError::Protocol(format!(
                        "hello claims guest_cid {guest_cid}, but the connection was accepted on the socket for cid {peer_cid}"
                    )));
                }
                guest = Some((guest_cid, vm));
                log.line(&format!(
                    "vsockd: guest {guest_cid} vm {vm} says hello (accepted on the socket for cid {peer_cid})"
                ));
                send(
                    transport,
                    guest,
                    &HostMessage::Welcome {
                        echo: WELCOME_ECHO.to_string(),
                    },
                    log,
                )?;
            }
            Ok(GuestMessage::ToolCall { call, name, args }) => {
                let Some((cid, vm)) = guest else {
                    return Err(HostError::Protocol(format!(
                        "tool_call {call} ({name}) before hello"
                    )));
                };
                if calls >= limits.max_calls {
                    log.line(&format!(
                        "vsockd: refusing call {call} ({}) for guest {cid} vm {vm}: the \
                         session's {}-call budget is exhausted",
                        sanitize_for_log(&name),
                        limits.max_calls
                    ));
                    return Err(HostError::Protocol(format!(
                        "tool-call budget ({}) exceeded",
                        limits.max_calls
                    )));
                }
                calls += 1;
                let (ok, output) = run_tool(&name, &args);
                send(
                    transport,
                    guest,
                    &HostMessage::ToolResult { call, ok, output },
                    log,
                )?;
                log.line(&format!(
                    "vsockd: answered call {call} ({}) for guest {cid} vm {vm}",
                    sanitize_for_log(&name)
                ));
            }
            Ok(GuestMessage::Bye { calls: guest_calls }) => {
                let Some((cid, vm)) = guest else {
                    return Err(HostError::Protocol("bye before hello".to_string()));
                };
                if guest_calls != calls {
                    log.line(&format!(
                        "vsockd: guest {cid} vm {vm} counted {guest_calls} tool calls, this side counted {calls}"
                    ));
                }
                log.line(&format!(
                    "vsockd: guest {cid} vm {vm} said bye after {calls} tool calls"
                ));
                return Ok(SessionSummary {
                    peer_cid,
                    guest_cid: cid,
                    vm,
                    calls,
                    guest_calls,
                });
            }
            Err(err) => {
                return Err(HostError::Protocol(format!(
                    "unparsable guest line ({err}): {inbound:?}"
                )));
            }
        }
    }
}

/// Runs one tool, refusing (with a reason) anything it cannot do.
///
/// This is the deny-first path: the tool decides, and a refusal is a normal
/// `tool_result` with `ok: false`, never a dropped connection.
fn run_tool(name: &str, args: &serde_json::Value) -> (bool, String) {
    match name {
        "echo" => match args.get("text").and_then(serde_json::Value::as_str) {
            Some(text) => (true, text.to_string()),
            None => (
                false,
                r#"invalid arguments: echo needs a string "text""#.to_string(),
            ),
        },
        "uptime" => match host_uptime() {
            Ok(uptime) => (true, uptime),
            Err(err) => (false, format!("cannot read the host's /proc/uptime: {err}")),
        },
        other => (false, format!("unknown tool: {other}")),
    }
}

/// The host's uptime: the first two fields of `/proc/uptime`.
fn host_uptime() -> io::Result<String> {
    let text = fs::read_to_string("/proc/uptime")?;
    let fields: Vec<&str> = text.split_whitespace().take(2).collect();
    if fields.len() < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "/proc/uptime has fewer than two fields",
        ));
    }
    Ok(format!("{} {}", fields[0], fields[1]))
}

/// The prefix for a logged inbound line: the guest's identity once known.
fn inbound_prefix(guest: Option<(u32, u32)>, peer_cid: u32) -> String {
    match guest {
        Some((cid, vm)) => format!("[guest {cid} vm{vm} ->]"),
        None => format!("[guest {peer_cid} vm? ->]"),
    }
}

/// Renders a guest line for the operator log with every control character
/// replaced by a visible `\xNN` escape.
///
/// The framing strips the newline, but a crafted line can still carry a
/// carriage return (`\r`), an ANSI escape (`\x1b`) or any other control byte;
/// logged raw, those move the terminal's cursor or repaint the screen, letting
/// a guest forge or erase log lines. Replacing them makes them inert while
/// printable text — including the JSON quotes — is left alone.
fn sanitize_for_log(line: &str) -> String {
    let mut sanitized = String::with_capacity(line.len());
    for character in line.chars() {
        if character.is_control() {
            sanitized.push_str(&format!("\\x{:02x}", character as u32));
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

/// Sends one host line and logs it.
fn send<T: LineTransport>(
    transport: &mut T,
    guest: Option<(u32, u32)>,
    message: &HostMessage,
    log: &dyn SessionLog,
) -> Result<(), HostError> {
    let line = message
        .to_line()
        .map_err(|err| HostError::Protocol(format!("cannot encode a host line: {err}")))?;
    let (cid, vm) = guest.unwrap_or((0, 0));
    log.line(&format!("[<-] guest {cid} vm{vm} {line}"));
    transport.send_line(&line)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;

    /// A [`LineTransport`] over a channel, so a test can put a scripted guest
    /// on the other end without a socket.
    struct Channel {
        out: Sender<String>,
        input: Receiver<String>,
    }

    impl LineTransport for Channel {
        fn send_line(&mut self, line: &str) -> io::Result<()> {
            self.out
                .send(line.to_string())
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "peer gone"))
        }

        fn recv_line(&mut self) -> io::Result<Option<String>> {
            Ok(self.input.recv().ok())
        }
    }

    /// An in-process, full-duplex pair of transports.
    fn pair() -> (Channel, Channel) {
        let (guest_out, host_in) = channel();
        let (host_out, guest_in) = channel();
        (
            Channel {
                out: guest_out,
                input: guest_in,
            },
            Channel {
                out: host_out,
                input: host_in,
            },
        )
    }

    /// A [`SessionLog`] that keeps every line, for assertions.
    #[derive(Default)]
    struct Recording(Mutex<Vec<String>>);

    impl Recording {
        fn text(&self) -> String {
            self.0
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .join("\n")
        }
    }

    impl SessionLog for Recording {
        fn line(&self, text: &str) {
            self.0
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(text.to_string());
        }
    }

    /// Receives one line, expecting it to be there.
    fn next(guest: &mut Channel) -> String {
        guest
            .recv_line()
            .expect("the host sent a line")
            .expect("the connection is open")
    }

    /// Unbounded session limits, for the tests that exercise the protocol
    /// rather than the bounds.
    fn limits() -> SessionLimits {
        SessionLimits {
            max_calls: u64::MAX,
            deadline: None,
        }
    }

    #[test]
    fn the_frozen_session_round_trips_against_a_scripted_guest() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();

        let script = thread::spawn(move || {
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("hello sends");
            assert_eq!(
                next(&mut guest),
                r#"{"type":"welcome","echo":"plaza-orchestrator"}"#
            );

            guest
                .send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza-guest-3-hello"}}"#)
                .expect("echo call sends");
            assert_eq!(
                next(&mut guest),
                r#"{"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}"#
            );

            guest
                .send_line(r#"{"type":"tool_call","call":2,"name":"uptime","args":{}}"#)
                .expect("uptime call sends");
            let uptime = next(&mut guest);
            let uptime: serde_json::Value =
                serde_json::from_str(&uptime).expect("the result is JSON");
            assert_eq!(uptime["ok"], serde_json::json!(true));
            // The host's own /proc/uptime first two fields.
            assert_eq!(
                uptime["output"].as_str().expect("output is a string").split_whitespace().count(),
                2
            );

            guest
                .send_line(r#"{"type":"bye","calls":2}"#)
                .expect("bye sends");
        });

        let summary = handle_connection(&mut host, 3, limits(), &log).expect("the session completes");
        assert_eq!(
            summary,
            SessionSummary {
                peer_cid: 3,
                guest_cid: 3,
                vm: 0,
                calls: 2,
                guest_calls: 2
            }
        );
        script.join().expect("the scripted guest finished");

        let log = log.text();
        assert!(log.contains("[guest 3 vm? ->] {\"type\":\"hello\""), "log: {log}");
        assert!(log.contains("[guest 3 vm0 ->] {\"type\":\"tool_call\",\"call\":1"), "log: {log}");
        assert!(log.contains("[<-] guest 3 vm0 {\"type\":\"welcome\""), "log: {log}");
        assert!(log.contains("said bye after 2 tool calls"), "log: {log}");
    }

    #[test]
    fn echo_refuses_arguments_it_cannot_echo() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        let script = thread::spawn(move || {
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("hello sends");
            let _welcome = next(&mut guest);
            guest
                .send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"nope":true}}"#)
                .expect("call sends");
            let refused = next(&mut guest);
            guest
                .send_line(r#"{"type":"bye","calls":1}"#)
                .expect("bye sends");
            refused
        });
        let summary = handle_connection(&mut host, 3, limits(), &log).expect("the session completes");
        let refused = script.join().expect("the scripted guest finished");
        let refused: serde_json::Value = serde_json::from_str(&refused).expect("the result is JSON");
        assert_eq!(refused["call"], serde_json::json!(1));
        assert_eq!(refused["ok"], serde_json::json!(false));
        assert!(
            refused["output"]
                .as_str()
                .expect("output is a string")
                .contains("needs a string"),
            "{refused}"
        );
        assert_eq!(summary.calls, 1);
    }

    #[test]
    fn an_unknown_tool_is_refused_not_answered_with_success() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        let script = thread::spawn(move || {
            guest
                .send_line(r#"{"type":"hello","vm":1,"guest_cid":4}"#)
                .expect("hello sends");
            let _welcome = next(&mut guest);
            guest
                .send_line(r#"{"type":"tool_call","call":1,"name":"rm_rf","args":{}}"#)
                .expect("call sends");
            let refused = next(&mut guest);
            guest
                .send_line(r#"{"type":"bye","calls":1}"#)
                .expect("bye sends");
            refused
        });
        let summary = handle_connection(&mut host, 4, limits(), &log).expect("the session completes");
        let refused = script.join().expect("the scripted guest finished");
        let refused: serde_json::Value = serde_json::from_str(&refused).expect("the result is JSON");
        assert_eq!(refused["ok"], serde_json::json!(false));
        assert_eq!(refused["output"], serde_json::json!("unknown tool: rm_rf"));
        assert_eq!(summary.calls, 1);
    }

    #[test]
    fn a_tool_call_before_hello_is_dropped() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        guest
            .send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"hi"}}"#)
            .expect("call sends");
        let err = handle_connection(&mut host, 3, limits(), &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("before hello")),
            "got {err:?}"
        );
    }

    #[test]
    fn a_malformed_line_drops_the_session() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        guest.send_line("not json").expect("garbage sends");
        let err = handle_connection(&mut host, 3, limits(), &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("unparsable guest line")),
            "got {err:?}"
        );
    }

    #[test]
    fn a_second_hello_is_dropped() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        let script = thread::spawn(move || {
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("hello sends");
            let _welcome = next(&mut guest);
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("second hello sends");
        });
        let err = handle_connection(&mut host, 3, limits(), &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("second hello")),
            "got {err:?}"
        );
        script.join().expect("the scripted guest finished");
    }

    #[test]
    fn a_hello_that_claims_the_wrong_cid_is_dropped() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        guest
            .send_line(r#"{"type":"hello","vm":0,"guest_cid":4}"#)
            .expect("hello sends");
        let err = handle_connection(&mut host, 3, limits(), &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail)
                if detail.contains("claims guest_cid 4") && detail.contains("accepted on the socket for cid 3")),
            "got {err:?}"
        );
        let log = log.text();
        assert!(log.contains("[guest 3 vm? ->]"), "log: {log}");
    }

    #[test]
    fn a_guest_that_never_greets_is_reported() {
        let (guest, mut host) = pair();
        drop(guest);
        let log = Recording::default();
        let err = handle_connection(&mut host, 3, limits(), &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("before sending hello")),
            "got {err:?}"
        );
    }

    #[test]
    fn host_uptime_is_the_first_two_fields_of_the_hosts_uptime() {
        let uptime = host_uptime().expect("/proc/uptime is readable");
        let fields: Vec<&str> = uptime.split_whitespace().collect();
        assert_eq!(fields.len(), 2);
        assert!(fields[0].parse::<f64>().is_ok(), "{uptime}");
        assert!(fields[1].parse::<f64>().is_ok(), "{uptime}");
    }

    /// A guest that keeps asking for tools past its budget is refused and
    /// dropped, so the host cannot be made to do unbounded work.
    #[test]
    fn a_tool_call_budget_is_enforced() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        let script = thread::spawn(move || {
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("hello sends");
            let _welcome = next(&mut guest);

            // Two calls fit a budget of two; the third is refused.
            guest
                .send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"a"}}"#)
                .expect("call 1 sends");
            let _ = next(&mut guest);
            guest
                .send_line(r#"{"type":"tool_call","call":2,"name":"echo","args":{"text":"b"}}"#)
                .expect("call 2 sends");
            let _ = next(&mut guest);
            guest
                .send_line(r#"{"type":"tool_call","call":3,"name":"echo","args":{"text":"c"}}"#)
                .expect("call 3 sends");
        });

        let limits = SessionLimits {
            max_calls: 2,
            deadline: None,
        };
        let err = handle_connection(&mut host, 3, limits, &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("tool-call budget (2) exceeded")),
            "got {err:?}"
        );
        script.join().expect("the scripted guest finished");
        assert!(
            log.text().contains("budget is exhausted"),
            "log must record the refusal: {}",
            log.text()
        );
    }

    /// A deadline in the past drops the session before any line is read, so a
    /// session cannot outlive the wall-clock bound the daemon gives it.
    #[test]
    fn a_past_deadline_drops_the_session() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        guest
            .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
            .expect("hello sends");
        let limits = SessionLimits {
            max_calls: u64::MAX,
            deadline: Some(Instant::now()),
        };
        let err = handle_connection(&mut host, 3, limits, &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("session deadline exceeded")),
            "got {err:?}"
        );
    }

    /// A guest line that carries control characters (a carriage return and an
    /// ANSI escape) is logged with those characters escaped, so it cannot
    /// forge or erase operator log lines.
    #[test]
    fn control_characters_in_a_guest_line_are_escaped_in_the_log() {
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        // Valid UTF-8, no newline, but an interior ESC sequence and carriage
        // return — exactly the bytes the framing leaves untouched.
        guest
            .send_line("not json\u{1b}[2K\rforged line")
            .expect("the crafted line sends");
        let _err =
            handle_connection(&mut host, 3, limits(), &log).expect_err("the malformed line drops the session");
        let logged = log.text();
        assert!(logged.contains("\\x1b"), "ESC must be escaped: {logged}");
        assert!(logged.contains("\\x0d"), "CR must be escaped: {logged}");
        assert!(!logged.contains('\u{1b}'), "raw ESC must not reach the log: {logged}");
        assert!(!logged.contains('\r'), "raw CR must not reach the log: {logged}");
    }
}
