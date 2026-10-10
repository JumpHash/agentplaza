//! The orchestrator side of one guest session.
//!
//! The host is the decision-maker: it accepts the handshake, runs the tools
//! the guest asks for, and answers each `tool_call` with exactly one
//! `tool_result`. A tool that is asked something it cannot do answers
//! `ok: false` with the reason — a refused call is data for the guest, not a
//! broken connection. A line that is not a valid guest message *is* a broken
//! connection: it is logged and the session is dropped, because framing this
//! side does not understand cannot be answered safely.
//!
//! Two session shapes live here, chosen by the `hello`:
//!
//! * **v1** — the frozen demo session ([`crate::proto`]): welcome, the guest's
//!   tool calls, `bye`. Served exactly as before.
//! * **v2, keepalive** — a long-lived agent. The hub dispatches `tool_call`s
//!   at will, the agent answers `tool_result`s and beats with a `heartbeat`
//!   every five seconds, and the session ends when the agent says `bye` (in
//!   answer to the hub's `stop`) or when the hub reaps it for going silent for
//!   [`SessionLimits::keepalive_idle`]. Keepalive sessions have no overall
//!   deadline: staying up is the point, and liveness is the heartbeat.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use crate::hub::Hub;
use crate::line::{Inbound, LineTransport};
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

/// What the session measures its deadlines against.
///
/// Production reads the process's real clock; a test injects a clock it steps
/// by hand, so the idle deadline is proven deterministically rather than by
/// waiting for real time to pass.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> Instant;
}

/// The process's clock: [`Instant::now`].
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
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
    /// How many tool calls were answered (v1) or served in either direction
    /// (keepalive).
    pub calls: u64,
    /// How many tool calls the guest's `bye` counted.
    pub guest_calls: u64,
    /// Whether this was a keepalive session. The hub logs a keepalive
    /// session's own end lines (`connected`, `heartbeat`, the clean end, the
    /// reap), so the caller must not also log v1's session line.
    pub keepalive: bool,
}

/// The bounds one guest session is held to, enforced in [`handle_session`].
#[derive(Debug, Clone, Copy)]
pub struct SessionLimits {
    /// The most tool calls this session serves before it is dropped. It
    /// bounds both the v1 guest-initiated calls and the hub's own dispatches.
    pub max_calls: u64,
    /// The moment a v1 session may not run past; `None` means no deadline.
    /// A keepalive session is bounded by its idle deadline instead, so this is
    /// not applied to it.
    pub deadline: Option<Instant>,
    /// The idle deadline a keepalive session is held to: no heartbeat for this
    /// long and the hub reaps the session. `None` refuses keepalive hellos —
    /// the daemon is configured to serve only the one-shot demo session.
    pub keepalive_idle: Option<Duration>,
    /// How long one read on a keepalive session waits before the session
    /// re-checks its idle deadline and its outbound work. A v1 session keeps
    /// the socket's own (longer) read timeout.
    pub keepalive_read_slice: Duration,
}

