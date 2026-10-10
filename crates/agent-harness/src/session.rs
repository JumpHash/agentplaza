//! The harness's session script: the frozen conversation, in order.
//!
//! The harness does not decide anything. It performs exactly this sequence —
//! `hello`, wait for `welcome`, `echo` tool call, `uptime` tool call, `bye` —
//! and every step is checked: a `welcome` whose echo is not
//! [`WELCOME_ECHO`], a `tool_result` for a call that was not made, a failed
//! tool, or a peer that closes or goes silent is a reported error, never a
//! quiet success.
//!
//! Lines that do not parse as a host message are logged and skipped, bounded
//! by [`MAX_SKIPPED_LINES`]: a peer that only ever sends garbage is a protocol
//! failure, not an infinite loop.

use std::fmt;
use std::io::{self, Write};

use serde_json::json;

use crate::line::LineTransport;
use crate::proto::{GuestMessage, HostMessage, WELCOME_ECHO};

/// How many unparsable or unexpected lines in a row one wait tolerates before
/// the session is failed.
pub const MAX_SKIPPED_LINES: usize = 16;

/// The `echo` tool's argument: the fixed string the guest asks the
/// orchestrator to say back.
pub fn echo_text(cid: u32) -> String {
    format!("plaza-guest-{cid}-hello")
}

/// Why a session did not complete.
#[derive(Debug)]
pub enum SessionError {
    /// The connection, or the transcript, failed: {0}
    Io(io::Error),
    /// A message could not be encoded: {0}
    Encode(serde_json::Error),
    /// The orchestrator stopped answering within the socket's read timeout.
    Timeout,
    /// The conversation left the frozen script: {0}
    Protocol(String),
    /// The orchestrator closed the connection before the session ended.
    HostClosed,
    /// Tool call {call} reported failure: {output}
    ToolFailed {
        /// The call that failed.
        call: u64,
        /// What the orchestrator reported.
        output: String,
    },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "connection failed: {err}"),
            Self::Encode(err) => write!(f, "cannot encode a protocol message: {err}"),
            Self::Timeout => write!(f, "timed out waiting for the orchestrator"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
            Self::HostClosed => write!(f, "the orchestrator closed the connection mid-session"),
            Self::ToolFailed { call, output } => {
                write!(f, "tool call {call} failed: {output}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

impl From<io::Error> for SessionError {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::WouldBlock {
            Self::Timeout
        } else {
            Self::Io(err)
        }
    }
}

/// Who this guest is: the CID the vsock device assigned it, and the VM index
/// the command line gave it.
///
/// The CID is not a configuration value — see [`crate::cmdline`] — it is
/// whatever the device says, which is what every packet this side sends
/// carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// The guest CID the vsock device assigned this VM.
    pub guest_cid: u32,
    /// This VM's index within the spawn, from `plaza.vm` (default 0).
    pub vm: u32,
}

/// Runs the frozen session over `transport`, writing the transcript (every
/// line in both directions, and this side's notes) to `log`.
///
/// Returns how many tool calls were made. The caller closes the transport;
/// the `bye` line is the last thing this function sends.
pub fn run<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
    identity: &Identity,
) -> Result<u64, SessionError> {
    send(
        transport,
        log,
        &GuestMessage::Hello {
            vm: identity.vm,
            guest_cid: identity.guest_cid,
        },
    )?;

    let echo = expect_welcome(transport, log)?;
    note(
        log,
        &format!("agent-harness: handshake complete (host echo {echo:?})"),
    );

    let mut calls = 0u64;

    // 1. `echo`: the orchestrator must say back exactly what was sent.
    calls += 1;
    let text = echo_text(identity.guest_cid);
    let output = tool_call(transport, log, calls, "echo", json!({ "text": text }))?;
    if output != text {
        return Err(SessionError::Protocol(format!(
            "echo call {calls} returned {output:?}, expected {text:?}"
        )));
    }
    note(log, &format!("agent-harness: echo call {calls} verified: {output:?}"));

    // 2. `uptime`: the orchestrator's first two /proc/uptime fields.
    calls += 1;
    let output = tool_call(transport, log, calls, "uptime", json!({}))?;
    if output.split_whitespace().count() < 2 {
        return Err(SessionError::Protocol(format!(
            "uptime call {calls} returned {output:?}, expected two fields"
        )));
    }
    note(log, &format!("agent-harness: uptime call {calls}: {output}"));

    send(transport, log, &GuestMessage::Bye { calls })?;
    note(log, &format!("agent-harness: session complete after {calls} tool calls"));
    Ok(calls)
}

/// Sends one protocol line and logs it.
fn send<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
    message: &GuestMessage,
) -> Result<(), SessionError> {
    let line = message.to_line().map_err(SessionError::Encode)?;
    note(log, &format!("[vsock->] {line}"));
    transport.send_line(&line)?;
    Ok(())
}

