//! The v1 session: `hello` → `welcome` → `echo` → `uptime` → `bye`.
//!
//! Every step is checked: a wrong welcome echo, a failed tool, or a peer that
//! closes or goes silent is an error, never a quiet success. Lines that are
//! not the awaited answer are logged and skipped, at most
//! [`MAX_SKIPPED_LINES`] in a row.

use std::fmt;
use std::io::{self, Read, Write};

use apz_proto::line::{Lines, Recv};
use apz_proto::wire::{GuestMessage, HostMessage, WELCOME_ECHO};
use serde_json::json;

/// How many unexpected lines in a row one wait tolerates.
pub const MAX_SKIPPED_LINES: usize = 16;

/// The `echo` tool's argument.
pub fn echo_text(cid: u32) -> String {
    format!("plaza-guest-{cid}-hello")
}

/// Why a session did not complete.
#[derive(Debug)]
pub enum SessionError {
    Io(io::Error),
    Encode(serde_json::Error),
    /// The orchestrator said nothing within the socket's read timeout.
    Timeout,
    Protocol(String),
    HostClosed,
    ToolFailed {
        call: u64,
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
            Self::ToolFailed { call, output } => write!(f, "tool call {call} failed: {output}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<io::Error> for SessionError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Who this guest is: the CID its vsock device assigned it (never the command
/// line's), and its VM index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub guest_cid: u32,
    pub vm: u32,
}

/// Runs the v1 session, logging every line both ways to `log`. Returns how many
/// tool calls were made; `bye` is the last line sent.
pub fn run<S: Read + Write, W: Write>(
    lines: &mut Lines<S>,
    log: &mut W,
    identity: &Identity,
) -> Result<u64, SessionError> {
    let hello = GuestMessage::Hello {
        vm: identity.vm,
        guest_cid: identity.guest_cid,
        role: None,
        keepalive: None,
    };
    send(lines, log, &hello)?;
    let echo = await_host(lines, log, "welcome", |message| match message {
        HostMessage::Welcome { echo, .. } if echo == WELCOME_ECHO => Some(Ok(echo)),
        HostMessage::Welcome { echo, .. } => Some(Err(wrong_echo(&echo))),
        _ => None,
    })?;
    note(
        log,
        &format!("agent-harness: handshake complete (host echo {echo:?})"),
    );

    let text = echo_text(identity.guest_cid);
    let output = tool_call(lines, log, 1, "echo", json!({ "text": text }))?;
    if output != text {
        return Err(SessionError::Protocol(format!(
            "echo call 1 returned {output:?}, expected {text:?}"
        )));
    }
    note(
        log,
        &format!("agent-harness: echo call 1 verified: {output:?}"),
    );

    let output = tool_call(lines, log, 2, "uptime", json!({}))?;
    if output.split_whitespace().count() < 2 {
        return Err(SessionError::Protocol(format!(
            "uptime call 2 returned {output:?}, expected two fields"
        )));
    }
    note(
        log,
        &format!(
            "agent-harness: uptime call 2: {}",
            escape_for_transcript(&output)
        ),
    );

    send(lines, log, &GuestMessage::Bye { calls: 2 })?;
    note(log, "agent-harness: session complete after 2 tool calls");
    Ok(2)
}

/// Sends one `tool_call` and returns its output; `ok: false` is an error.
fn tool_call<S: Read + Write, W: Write>(
    lines: &mut Lines<S>,
    log: &mut W,
    call: u64,
    name: &str,
    args: serde_json::Value,
) -> Result<String, SessionError> {
    send(
        lines,
        log,
        &GuestMessage::ToolCall {
            call,
            name: name.to_string(),
            args,
        },
    )?;
    await_host(
        lines,
        log,
        &format!("tool_result {call}"),
        |message| match message {
            HostMessage::ToolResult {
                call: answered,
                ok: true,
                output,
            } if answered == call => Some(Ok(output)),
            HostMessage::ToolResult {
                call: answered,
                ok: false,
                output,
            } if answered == call => Some(Err(SessionError::ToolFailed { call, output })),
            _ => None,
        },
    )
}

/// Reads host lines until `pick` accepts one, skipping (and logging) the rest.
pub(crate) fn await_host<S: Read, W: Write, T>(
    lines: &mut Lines<S>,
    log: &mut W,
    awaited: &str,
    mut pick: impl FnMut(HostMessage) -> Option<Result<T, SessionError>>,
) -> Result<T, SessionError> {
    for _ in 0..=MAX_SKIPPED_LINES {
        let line = match lines.recv()? {
            Recv::Line(line) => line,
            Recv::Idle => return Err(SessionError::Timeout),
            Recv::Closed => return Err(SessionError::HostClosed),
        };
        note(log, &format!("[vsock<-] {}", escape_for_transcript(&line)));
        match HostMessage::from_line(&line) {
            Ok(message) => {
                let kind = kind_of(&message);
                match pick(message) {
                    Some(picked) => return picked,
                    None => note(
                        log,
                        &format!("agent-harness: ignoring {kind} while waiting for {awaited}"),
                    ),
                }
            }
            Err(err) => note(
                log,
                &format!("agent-harness: ignoring malformed host line ({err})"),
            ),
        }
    }
    Err(SessionError::Protocol(format!(
        "no {awaited} within {} lines",
        MAX_SKIPPED_LINES + 1
    )))
}

pub(crate) fn wrong_echo(echo: &str) -> SessionError {
    SessionError::Protocol(format!("welcome echo {echo:?} is not {WELCOME_ECHO:?}"))
}

/// Sends one line and logs it.
pub(crate) fn send<S: Write, W: Write>(
    lines: &mut Lines<S>,
    log: &mut W,
    message: &GuestMessage,
) -> Result<(), SessionError> {
    let line = message.to_line().map_err(SessionError::Encode)?;
    note(log, &format!("[vsock->] {line}"));
    lines.send(&line)?;
    Ok(())
}

/// The message's wire name, for log lines.
pub(crate) fn kind_of(message: &HostMessage) -> &'static str {
    match message {
        HostMessage::Welcome { .. } => "welcome",
        HostMessage::ToolResult { .. } => "tool_result",
        HostMessage::ToolCall { .. } => "tool_call",
        HostMessage::Stop { .. } => "stop",
        HostMessage::Work { .. } => "work",
        HostMessage::ModelResult { .. } => "model_result",
        HostMessage::ToolDecision { .. } => "tool_decision",
        HostMessage::Cancel { .. } => "cancel",
        HostMessage::Profile { .. } => "profile",
        HostMessage::Control { .. } => "control",
    }
}

/// Writes one transcript line (the serial console); best-effort.
pub(crate) fn note<W: Write>(log: &mut W, text: &str) {
    let _ = writeln!(log, "{text}");
}

/// Escapes control characters in host-supplied text so a peer cannot forge
/// transcript lines.
pub(crate) fn escape_for_transcript(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_control() {
            out.push_str(&format!("\\x{:02x}", u32::from(ch)));
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod testing {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use apz_proto::line::{Lines, Recv};

    /// A connected guest/host pair over a real socket.
    pub fn pair() -> (Lines<UnixStream>, Lines<UnixStream>) {
        let (guest, host) = UnixStream::pair().expect("a socket pair");
        host.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a read timeout");
        (Lines::new(guest), Lines::new(host))
    }

    /// The guest's next line.
    pub fn next(host: &mut Lines<UnixStream>) -> String {
        match host.recv().expect("the guest's line") {
            Recv::Line(line) => line,
            other => panic!("expected a line, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{next, pair};
    use super::*;
    use std::thread;

    const IDENTITY: Identity = Identity {
        guest_cid: 3,
        vm: 0,
    };

    #[test]
    fn the_v1_session_round_trips_and_skips_stray_lines() {
        let (mut guest, mut host) = pair();
        let orchestrator = thread::spawn(move || {
            let hello = next(&mut host);
            // A stale result and garbage before the welcome are skipped.
            host.send(r#"{"type":"tool_result","call":9,"ok":true,"output":"stale"}"#)
                .unwrap();
            host.send("not json").unwrap();
            host.send(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .unwrap();
            let echo = next(&mut host);
            host.send(
                r#"{"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}"#,
            )
            .unwrap();
            let uptime = next(&mut host);
            host.send(r#"{"type":"tool_result","call":2,"ok":true,"output":"12.34 56.78"}"#)
                .unwrap();
            vec![hello, echo, uptime, next(&mut host)]
        });
        let mut log = Vec::new();
        assert_eq!(
            run(&mut guest, &mut log, &IDENTITY).expect("the session completes"),
            2
        );
        assert_eq!(
            orchestrator.join().unwrap(),
            [
                r#"{"type":"hello","vm":0,"guest_cid":3}"#,
                r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza-guest-3-hello"}}"#,
                r#"{"type":"tool_call","call":2,"name":"uptime","args":{}}"#,
                r#"{"type":"bye","calls":2}"#,
            ]
        );
        let log = String::from_utf8(log).unwrap();
        let hello = log
            .find("[vsock->] {\"type\":\"hello\"")
            .expect("hello logged");
        let welcome = log
            .find("[vsock<-] {\"type\":\"welcome\"")
            .expect("welcome logged");
        assert!(hello < welcome, "the transcript is in protocol order");
    }

    #[test]
    fn a_failed_tool_is_reported_with_its_call_and_output() {
        let (mut guest, mut host) = pair();
        let orchestrator = thread::spawn(move || {
            next(&mut host);
            host.send(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .unwrap();
            next(&mut host);
            host.send(r#"{"type":"tool_result","call":1,"ok":false,"output":"denied by policy"}"#)
                .unwrap();
        });
        let err = run(&mut guest, &mut Vec::new(), &IDENTITY).expect_err("the session fails");
        assert!(
            matches!(&err, SessionError::ToolFailed { call: 1, output } if output == "denied by policy"),
            "{err:?}"
        );
        orchestrator.join().unwrap();
    }

    #[test]
    fn a_wrong_echo_a_close_and_a_garbage_flood_each_fail_the_session() {
        let (mut guest, mut host) = pair();
        let orchestrator = thread::spawn(move || {
            next(&mut host);
            host.send(r#"{"type":"welcome","echo":"someone-else"}"#)
                .unwrap();
        });
        let err = run(&mut guest, &mut Vec::new(), &IDENTITY).expect_err("a wrong echo");
        assert!(matches!(err, SessionError::Protocol(_)), "{err:?}");
        orchestrator.join().unwrap();

        let (mut guest, mut host) = pair();
        let orchestrator = thread::spawn(move || drop(next(&mut host)));
        let err = run(&mut guest, &mut Vec::new(), &IDENTITY).expect_err("a close");
        assert!(matches!(err, SessionError::HostClosed), "{err:?}");
        orchestrator.join().unwrap();

        let (mut guest, mut host) = pair();
        let orchestrator = thread::spawn(move || {
            next(&mut host);
            for _ in 0..=MAX_SKIPPED_LINES + 1 {
                let _ = host.send("garbage");
            }
        });
        let err = run(&mut guest, &mut Vec::new(), &IDENTITY).expect_err("a flood");
        assert!(matches!(err, SessionError::Protocol(_)), "{err:?}");
        orchestrator.join().unwrap();
    }

    #[test]
    fn host_control_characters_are_escaped_in_the_transcript() {
        assert_eq!(
            escape_for_transcript("ok\r\x1b[2Kforged"),
            "ok\\x0d\\x1b[2Kforged"
        );
    }
}