/// Why a session ended without a `bye`.
#[derive(Debug)]
pub enum HostError {
    /// The connection failed: {0}
    Io(io::Error),
    /// The guest broke the protocol: {0}
    Protocol(String),
    /// The hub reaped a keepalive session that stopped heartbeating. The hub
    /// has already logged the reap with its reason; there is nothing left to
    /// report about the session.
    Reaped,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "connection failed: {err}"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
            Self::Reaped => write!(f, "reaped: the keepalive agent stopped heartbeating"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<io::Error> for HostError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// The accepted `hello`: who the agent is, and whether it asked to stay.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Handshake {
    vm: u32,
    guest_cid: u32,
    role: Option<String>,
    keepalive: bool,
}

/// Runs one guest session, v1 or keepalive, to its `bye` (or its reap).
///
/// `peer_cid` is the guest CID this session was accepted for — the one the
/// listener's socket path encodes (`{uds_dir}/vm{cid}_{port}`). The guest
/// reports its own CID again in `hello`, and the two are checked against each
/// other: identity is by configuration, because the UDS muxer carries no peer
/// address on the wire. `limits` bounds the session; `hub` is where a
/// keepalive session registers; `clock` is what it measures its idle deadline
/// against.
pub fn handle_session<T: LineTransport>(
    transport: &mut T,
    peer_cid: u32,
    limits: SessionLimits,
    log: &dyn SessionLog,
    hub: &Hub,
    clock: &dyn Clock,
) -> Result<SessionSummary, HostError> {
    let hello = read_handshake(transport, peer_cid, log)?;
    if hello.keepalive {
        let Some(idle) = limits.keepalive_idle else {
            return Err(HostError::Protocol(
                "a keepalive hello on a daemon configured for one-shot sessions only".to_string(),
            ));
        };
        // The session now has to wake up on its own account, so its reads are
        // sliced; a v1 session keeps the socket's longer read timeout.
        transport.set_read_slice(limits.keepalive_read_slice)?;
        run_keepalive(transport, &hello, peer_cid, limits, idle, log, hub, clock)
    } else {
        send(
            transport,
            hello.guest_cid,
            hello.vm,
            &HostMessage::Welcome {
                echo: WELCOME_ECHO.to_string(),
                role: None,
                keepalive: None,
            },
            log,
        )?;
        run_v1_body(transport, &hello, limits, log)
    }
}

/// Reads the first line of the session — the `hello` — and checks it.
///
/// Everything before a valid `hello` is a protocol violation, and a `hello`
/// that claims a CID other than the one the connection was accepted for is
/// dropped: identity is by configuration. The guest's line is logged with the
/// accepted-CID prefix, and the identity it declared is logged once accepted.
fn read_handshake<T: LineTransport>(
    transport: &mut T,
    peer_cid: u32,
    log: &dyn SessionLog,
) -> Result<Handshake, HostError> {
    let inbound = match transport.recv_line() {
        Ok(Some(line)) => line,
        Ok(None) => {
            return Err(HostError::Protocol(
                "the guest closed before sending hello".to_string(),
            ));
        }
        Err(err) => return Err(HostError::from(err)),
    };
    log.line(&format!(
        "[guest {peer_cid} vm? ->] {}",
        sanitize_for_log(&inbound)
    ));
    match GuestMessage::from_line(&inbound) {
        Ok(GuestMessage::Hello {
            vm,
            guest_cid,
            role,
            keepalive,
        }) => {
            // The guest's claim about its own CID is checked against the CID
            // the accepted socket path encodes. The muxer carries no peer
            // address, so this is identity by configuration: a disagreement
            // means the guest is not who it says it is, and its session is
            // dropped rather than served under a name it has not earned.
            if guest_cid != peer_cid {
                return Err(HostError::Protocol(format!(
                    "hello claims guest_cid {guest_cid}, but the connection was accepted on the socket for cid {peer_cid}"
                )));
            }
            log.line(&format!(
                "vsockd: guest {guest_cid} vm {vm} says hello (accepted on the socket for cid {peer_cid})"
            ));
            Ok(Handshake {
                vm,
                guest_cid,
                role,
                keepalive: keepalive == Some(true),
            })
        }
        Ok(GuestMessage::ToolCall { call, name, .. }) => Err(HostError::Protocol(format!(
            "tool_call {call} ({name}) before hello"
        ))),
        Ok(GuestMessage::ToolResult { call, .. }) => Err(HostError::Protocol(format!(
            "tool_result {call} before hello"
        ))),
        Ok(GuestMessage::Heartbeat { seq }) => Err(HostError::Protocol(format!(
            "heartbeat {seq} before hello"
        ))),
        Ok(GuestMessage::Bye { .. }) => {
            Err(HostError::Protocol("bye before hello".to_string()))
        }
        Err(err) => Err(HostError::Protocol(format!(
            "unparsable guest line ({err}): {inbound:?}"
        ))),
    }
}

/// Serves the frozen v1 session after its welcome: the guest's tool calls and
/// its `bye`.
fn run_v1_body<T: LineTransport>(
    transport: &mut T,
    hello: &Handshake,
    limits: SessionLimits,
    log: &dyn SessionLog,
) -> Result<SessionSummary, HostError> {
    let (cid, vm) = (hello.guest_cid, hello.vm);
    let mut calls = 0u64;

    loop {
        if limits.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(HostError::Protocol("session deadline exceeded".to_string()));
        }
        let inbound = match transport.recv_line() {
            Ok(Some(line)) => line,
            Ok(None) => {
                return Err(HostError::Protocol(
                    "the guest closed before sending bye".to_string(),
                ));
            }
            Err(err) => return Err(HostError::from(err)),
        };
        log.line(&format!(
            "[guest {cid} vm{vm} ->] {}",
            sanitize_for_log(&inbound)
        ));

        match GuestMessage::from_line(&inbound) {
            Ok(GuestMessage::ToolCall { call, name, args }) => {
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
                    cid,
                    vm,
                    &HostMessage::ToolResult { call, ok, output },
                    log,
                )?;
                log.line(&format!(
                    "vsockd: answered call {call} ({}) for guest {cid} vm {vm}",
                    sanitize_for_log(&name)
                ));
            }
            Ok(GuestMessage::Bye { calls: guest_calls }) => {
                if guest_calls != calls {
                    log.line(&format!(
                        "vsockd: guest {cid} vm {vm} counted {guest_calls} tool calls, this side counted {calls}"
                    ));
                }
                log.line(&format!(
                    "vsockd: guest {cid} vm {vm} said bye after {calls} tool calls"
                ));
                return Ok(SessionSummary {
                    peer_cid: cid,
                    guest_cid: cid,
                    vm,
                    calls,
                    guest_calls,
                    keepalive: false,
                });
            }
            Ok(GuestMessage::Hello {
                vm: again_vm,
                guest_cid: again_cid,
                ..
            }) => {
                return Err(HostError::Protocol(format!(
                    "a second hello from guest {again_cid} vm {again_vm} (already greeted guest {cid} vm {vm})"
                )));
            }
            Ok(GuestMessage::Heartbeat { seq }) => {
                return Err(HostError::Protocol(format!(
                    "heartbeat {seq} in a session that did not ask for keepalive"
                )));
            }
            Ok(GuestMessage::ToolResult { call, .. }) => {
                return Err(HostError::Protocol(format!(
                    "tool_result {call} in a session this side issued no calls to"
                )));
            }
            Err(err) => {
                return Err(HostError::Protocol(format!(
                    "unparsable guest line ({err}): {inbound:?}"
                )));
            }
        }
    }
}