/// Receives one line, or reports why there was none.
fn receive<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
) -> Result<String, SessionError> {
    let line = match transport.recv_line() {
        Ok(Some(line)) => line,
        Ok(None) => return Err(SessionError::HostClosed),
        Err(err) => return Err(SessionError::from(err)),
    };
    note(log, &format!("[vsock<-] {line}"));
    Ok(line)
}

/// Waits for the `welcome` that answers our `hello`, skipping lines that are
/// neither a `welcome` nor parseable.
fn expect_welcome<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
) -> Result<String, SessionError> {
    let mut skipped = 0usize;
    loop {
        let line = receive(transport, log)?;
        match HostMessage::from_line(&line) {
            Ok(HostMessage::Welcome { echo }) if echo == WELCOME_ECHO => return Ok(echo),
            Ok(HostMessage::Welcome { echo }) => {
                return Err(SessionError::Protocol(format!(
                    "welcome echo {echo:?} is not {WELCOME_ECHO:?}"
                )));
            }
            Ok(other) => {
                skipped += 1;
                note(
                    log,
                    &format!("agent-harness: ignoring {} while waiting for welcome", kind_of(&other)),
                );
            }
            Err(err) => {
                skipped += 1;
                note(log, &format!("agent-harness: ignoring malformed host line ({err})"));
            }
        }
        if skipped > MAX_SKIPPED_LINES {
            return Err(SessionError::Protocol(format!(
                "no welcome within {skipped} lines"
            )));
        }
    }
}

/// Sends one `tool_call` and returns its `output`, failing if the
/// orchestrator reports `ok: false`.
fn tool_call<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
    call: u64,
    name: &str,
    args: serde_json::Value,
) -> Result<String, SessionError> {
    send(
        transport,
        log,
        &GuestMessage::ToolCall {
            call,
            name: name.to_string(),
            args,
        },
    )?;

    let mut skipped = 0usize;
    loop {
        let line = receive(transport, log)?;
        match HostMessage::from_line(&line) {
            Ok(HostMessage::ToolResult {
                call: answered,
                ok,
                output,
            }) if answered == call => {
                if !ok {
                    return Err(SessionError::ToolFailed { call, output });
                }
                return Ok(output);
            }
            Ok(other) => {
                skipped += 1;
                note(
                    log,
                    &format!(
                        "agent-harness: ignoring {} while waiting for tool_result {call}",
                        kind_of(&other)
                    ),
                );
            }
            Err(err) => {
                skipped += 1;
                note(log, &format!("agent-harness: ignoring malformed host line ({err})"));
            }
        }
        if skipped > MAX_SKIPPED_LINES {
            return Err(SessionError::Protocol(format!(
                "no tool_result for call {call} within {skipped} lines"
            )));
        }
    }
}

/// The message's protocol name, for log lines that name what was skipped.
fn kind_of(message: &HostMessage) -> &'static str {
    match message {
        HostMessage::Welcome { .. } => "welcome",
        HostMessage::ToolResult { .. } => "tool_result",
    }
}

