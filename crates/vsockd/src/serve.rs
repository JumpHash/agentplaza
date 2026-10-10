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

use crate::line::LineTransport;
use crate::proto::{GuestMessage, HostMessage, WELCOME_ECHO};
use crate::vsock::{CID_ANY, HOST_CID};

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
/// `peer_cid` is the CID the accepted connection reported; the guest reports
/// its own CID again in `hello`, and the two are logged together so a
/// disagreement is visible rather than assumed away.
pub fn handle_connection<T: LineTransport>(
    transport: &mut T,
    peer_cid: u32,
    log: &dyn SessionLog,
) -> Result<SessionSummary, HostError> {
    let mut guest: Option<(u32, u32)> = None; // (cid, vm) once `hello` arrived
    let mut calls = 0u64;

    loop {
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
        log.line(&format!("{} {inbound}", inbound_prefix(guest, peer_cid)));

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
                // address the kernel reported for the accepted connection:
                // that address is the CID the packets really carried, so a
                // disagreement means the guest is not who it says it is, and
                // its session is dropped rather than served under a name it
                // has not earned.
                if peer_cid_is_usable(peer_cid) {
                    if guest_cid != peer_cid {
                        return Err(HostError::Protocol(format!(
                            "hello claims guest_cid {guest_cid}, but the connection was accepted from cid {peer_cid}"
                        )));
                    }
                } else {
                    log.line(&format!(
                        "vsockd: the transport reported no peer CID for this connection (accepted as {peer_cid}); \
                         the hello's guest_cid {guest_cid} cannot be checked against it"
                    ));
                }
                guest = Some((guest_cid, vm));
                log.line(&format!(
                    "vsockd: guest {guest_cid} vm {vm} says hello (accepted from cid {peer_cid})"
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
                calls += 1;
                let (ok, output) = run_tool(&name, &args);
                send(
                    transport,
                    guest,
                    &HostMessage::ToolResult { call, ok, output },
                    log,
                )?;
                log.line(&format!(
                    "vsockd: answered call {call} ({name}) for guest {cid} vm {vm}"
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
        None if peer_cid_is_usable(peer_cid) => format!("[guest {peer_cid} vm? ->]"),
        None => "[guest cid? vm? ->]".to_string(),
    }
}

/// Whether a CID the kernel reported for an accepted connection can be checked
/// against: 0/1/2 are the vsock stack's own (hypervisor, local, host) and
/// [`CID_ANY`] means the transport reported no address at all.
fn peer_cid_is_usable(peer_cid: u32) -> bool {
    peer_cid > HOST_CID && peer_cid != CID_ANY
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

        let summary = handle_connection(&mut host, 3, &log).expect("the session completes");
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
        let summary = handle_connection(&mut host, 3, &log).expect("the session completes");
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
        let summary = handle_connection(&mut host, 4, &log).expect("the session completes");
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
        let err = handle_connection(&mut host, 3, &log).expect_err("the session is dropped");
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
        let err = handle_connection(&mut host, 3, &log).expect_err("the session is dropped");
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
        let err = handle_connection(&mut host, 3, &log).expect_err("the session is dropped");
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
        let err = handle_connection(&mut host, 3, &log).expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail)
                if detail.contains("claims guest_cid 4") && detail.contains("accepted from cid 3")),
            "got {err:?}"
        );
        let log = log.text();
        assert!(log.contains("[guest 3 vm? ->]"), "log: {log}");
    }

    #[test]
    fn a_transport_that_reports_no_peer_cid_is_not_a_mismatch() {
        // `accept` on a transport that does not fill the peer address answers
        // VMADDR_CID_ANY; there is then nothing to check the claim against, and
        // the session is served (with a note) rather than failed on a
        // comparison that cannot be made.
        let (mut guest, mut host) = pair();
        let log = Recording::default();
        let script = thread::spawn(move || {
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":7}"#)
                .expect("hello sends");
            let _welcome = next(&mut guest);
            guest
                .send_line(r#"{"type":"bye","calls":0}"#)
                .expect("bye sends");
        });
        let summary = handle_connection(&mut host, CID_ANY, &log).expect("the session completes");
        assert_eq!(summary.guest_cid, 7);
        assert_eq!(summary.peer_cid, CID_ANY);
        script.join().expect("the scripted guest finished");
        assert!(
            log.text().contains("cannot be checked against it"),
            "log: {}",
            log.text()
        );
    }

    #[test]
    fn a_guest_that_never_greets_is_reported() {
        let (guest, mut host) = pair();
        drop(guest);
        let log = Recording::default();
        let err = handle_connection(&mut host, 3, &log).expect_err("the session is dropped");
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
}