/// The `tool_call` a keepalive session dispatches to its agent.
///
/// The session owns the numbering — the protocol's `call` is "the sequence
/// number within the session" — so the line is built here from the hub's
/// request rather than forwarded from it.
fn tool_call_message(call: u64, name: String, args: serde_json::Value) -> HostMessage {
    HostMessage::ToolCall { call, name, args }
}

/// The `stop` that ends a keepalive session, in the hub's words.
fn stop_message(reason: String) -> HostMessage {
    HostMessage::Stop { reason }
}

/// Serves one keepalive session: the welcome, then the loop that serves the
/// hub's tool calls, records the agent's heartbeats, and ends on the agent's
/// `bye`.
fn run_keepalive<T: LineTransport>(
    transport: &mut T,
    hello: &Handshake,
    peer_cid: u32,
    limits: SessionLimits,
    idle: Duration,
    log: &dyn SessionLog,
    hub: &Hub,
    clock: &dyn Clock,
) -> Result<SessionSummary, HostError> {
    let (cid, vm) = (hello.guest_cid, hello.vm);
    let role = hello.role.as_deref().unwrap_or("");
    let accepted_at = clock.now();
    // Register before the welcome, so the agent is never visible to a status
    // surface without having been accepted. The guard unregisters on *every*
    // exit below — including an I/O error mid-session — so a doomed session
    // cannot leak its row. Once a clean end or a reap has removed it, the
    // guard is a no-op.
    let (token, steering) = hub.connect(cid, role, accepted_at);
    let _registered = Registration { hub, cid, token };

    send(
        transport,
        cid,
        vm,
        &HostMessage::keepalive_welcome(role),
        log,
    )?;

    let mut calls = 0u64;
    let mut outstanding: Vec<u64> = Vec::new();
    let mut last_heartbeat = accepted_at;
    let mut stopping = false;

    loop {
        if clock.now().saturating_duration_since(last_heartbeat) > idle {
            hub.reap_idle(cid, token, idle);
            return Err(HostError::Reaped);
        }

        // The hub's own dispatches. Once a stop has gone out the session is
        // ending, so no further work is dispatched into it.
        if !stopping {
            loop {
                let message = match steering.try_recv() {
                    Ok(message) => message,
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                };
                match message {
                    // The `call` number is the *session's*: the hub says which
                    // tool to run, and the session numbers the exchange, so the
                    // agent sees one monotonic sequence per session.
                    HostMessage::ToolCall { name, args, .. } => {
                        if calls >= limits.max_calls {
                            log.line(&format!(
                                "vsockd: refusing the hub's call ({}) for agent {cid}: \
                                 the session's {}-call budget is exhausted",
                                sanitize_for_log(&name),
                                limits.max_calls
                            ));
                            return Err(HostError::Protocol(format!(
                                "tool-call budget ({}) exceeded",
                                limits.max_calls
                            )));
                        }
                        calls += 1;
                        let call = calls;
                        hub.note_calls(cid, calls);
                        outstanding.push(call);
                        send(
                            transport,
                            cid,
                            vm,
                            &tool_call_message(call, name, args),
                            log,
                        )?;
                    }
                    HostMessage::Stop { reason } => {
                        send(transport, cid, vm, &stop_message(reason), log)?;
                        stopping = true;
                    }
                    other => {
                        // The hub speaks `HostMessage`, but only two of them
                        // are the hub's to dispatch into a session: anything
                        // else is refused rather than guessed at.
                        log.line(&format!(
                            "vsockd: ignoring a host line the hub does not dispatch: {other:?}"
                        ));
                    }
                }
            }
        }

        match transport.recv_line_idle() {
            Ok(Inbound::Idle) => continue,
            Ok(Inbound::Closed) => {
                let detail = if stopping {
                    "the keepalive guest closed after a stop without sending bye"
                } else {
                    "the keepalive guest closed without sending bye"
                };
                return Err(HostError::Protocol(detail.to_string()));
            }
            Err(err) => return Err(HostError::from(err)),
            Ok(Inbound::Line(line)) => {
                log.line(&format!(
                    "[guest {cid} vm{vm} ->] {}",
                    sanitize_for_log(&line)
                ));
                match GuestMessage::from_line(&line) {
                    Ok(GuestMessage::Heartbeat { seq }) => {
                        // A heartbeat after a stop does not extend the
                        // session: the hub has already ended it, and the agent
                        // owes a `bye`. It is still recorded, so the operator
                        // sees the agent is alive but ignoring the stop.
                        if !stopping {
                            last_heartbeat = clock.now();
                        }
                        hub.heartbeat(cid, seq, clock.now());
                    }
                    Ok(GuestMessage::ToolResult { call, ok, .. }) => {
                        match outstanding.iter().position(|issued| *issued == call) {
                            Some(index) => {
                                outstanding.remove(index);
                                log.line(&format!(
                                    "vsockd: agent {cid} answered tool call {call} (ok: {ok})"
                                ));
                            }
                            None => log.line(&format!(
                                "vsockd: agent {cid} answered tool call {call}, which this side did not issue"
                            )),
                        }
                    }
                    Ok(GuestMessage::ToolCall { call, name, args }) => {
                        // A keepalive agent may still ask the hub for a tool,
                        // exactly as in v1; the hub answers it the same way.
                        if calls >= limits.max_calls {
                            log.line(&format!(
                                "vsockd: refusing call {call} ({}) for agent {cid}: the \
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
                        hub.note_calls(cid, calls);
                        let (ok, output) = run_tool(&name, &args);
                        send(
                            transport,
                            cid,
                            vm,
                            &HostMessage::ToolResult { call, ok, output },
                            log,
                        )?;
                        log.line(&format!(
                            "vsockd: answered call {call} ({}) for agent {cid}",
                            sanitize_for_log(&name)
                        ));
                    }
                    Ok(GuestMessage::Bye { calls: guest_calls }) => {
                        if guest_calls != calls {
                            log.line(&format!(
                                "vsockd: agent {cid} counted {guest_calls} tool calls, this side counted {calls}"
                            ));
                        }
                        hub.end_clean(cid, token, calls);
                        return Ok(SessionSummary {
                            peer_cid,
                            guest_cid: cid,
                            vm,
                            calls,
                            guest_calls,
                            keepalive: true,
                        });
                    }
                    Ok(GuestMessage::Hello {
                        vm: again_vm,
                        guest_cid: again_cid,
                        ..
                    }) => {
                        return Err(HostError::Protocol(format!(
                            "a second hello from guest {again_cid} vm {again_vm} (already greeted guest {cid} vm {vm})"
                        )));
                    }
                    Err(err) => {
                        return Err(HostError::Protocol(format!(
                            "unparsable guest line ({err}): {line:?}"
                        )));
                    }
                }
            }
        }
    }
}

/// Unregisters a keepalive session however it ends.
struct Registration<'a> {
    hub: &'a Hub,
    cid: u32,
    token: u64,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.hub.hangup(self.cid, self.token);
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
    cid: u32,
    vm: u32,
    message: &HostMessage,
    log: &dyn SessionLog,
) -> Result<(), HostError> {
    let line = message
        .to_line()
        .map_err(|err| HostError::Protocol(format!("cannot encode a host line: {err}")))?;
    log.line(&format!("[<-] guest {cid} vm{vm} {line}"));
    transport.send_line(&line)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use crate::line::StreamLines;

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

    /// A transport driven by a script: the steps are consumed in order, and an
    /// exhausted script leaves the connection open but silent (`Idle`) or
    /// closed, however the script says.
    #[derive(Default)]
    struct Scripted {
        steps: Mutex<VecDeque<Step>>,
        sent: Mutex<Vec<String>>,
    }

    enum Step {
        Line(String),
        Idle,
        Closed,
    }

    impl Scripted {
        /// The scripted lines, then a closed connection.
        fn script(lines: &[&str]) -> Self {
            let mut steps: VecDeque<Step> = lines.iter().map(|l| Step::Line(l.to_string())).collect();
            steps.push_back(Step::Closed);
            Self {
                steps: Mutex::new(steps),
                sent: Mutex::new(Vec::new()),
            }
        }

        /// The scripted lines, then silence — the connection stays open.
        fn idling(lines: &[&str]) -> Self {
            let steps: VecDeque<Step> = lines.iter().map(|l| Step::Line(l.to_string())).collect();
            Self {
                steps: Mutex::new(steps),
                sent: Mutex::new(Vec::new()),
            }
        }

        fn next(&self) -> Option<Step> {
            self.steps
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .pop_front()
        }

        fn sent(&self) -> Vec<String> {
            self.sent
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .clone()
        }
    }

    impl LineTransport for Scripted {
        fn send_line(&mut self, line: &str) -> io::Result<()> {
            self.sent
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(line.to_string());
            Ok(())
        }

        fn recv_line(&mut self) -> io::Result<Option<String>> {
            match self.next() {
                None | Some(Step::Closed) => Ok(None),
                Some(Step::Idle) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                Some(Step::Line(line)) => Ok(Some(line)),
            }
        }

        fn recv_line_idle(&mut self) -> io::Result<Inbound> {
            match self.next() {
                None | Some(Step::Idle) => Ok(Inbound::Idle),
                Some(Step::Closed) => Ok(Inbound::Closed),
                Some(Step::Line(line)) => Ok(Inbound::Line(line)),
            }
        }
    }

    /// A clock the test steps by hand.
    struct StepClock {
        next: Mutex<Instant>,
        step: Duration,
    }

    impl StepClock {
        fn new(step: Duration) -> Self {
            Self {
                next: Mutex::new(Instant::now()),
                step,
            }
        }
    }

    impl Clock for StepClock {
        fn now(&self) -> Instant {
            let mut next = self.next.lock().unwrap_or_else(|err| err.into_inner());
            let now = *next;
            *next = now.checked_add(self.step).unwrap_or(now);
            now
        }
    }

    /// A hub that logs into `log`.
    fn hub(log: &Arc<Recording>) -> Hub {
        Hub::new(Arc::clone(log) as Arc<dyn SessionLog + Send + Sync>)
    }

    /// Receives one line, expecting it to be there.
    fn next(guest: &mut Channel) -> String {
        guest
            .recv_line()
            .expect("the host sent a line")
            .expect("the connection is open")
    }

    /// Unbounded session limits with keepalive disabled — the v1 tests
    /// exercise the protocol, not the bounds.
    fn limits() -> SessionLimits {
        SessionLimits {
            max_calls: u64::MAX,
            deadline: None,
            keepalive_idle: None,
            keepalive_read_slice: Duration::from_secs(1),
        }
    }

    /// Limits that accept keepalive sessions, sized for a test.
    fn keepalive_limits() -> SessionLimits {
        SessionLimits {
            max_calls: 64,
            deadline: None,
            keepalive_idle: Some(Duration::from_secs(300)),
            keepalive_read_slice: Duration::from_millis(20),
        }
    }

    #[test]
    fn the_frozen_session_round_trips_against_a_scripted_guest() {
        let (mut guest, mut host) = pair();
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let clock = SystemClock;

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

        let summary = handle_session(&mut host, 3, limits(), &*log, &hub, &clock)
            .expect("the session completes");
        assert_eq!(
            summary,
            SessionSummary {
                peer_cid: 3,
                guest_cid: 3,
                vm: 0,
                calls: 2,
                guest_calls: 2,
                keepalive: false,
            }
        );
        script.join().expect("the scripted guest finished");

        let text = log.text();
        assert!(
            text.contains("[guest 3 vm? ->] {\"type\":\"hello\""),
            "log: {text}"
        );
        assert!(
            text.contains("[guest 3 vm0 ->] {\"type\":\"tool_call\",\"call\":1"),
            "log: {text}"
        );
        assert!(
            text.contains("[<-] guest 3 vm0 {\"type\":\"welcome\""),
            "log: {text}"
        );
        assert!(text.contains("said bye after 2 tool calls"), "log: {text}");
    }

    #[test]
    fn echo_refuses_arguments_it_cannot_echo() {
        let (mut guest, mut host) = pair();
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
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
        let summary = handle_session(&mut host, 3, limits(), &*log, &hub, &SystemClock)
            .expect("the session completes");
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
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
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
        let summary = handle_session(&mut host, 4, limits(), &*log, &hub, &SystemClock)
            .expect("the session completes");
        let refused = script.join().expect("the scripted guest finished");
        let refused: serde_json::Value = serde_json::from_str(&refused).expect("the result is JSON");
        assert_eq!(refused["ok"], serde_json::json!(false));
        assert_eq!(refused["output"], serde_json::json!("unknown tool: rm_rf"));
        assert_eq!(summary.calls, 1);
    }

    #[test]
    fn a_tool_call_before_hello_is_dropped() {
        let (mut guest, mut host) = pair();
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        guest
            .send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"hi"}}"#)
            .expect("call sends");
        let err = handle_session(&mut host, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("before hello")),
            "got {err:?}"
        );
    }

    #[test]
    fn a_malformed_line_drops_the_session() {
        let (mut guest, mut host) = pair();
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        guest.send_line("not json").expect("garbage sends");
        let err = handle_session(&mut host, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("unparsable guest line")),
            "got {err:?}"
        );
    }

    #[test]
    fn a_second_hello_is_dropped() {
        let (mut guest, mut host) = pair();
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let script = thread::spawn(move || {
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("hello sends");
            let _welcome = next(&mut guest);
            guest
                .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("second hello sends");
        });
        let err = handle_session(&mut host, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("second hello")),
            "got {err:?}"
        );
        script.join().expect("the scripted guest finished");
    }

    #[test]
    fn a_hello_that_claims_the_wrong_cid_is_dropped() {
        let (mut guest, mut host) = pair();
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        guest
            .send_line(r#"{"type":"hello","vm":0,"guest_cid":4}"#)
            .expect("hello sends");
        let err = handle_session(&mut host, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail)
                if detail.contains("claims guest_cid 4") && detail.contains("accepted on the socket for cid 3")),
            "got {err:?}"
        );
        let text = log.text();
        assert!(text.contains("[guest 3 vm? ->]"), "log: {text}");
    }

    #[test]
    fn a_guest_that_never_greets_is_reported() {
        let (guest, mut host) = pair();
        drop(guest);
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let err = handle_session(&mut host, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
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
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
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
            ..limits()
        };
        let err = handle_session(&mut host, 3, limits, &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
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
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        guest
            .send_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
            .expect("hello sends");
        let limits = SessionLimits {
            deadline: Some(Instant::now()),
            ..limits()
        };
        let err = handle_session(&mut host, 3, limits, &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
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
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        // Valid UTF-8, no newline, but an interior ESC sequence and carriage
        // return — exactly the bytes the framing leaves untouched.
        guest
            .send_line("not json\u{1b}[2K\rforged line")
            .expect("the crafted line sends");
        let _err = handle_session(&mut host, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the malformed line drops the session");
        let logged = log.text();
        assert!(logged.contains("\\x1b"), "ESC must be escaped: {logged}");
        assert!(logged.contains("\\x0d"), "CR must be escaped: {logged}");
        assert!(!logged.contains('\u{1b}'), "raw ESC must not reach the log: {logged}");
        assert!(!logged.contains('\r'), "raw CR must not reach the log: {logged}");
    }

    /// A hello that asks for keepalive on a daemon configured for one-shot
    /// sessions is refused, never served as something else. The reason travels
    /// back to the caller, which logs the drop (the v1 refusals' path).
    #[test]
    fn a_keepalive_hello_is_refused_when_keepalive_is_not_served() {
        let scripted = Scripted::script(&[
            r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder","keepalive":true}"#,
        ]);
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let mut scripted = scripted;
        let err = handle_session(&mut scripted, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the keepalive hello is refused");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("configured for one-shot sessions")),
            "got {err:?}"
        );
        // Nothing was registered and nothing was sent: a refused handshake has
        // no side effects.
        assert_eq!(hub.agent_count(), 0);
        assert!(scripted.sent().is_empty(), "{:?}", scripted.sent());
    }

    /// A keepalive session counts tool calls against the same budget as a v1
    /// one: an agent that keeps asking past it is refused, dropped, and logged
    /// — and its registry row goes with it.
    #[test]
    fn a_keepalive_tool_call_budget_is_enforced() {
        let mut scripted = Scripted::script(&[
            r#"{"type":"hello","vm":0,"guest_cid":22,"role":"prober","keepalive":true}"#,
            r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"a"}}"#,
            r#"{"type":"tool_call","call":2,"name":"echo","args":{"text":"b"}}"#,
            r#"{"type":"tool_call","call":3,"name":"echo","args":{"text":"c"}}"#,
        ]);
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let limits = SessionLimits {
            max_calls: 2,
            ..keepalive_limits()
        };
        let err = handle_session(&mut scripted, 22, limits, &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("tool-call budget (2) exceeded")),
            "got {err:?}"
        );
        assert_eq!(hub.agent_count(), 0, "the dropped session is unregistered");
        assert!(
            log.text().contains("budget is exhausted"),
            "log must record the refusal: {}",
            log.text()
        );
    }

    /// A heartbeat in a session that never asked for keepalive is a protocol
    /// violation, not a silently ignored line.
    #[test]
    fn a_heartbeat_in_a_v1_session_is_refused() {
        let mut scripted = Scripted::script(&[
            r#"{"type":"hello","vm":0,"guest_cid":3}"#,
            r#"{"type":"heartbeat","seq":1}"#,
        ]);
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let err = handle_session(&mut scripted, 3, limits(), &*log, &hub, &SystemClock)
            .expect_err("the session is dropped");
        assert!(
            matches!(&err, HostError::Protocol(detail) if detail.contains("did not ask for keepalive")),
            "got {err:?}"
        );
    }

    /// A role without keepalive is a v1 session: the v1 welcome's byte-shape is
    /// unchanged, so the role is not echoed into it.
    #[test]
    fn a_role_without_keepalive_gets_the_v1_welcome() {
        let mut scripted = Scripted::script(&[
            r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder"}"#,
            r#"{"type":"bye","calls":0}"#,
        ]);
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let summary = handle_session(&mut scripted, 3, limits(), &*log, &hub, &SystemClock)
            .expect("the session completes");
        assert_eq!(
            scripted.sent(),
            vec![r#"{"type":"welcome","echo":"plaza-orchestrator"}"#]
        );
        assert!(!summary.keepalive);
        assert_eq!(hub.agent_count(), 0, "a v1 session is not registered");
    }

    /// A keepalive agent that stops heartbeating is reaped at the idle
    /// deadline and its row is dropped — a dead guest is never leaked.
    ///
    /// The clock is injected and stepped by hand (`StepClock`), so the 300 s
    /// deadline is proven without waiting for it.
    #[test]
    fn a_keepalive_agent_that_stops_beating_is_reaped() {
        let mut scripted = Scripted::idling(&[
            r#"{"type":"hello","vm":0,"guest_cid":21,"role":"prober","keepalive":true}"#,
        ]);
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let clock = StepClock::new(Duration::from_secs(100));
        let err = handle_session(&mut scripted, 21, keepalive_limits(), &*log, &hub, &clock)
            .expect_err("the idle agent is reaped");
        assert!(matches!(err, HostError::Reaped), "got {err:?}");
        assert_eq!(hub.agent_count(), 0, "the reaped agent's row is dropped");
        let text = log.text();
        assert!(
            text.contains("vsockd: agent 21 role prober connected"),
            "log: {text}"
        );
        assert!(
            text.contains("vsockd: agent 21 reaped idle > 300s"),
            "log: {text}"
        );
        assert!(
            !text.contains("ended cleanly"),
            "a reap is not a clean end: {text}"
        );
    }

    /// The live proof of the keepalive hub: a stub client speaks raw JSON
    /// lines over a real Unix socket pair, the session runs the other end, and
    /// the test steers the hub (as the control plane of a later slice will),
    /// so the whole v2 exchange happens for real — welcome with the role,
    /// heartbeats, the hub's tool calls and their results, a stop, a clean
    /// `bye`, the clean-end log, and a drained registry.
    #[test]
    fn the_keepalive_hub_round_trips_over_a_unix_socket() {
        use std::os::unix::net::UnixStream;

        let (guest_stream, hub_stream) = UnixStream::pair().expect("a Unix socket pair");
        guest_stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("the stub's read timeout");
        let log = Arc::new(Recording::default());
        let hub = Arc::new(hub(&log));

        let limits = keepalive_limits();
        let session_hub = Arc::clone(&hub);
        let session_log = Arc::clone(&log);
        let session = thread::spawn(move || {
            let mut transport = StreamLines::new(hub_stream);
            handle_session(
                &mut transport,
                20,
                limits,
                &*session_log,
                &session_hub,
                &SystemClock,
            )
        });

        let mut guest = StreamLines::new(guest_stream);

        // 1. hello with a role and keepalive; the welcome echoes what the hub
        //    accepted.
        guest
            .send_line(r#"{"type":"hello","vm":0,"guest_cid":20,"role":"builder","keepalive":true}"#)
            .expect("hello sends");
        assert_eq!(
            strand(&mut guest),
            r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#
        );

        // 2. the agent beats.
        for seq in 1..=3 {
            guest
                .send_line(&format!(r#"{{"type":"heartbeat","seq":{seq}}}"#))
                .expect("a heartbeat sends");
        }

        // 3. the hub dispatches two tool calls at will; the agent answers each.
        let steering = hub
            .steer(20)
            .expect("the connected agent has a steering channel");
        let calls = [
            (1u64, "echo", serde_json::json!({ "text": "plaza-agent-20" })),
            (2, "uptime", serde_json::json!({})),
        ];
        for (call, name, args) in calls {
            steering
                .send(HostMessage::ToolCall {
                    call,
                    name: name.to_string(),
                    args,
                })
                .expect("the hub dispatches");
            // A heartbeat wakes the session so it takes its dispatch at once.
            guest
                .send_line(&format!(r#"{{"type":"heartbeat","seq":{}}}"#, 3 + call))
                .expect("a heartbeat sends");
            let dispatched: serde_json::Value =
                serde_json::from_str(&strand(&mut guest)).expect("the dispatch is JSON");
            assert_eq!(dispatched["type"], "tool_call");
            assert_eq!(dispatched["call"], serde_json::json!(call));
            assert_eq!(dispatched["name"], serde_json::json!(name));
            guest
                .send_line(&format!(
                    r#"{{"type":"tool_result","call":{call},"ok":true,"output":"done"}}"#
                ))
                .expect("the result sends");
        }

        // 4. the hub stops the agent; the agent answers bye and the session
        //    ends cleanly.
        steering
            .send(HostMessage::Stop {
                reason: "slice proof".to_string(),
            })
            .expect("the hub stops it");
        guest
            .send_line(r#"{"type":"heartbeat","seq":9}"#)
            .expect("a heartbeat sends");
        assert_eq!(strand(&mut guest), r#"{"type":"stop","reason":"slice proof"}"#);
        guest
            .send_line(r#"{"type":"bye","calls":2}"#)
            .expect("bye sends");

        let summary = session
            .join()
            .expect("the session thread finishes")
            .expect("the session completes");
        assert_eq!(
            summary,
            SessionSummary {
                peer_cid: 20,
                guest_cid: 20,
                vm: 0,
                calls: 2,
                guest_calls: 2,
                keepalive: true,
            }
        );
        assert_eq!(hub.agent_count(), 0, "the registry is drained");

        let text = log.text();
        for expected in [
            "vsockd: agent 20 role builder connected",
            "vsockd: agent 20 heartbeat 1",
            "vsockd: agent 20 answered tool call 1 (ok: true)",
            "vsockd: agent 20 answered tool call 2 (ok: true)",
            "vsockd: agent 20 session ended cleanly after 2 tool calls",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
        // The proof's transcript, for `cargo test -- --nocapture`.
        println!("--- keepalive session, cid 20, over a Unix socket ---");
        println!("{text}");
        println!("--- end (registry drained: {} agents) ---", hub.agent_count());
    }

    /// Reads one host line from the stub's end of the socket.
    fn strand(guest: &mut StreamLines<std::os::unix::net::UnixStream>) -> String {
        match guest.recv_line().expect("the host sent a line") {
            Some(line) => line,
            None => panic!("the hub closed the session"),
        }
    }

    /// The registry keeps the agent's role and running call count, and drops
    /// the row when the session ends.
    #[test]
    fn the_registry_tracks_the_agents_role_and_calls() {
        let log = Arc::new(Recording::default());
        let hub = hub(&log);
        let (token, _steering) = hub.connect(30, "builder", Instant::now());
        let agent = hub.agent(30).expect("the agent is registered");
        assert_eq!((agent.cid, agent.role.as_str()), (30, "builder"));
        assert_eq!(agent.calls, 0);
        hub.note_calls(30, 7);
        assert_eq!(hub.agent(30).expect("registered").calls, 7);
        hub.end_clean(30, token, 7);
        assert!(hub.agent(30).is_none());
        assert_eq!(hub.agent_count(), 0);
        assert!(
            log.text()
                .contains("vsockd: agent 30 session ended cleanly after 7 tool calls"),
            "log: {}",
            log.text()
        );
    }
}