/// Writes one transcript line.
///
/// The transcript is the serial console, which is how a human sees the loop;
/// it is best-effort (a console that has gone away must not fail the
/// protocol), so a write error is dropped rather than propagated.
fn note<W: Write>(log: &mut W, text: &str) {
    let _ = writeln!(log, "{text}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;

    /// A [`LineTransport`] over a channel, so a test can put a scripted
    /// orchestrator on the other end without a socket.
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

    fn identity() -> Identity {
        Identity {
            guest_cid: 3,
            vm: 0,
        }
    }

    /// Receives one line from the guest, expecting it to be there.
    fn next(host: &mut Channel) -> String {
        host.recv_line()
            .expect("the guest sent a line")
            .expect("the connection is open")
    }

    /// The scripted orchestrator: exactly the frozen host side.
    fn scripted_orchestrator(mut host: Channel) -> Vec<String> {
        let hello = next(&mut host);
        let welcome = r#"{"type":"welcome","echo":"plaza-orchestrator"}"#.to_string();
        host.send_line(&welcome).expect("welcome sends");

        let echo_call = next(&mut host);
        let echo_result =
            r#"{"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}"#
                .to_string();
        host.send_line(&echo_result).expect("echo result sends");

        let uptime_call = next(&mut host);
        let uptime_result =
            r#"{"type":"tool_result","call":2,"ok":true,"output":"12.34 56.78"}"#.to_string();
        host.send_line(&uptime_result).expect("uptime result sends");

        let bye = next(&mut host);
        vec![hello, echo_call, uptime_call, bye]
    }

    #[test]
    fn the_frozen_session_round_trips_against_a_scripted_orchestrator() {
        let (guest, host) = pair();
        let orchestrator = thread::spawn(move || scripted_orchestrator(host));

        let mut guest = guest;
        let mut log = Vec::new();
        let calls = run(&mut guest, &mut log, &identity()).expect("session completes");
        assert_eq!(calls, 2);

        let lines = orchestrator.join().expect("the orchestrator thread finished");
        assert_eq!(
            lines,
            vec![
                r#"{"type":"hello","vm":0,"guest_cid":3}"#.to_string(),
                r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza-guest-3-hello"}}"#
                    .to_string(),
                r#"{"type":"tool_call","call":2,"name":"uptime","args":{}}"#.to_string(),
                r#"{"type":"bye","calls":2}"#.to_string(),
            ]
        );

        // Ordering is part of the contract: every line is logged when it is
        // sent or received.
        let log = String::from_utf8(log).expect("the transcript is UTF-8");
        let positions: Vec<usize> = [
            "[vsock->] {\"type\":\"hello\"",
            "[vsock<-] {\"type\":\"welcome\"",
            "[vsock->] {\"type\":\"tool_call\",\"call\":1",
            "[vsock<-] {\"type\":\"tool_result\",\"call\":1",
            "[vsock->] {\"type\":\"tool_call\",\"call\":2",
            "[vsock<-] {\"type\":\"tool_result\",\"call\":2",
            "[vsock->] {\"type\":\"bye\"",
        ]
        .iter()
        .map(|needle| {
            log.find(needle)
                .unwrap_or_else(|| panic!("{needle} is in the transcript"))
        })
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "the transcript is in protocol order"
        );
    }

    #[test]
    fn a_replayed_welcome_and_a_stale_result_are_skipped_not_fatal() {
        let (guest, mut host) = pair();
        let orchestrator = thread::spawn(move || {
            let _hello = next(&mut host);
            // A duplicate of a handshake the guest never saw, and a result for
            // a call that was not made: both must be skipped.
            host.send_line(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .expect("welcome sends");
            host.send_line(r#"{"type":"tool_result","call":9,"ok":true,"output":"stale"}"#)
                .expect("stale result sends");
            host.send_line(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .expect("second welcome sends");

            let _echo = next(&mut host);
            host.send_line(r#"not json at all"#).expect("garbage sends");
            host.send_line(r#"{"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}"#)
                .expect("echo result sends");

            let _uptime = next(&mut host);
            host.send_line(r#"{"type":"tool_result","call":2,"ok":true,"output":"1.00 2.00"}"#)
                .expect("uptime result sends");

            let bye = next(&mut host);
            assert_eq!(bye, r#"{"type":"bye","calls":2}"#);
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let calls = run(&mut guest, &mut log, &identity()).expect("session completes");
        assert_eq!(calls, 2);
        orchestrator.join().expect("the orchestrator thread finished");
    }

    #[test]
    fn a_failed_tool_is_reported_with_its_call_and_output() {
        let (guest, mut host) = pair();
        let orchestrator = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .expect("welcome sends");
            let _echo = next(&mut host);
            host.send_line(r#"{"type":"tool_result","call":1,"ok":false,"output":"denied by policy"}"#)
                .expect("failure result sends");
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let err = run(&mut guest, &mut log, &identity()).expect_err("the session fails");
        match err {
            SessionError::ToolFailed { call, output } => {
                assert_eq!(call, 1);
                assert_eq!(output, "denied by policy");
            }
            other => panic!("expected ToolFailed, got {other:?}"),
        }
        orchestrator.join().expect("the orchestrator thread finished");
    }

    #[test]
    fn a_closed_connection_mid_session_is_reported() {
        let (guest, mut host) = pair();
        // The orchestrator takes the `hello` and then goes away without
        // answering it: the session must report that, not hang or guess.
        let orchestrator = thread::spawn(move || {
            let _hello = next(&mut host);
        });
        let mut guest = guest;
        let mut log = Vec::new();
        let err = run(&mut guest, &mut log, &identity()).expect_err("the session fails");
        assert!(matches!(err, SessionError::HostClosed), "got {err:?}");
        orchestrator.join().expect("the orchestrator thread finished");
    }

    #[test]
    fn a_wrong_welcome_echo_is_a_protocol_error() {
        let (guest, mut host) = pair();
        let orchestrator = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(r#"{"type":"welcome","echo":"someone-else"}"#)
                .expect("welcome sends");
        });
        let mut guest = guest;
        let mut log = Vec::new();
        let err = run(&mut guest, &mut log, &identity()).expect_err("the session fails");
        assert!(matches!(err, SessionError::Protocol(_)), "got {err:?}");
        orchestrator.join().expect("the orchestrator thread finished");
    }

    #[test]
    fn a_peer_that_only_sends_garbage_is_failed_not_looped_on() {
        let (guest, mut host) = pair();
        let count = MAX_SKIPPED_LINES + 2;
        let orchestrator = thread::spawn(move || {
            let _hello = next(&mut host);
            // The guest fails the session after MAX_SKIPPED_LINES garbage
            // lines and drops its end, so the last sends may fail; that is the
            // guest stopping, not a test failure.
            for _ in 0..count {
                let _ = host.send_line("garbage");
            }
        });
        let mut guest = guest;
        let mut log = Vec::new();
        let err = run(&mut guest, &mut log, &identity()).expect_err("the session fails");
        assert!(matches!(err, SessionError::Protocol(_)), "got {err:?}");
        orchestrator.join().expect("the orchestrator thread finished");
    }
}
