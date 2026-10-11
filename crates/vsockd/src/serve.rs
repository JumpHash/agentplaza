//! The host side of one guest session, chosen by its `hello`:
//!
//! * **v1** — welcome, the guest's tool calls (`echo`, `uptime`), `bye`.
//! * **keepalive** — registered with the [`Hub`]; the hub's commands go out
//!   as `tool_call`s, raw lines or `stop`, heartbeats keep it alive, and it is
//!   reaped after [`SessionLimits::keepalive_idle`] without one.
//!
//! A tool that cannot run answers `ok: false`; a line that is not a valid
//! guest message drops the session. The CID is the socket's, never the line's.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use apz_proto::line::{Lines, ReadTimeout, Recv};
use apz_proto::wire::{GuestMessage, HostMessage};

use crate::hub::{Command, Hub, ToolReply};

/// Where session log lines go.
pub trait SessionLog {
    fn line(&self, text: &str);
}

/// Standard output, one whole line per write.
pub struct StdoutLog;

impl SessionLog for StdoutLog {
    fn line(&self, text: &str) {
        let _ = writeln!(io::stdout().lock(), "{text}");
    }
}

/// What deadlines are measured against; tests step it by hand.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// What one completed session did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSummary {
    pub guest_cid: u32,
    pub vm: u32,
    /// Tool calls served, in either direction.
    pub calls: u64,
    /// What the guest's `bye` counted.
    pub guest_calls: u64,
    pub keepalive: bool,
}

/// The bounds a session is held to.
#[derive(Debug, Clone, Copy)]
pub struct SessionLimits {
    /// Tool calls per session, both directions.
    pub max_calls: u64,
    /// A v1 session's wall-clock end.
    pub deadline: Option<Instant>,
    /// No heartbeat for this long reaps a keepalive session; `None` refuses
    /// keepalive hellos.
    pub keepalive_idle: Option<Duration>,
    /// How often a keepalive session wakes to take the hub's commands.
    pub keepalive_read_slice: Duration,
}

/// Why a session ended without a `bye`.
#[derive(Debug)]
pub enum HostError {
    Io(io::Error),
    Protocol(String),
    /// A keepalive connection dropped: expected, the guest reconnects.
    Dropped(String),
    /// Reaped for going silent (the hub logged it).
    Reaped,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "connection failed: {err}"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
            Self::Dropped(detail) => write!(f, "connection lost: {detail}"),
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

fn violation(detail: impl Into<String>) -> HostError {
    HostError::Protocol(detail.into())
}

/// Serves one guest session accepted on the socket for `peer_cid`.
pub fn handle_session<S: Read + Write + ReadTimeout>(
    lines: &mut Lines<S>,
    peer_cid: u32,
    limits: SessionLimits,
    log: &dyn SessionLog,
    hub: &Hub,
    clock: &dyn Clock,
) -> Result<SessionSummary, HostError> {
    let line = match lines.recv()? {
        Recv::Line(line) => line,
        Recv::Idle => return Err(violation("the guest sent no hello within the read timeout")),
        Recv::Closed => return Err(violation("the guest closed before sending hello")),
    };
    log.line(&format!("[guest {peer_cid} vm? ->] {}", sanitize_for_log(&line)));
    let (vm, role, keepalive) = match GuestMessage::from_line(&line) {
        Ok(GuestMessage::Hello { vm, guest_cid, role, keepalive }) => {
            // Identity is the socket path's; a guest claiming another CID is dropped.
            if guest_cid != peer_cid {
                return Err(violation(format!(
                    "hello claims guest_cid {guest_cid}, but the connection was accepted on the socket for cid {peer_cid}"
                )));
            }
            log.line(&format!("vsockd: guest {peer_cid} vm {vm} says hello (accepted on the socket for cid {peer_cid})"));
            (vm, role, keepalive == Some(true))
        }
        Ok(other) => return Err(violation(format!("{} before hello", guest_kind(&other)))),
        Err(err) => return Err(violation(format!("unparsable guest line ({err}): {line:?}"))),
    };
    let session = Session { cid: peer_cid, vm, limits, log };
    if !keepalive {
        session.send(lines, &HostMessage::welcome(None, None, None))?;
        return session.v1(lines);
    }
    let Some(idle) = limits.keepalive_idle else {
        return Err(violation("a keepalive hello on a daemon configured for one-shot sessions only"));
    };
    lines.get_ref().set_read_timeout(Some(limits.keepalive_read_slice))?;
    session.keepalive(lines, role.as_deref().unwrap_or(""), idle, hub, clock)
}

struct Session<'a> {
    cid: u32,
    vm: u32,
    limits: SessionLimits,
    log: &'a dyn SessionLog,
}

impl Session<'_> {
    /// The v1 body: the guest's tool calls until its `bye`.
    fn v1<S: Read + Write>(&self, lines: &mut Lines<S>) -> Result<SessionSummary, HostError> {
        let (cid, vm) = (self.cid, self.vm);
        let mut calls = 0u64;
        loop {
            if self.limits.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(violation("session deadline exceeded"));
            }
            let line = match lines.recv()? {
                Recv::Line(line) => line,
                Recv::Idle => return Err(HostError::Io(io::ErrorKind::TimedOut.into())),
                Recv::Closed => return Err(violation("the guest closed before sending bye")),
            };
            match self.parse(&line)? {
                GuestMessage::ToolCall { call, name, args } => {
                    self.charge(&mut calls, &name)?;
                    let (ok, output) = run_tool(&name, &args);
                    self.send(lines, &HostMessage::ToolResult { call, ok, output })?;
                    self.log.line(&format!("vsockd: answered call {call} ({}) for guest {cid} vm {vm}", sanitize_for_log(&name)));
                }
                GuestMessage::Bye { calls: guest_calls } => {
                    if guest_calls != calls {
                        self.log.line(&format!("vsockd: guest {cid} vm {vm} counted {guest_calls} tool calls, this side counted {calls}"));
                    }
                    self.log.line(&format!("vsockd: guest {cid} vm {vm} said bye after {calls} tool calls"));
                    return Ok(SessionSummary { guest_cid: cid, vm, calls, guest_calls, keepalive: false });
                }
                GuestMessage::Hello { .. } => return Err(violation(format!("a second hello from guest {cid} vm {vm}"))),
                other => {
                    return Err(violation(format!("{} in a session that did not ask for keepalive", guest_kind(&other))));
                }
            }
        }
    }

    /// The keepalive body: the hub's commands out, the agent's lines in, until
    /// its `bye`, a drop, or the idle reap.
    fn keepalive<S: Read + Write>(
        &self,
        lines: &mut Lines<S>,
        role: &str,
        idle: Duration,
        hub: &Hub,
        clock: &dyn Clock,
    ) -> Result<SessionSummary, HostError> {
        let cid = self.cid;
        let mut last_heartbeat = clock.now();
        // Registered before the welcome; the guard unregisters on every exit.
        let (token, steering) = hub.connect(cid, role, last_heartbeat);
        let _registered = Registration { hub, cid, token };
        self.send(lines, &HostMessage::welcome(Some(role.to_string()), Some(true), None))?;

        let mut calls = 0u64;
        // Dispatched tool calls not yet answered, and who waits on each.
        let mut outstanding: Vec<(u64, Option<Sender<ToolReply>>)> = Vec::new();
        let mut stopping = false;
        loop {
            if clock.now().saturating_duration_since(last_heartbeat) > idle {
                hub.reap_idle(cid, token, idle);
                return Err(HostError::Reaped);
            }
            // Once a stop is out, nothing more is dispatched.
            while !stopping && let Ok(command) = steering.try_recv() {
                match command {
                    Command::Call { name, args, reply } => {
                        self.charge(&mut calls, &name)?;
                        hub.note_calls(cid, calls);
                        outstanding.push((calls, reply));
                        self.send(lines, &HostMessage::ToolCall { call: calls, name, args })?;
                    }
                    Command::Send { message, written } => {
                        self.send(lines, &message)?;
                        let _ = written.send(());
                    }
                    Command::Stop { reason } => {
                        self.send(lines, &HostMessage::Stop { reason })?;
                        stopping = true;
                    }
                }
            }
            let line = match lines.recv()? {
                Recv::Line(line) => line,
                Recv::Idle => continue,
                Recv::Closed if stopping => {
                    return Err(HostError::Dropped("the agent dropped after a stop, before its bye".into()));
                }
                Recv::Closed => {
                    return Err(HostError::Dropped("the agent's connection dropped; it reconnects with backoff".into()));
                }
            };
            match self.parse(&line)? {
                GuestMessage::Heartbeat { seq, busy } => {
                    // A beat after a stop is recorded but does not extend the session.
                    if !stopping {
                        last_heartbeat = clock.now();
                    }
                    hub.heartbeat(cid, seq, busy.unwrap_or(false), clock.now());
                }
                GuestMessage::ToolResult { call, ok, output } => {
                    match outstanding.iter().position(|(issued, _)| *issued == call) {
                        Some(index) => {
                            self.log.line(&format!("vsockd: agent {cid} answered tool call {call} (ok: {ok})"));
                            // A waiter that gave up has dropped its receiver.
                            if let (_, Some(reply)) = outstanding.remove(index) {
                                let _ = reply.send(ToolReply { ok, output });
                            }
                        }
                        None => self.log.line(&format!(
                            "vsockd: agent {cid} answered tool call {call}, which this side did not issue"
                        )),
                    }
                }
                GuestMessage::ToolCall { call, name, args } => {
                    self.charge(&mut calls, &name)?;
                    hub.note_calls(cid, calls);
                    let (ok, output) = run_tool(&name, &args);
                    self.send(lines, &HostMessage::ToolResult { call, ok, output })?;
                    self.log.line(&format!("vsockd: answered call {call} ({}) for agent {cid}", sanitize_for_log(&name)));
                }
                GuestMessage::Bye { calls: guest_calls } => {
                    if guest_calls != calls {
                        self.log.line(&format!("vsockd: agent {cid} counted {guest_calls} tool calls, this side counted {calls}"));
                    }
                    hub.end_clean(cid, token, calls, guest_calls);
                    return Ok(SessionSummary { guest_cid: cid, vm: self.vm, calls, guest_calls, keepalive: true });
                }
                GuestMessage::Hello { .. } => return Err(violation(format!("a second hello from agent {cid}"))),
                other => self.log.line(&format!(
                    "vsockd: agent {cid} sent {}; this hub relays no agent loop",
                    guest_kind(&other)
                )),
            }
        }
    }

    /// Logs a guest line and parses it; an unparsable line drops the session.
    fn parse(&self, line: &str) -> Result<GuestMessage, HostError> {
        self.log.line(&format!("[guest {} vm{} ->] {}", self.cid, self.vm, sanitize_for_log(line)));
        GuestMessage::from_line(line).map_err(|err| violation(format!("unparsable guest line ({err}): {line:?}")))
    }

    /// Counts one tool call against the session's budget.
    fn charge(&self, calls: &mut u64, name: &str) -> Result<(), HostError> {
        let max = self.limits.max_calls;
        if *calls >= max {
            self.log.line(&format!(
                "vsockd: refusing a call ({}) for guest {}: the session's {max}-call budget is exhausted",
                sanitize_for_log(name),
                self.cid
            ));
            return Err(violation(format!("tool-call budget ({max}) exceeded")));
        }
        *calls += 1;
        Ok(())
    }

    fn send<S: Write>(&self, lines: &mut Lines<S>, message: &HostMessage) -> Result<(), HostError> {
        let line = message.to_line().map_err(|err| violation(format!("cannot encode a host line: {err}")))?;
        self.log.line(&format!("[<-] guest {} vm{} {line}", self.cid, self.vm));
        lines.send(&line)?;
        Ok(())
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

fn guest_kind(message: &GuestMessage) -> &'static str {
    match message {
        GuestMessage::Hello { .. } => "hello",
        GuestMessage::ToolCall { .. } => "tool_call",
        GuestMessage::ToolResult { .. } => "tool_result",
        GuestMessage::Heartbeat { .. } => "heartbeat",
        GuestMessage::Bye { .. } => "bye",
        GuestMessage::ModelCall { .. } => "model_call",
        GuestMessage::ToolRequest { .. } => "tool_request",
        GuestMessage::ToolReport { .. } => "tool_report",
        GuestMessage::WorkDone { .. } => "work_done",
    }
}

/// The v1 tools the hub runs for a guest; anything else is refused as data.
fn run_tool(name: &str, args: &serde_json::Value) -> (bool, String) {
    match name {
        "echo" => match args.get("text").and_then(serde_json::Value::as_str) {
            Some(text) => (true, text.to_string()),
            None => (false, r#"invalid arguments: echo needs a string "text""#.to_string()),
        },
        "uptime" => match host_uptime() {
            Ok(uptime) => (true, uptime),
            Err(err) => (false, format!("cannot read the host's /proc/uptime: {err}")),
        },
        other => (false, format!("unknown tool: {other}")),
    }
}

/// The first two fields of `/proc/uptime`.
fn host_uptime() -> io::Result<String> {
    let text = fs::read_to_string("/proc/uptime")?;
    match text.split_whitespace().take(2).collect::<Vec<_>>()[..] {
        [up, idle] => Ok(format!("{up} {idle}")),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "/proc/uptime has fewer than two fields")),
    }
}

/// Escapes control characters so a guest cannot forge or erase log lines.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::tests::hub_with;
    use crate::hub::{SendOutcome, StopOutcome, TellOutcome};
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::thread;

    const HELLO: &str = r#"{"type":"hello","vm":0,"guest_cid":3}"#;
    const KEEPALIVE_HELLO: &str = r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder","keepalive":true}"#;
    const KEEPALIVE_WELCOME: &str = r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#;

    fn v1_limits() -> SessionLimits {
        SessionLimits { max_calls: 64, deadline: None, keepalive_idle: None, keepalive_read_slice: Duration::from_secs(1) }
    }

    fn keepalive_limits() -> SessionLimits {
        SessionLimits {
            keepalive_idle: Some(Duration::from_secs(300)),
            keepalive_read_slice: Duration::from_millis(20),
            ..v1_limits()
        }
    }

    /// Runs a session against a guest that writes `script` and then closes
    /// (or, with `close: false`, goes silent). Returns the result, the log and
    /// every line the hub wrote.
    fn scripted(
        script: &[&str],
        close: bool,
        limits: SessionLimits,
        clock: &dyn Clock,
    ) -> (Result<SessionSummary, HostError>, String, Vec<String>) {
        let (mut guest, host) = UnixStream::pair().unwrap();
        for line in script {
            writeln!(guest, "{line}").unwrap();
        }
        if close {
            guest.shutdown(Shutdown::Write).unwrap();
        }
        let (log, hub) = hub_with(None);
        let result = handle_session(&mut Lines::new(host), 3, limits, &*log, &hub, clock);
        assert_eq!(hub.agent_count(), 0, "no session outlives its end");
        let mut sent = String::new();
        guest.read_to_string(&mut sent).unwrap();
        (result, log.text(), sent.lines().map(str::to_string).collect())
    }

    fn recv(guest: &mut Lines<UnixStream>) -> String {
        match guest.recv().unwrap() {
            Recv::Line(line) => line,
            other => panic!("expected a line, got {other:?}"),
        }
    }

    #[test]
    fn the_v1_session_serves_its_tools_and_refuses_others_as_data() {
        let (result, log, sent) = scripted(
            &[
                HELLO,
                r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza-guest-3-hello"}}"#,
                r#"{"type":"tool_call","call":2,"name":"uptime","args":{}}"#,
                r#"{"type":"tool_call","call":3,"name":"rm_rf","args":{}}"#,
                r#"{"type":"bye","calls":3}"#,
            ],
            true,
            v1_limits(),
            &SystemClock,
        );
        let summary = result.expect("the session completes");
        assert_eq!((summary.calls, summary.guest_calls, summary.keepalive), (3, 3, false));
        assert_eq!(sent[0], r#"{"type":"welcome","echo":"plaza-orchestrator"}"#);
        assert_eq!(sent[1], r#"{"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}"#);
        let uptime: serde_json::Value = serde_json::from_str(&sent[2]).unwrap();
        assert_eq!(uptime["output"].as_str().unwrap().split_whitespace().count(), 2, "{uptime}");
        assert_eq!(sent[3], r#"{"type":"tool_result","call":3,"ok":false,"output":"unknown tool: rm_rf"}"#);
        assert!(log.contains("[guest 3 vm? ->] {\"type\":\"hello\"") && log.contains("said bye after 3 tool calls"), "{log}");
    }

    #[test]
    fn protocol_violations_drop_the_session() {
        let budget = SessionLimits { max_calls: 1, ..v1_limits() };
        let past = SessionLimits { deadline: Some(Instant::now()), ..v1_limits() };
        let echo = r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"a"}}"#;
        let cases: [(&[&str], SessionLimits, &str); 9] = [
            (&[echo], v1_limits(), "tool_call before hello"),
            (&["not json"], v1_limits(), "unparsable guest line"),
            (&[r#"{"type":"hello","vm":0,"guest_cid":4}"#], v1_limits(), "claims guest_cid 4"),
            (&[HELLO, HELLO], v1_limits(), "second hello"),
            (&[HELLO, r#"{"type":"heartbeat","seq":1}"#], v1_limits(), "did not ask for keepalive"),
            (&[HELLO, echo, echo], budget, "tool-call budget (1) exceeded"),
            (&[HELLO], past, "session deadline exceeded"),
            (&[KEEPALIVE_HELLO], v1_limits(), "configured for one-shot sessions"),
            (&[], v1_limits(), "closed before sending hello"),
        ];
        for (script, limits, expected) in cases {
            let (result, _, _) = scripted(script, true, limits, &SystemClock);
            let err = result.expect_err(expected);
            assert!(matches!(&err, HostError::Protocol(detail) if detail.contains(expected)), "{expected}: {err:?}");
        }
    }

    #[test]
    fn control_characters_in_a_guest_line_are_escaped_in_the_log() {
        let (_, log, _) = scripted(&["not json\u{1b}[2K\rforged line"], true, v1_limits(), &SystemClock);
        assert!(log.contains("\\x1b") && log.contains("\\x0d"), "{log}");
        assert!(!log.contains('\u{1b}') && !log.contains('\r'), "{log}");
    }

    #[test]
    fn a_role_without_keepalive_gets_the_v1_welcome() {
        let script = [r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder"}"#, r#"{"type":"bye","calls":0}"#];
        let (result, _, sent) = scripted(&script, true, keepalive_limits(), &SystemClock);
        assert!(!result.expect("a v1 session").keepalive);
        assert_eq!(sent, [r#"{"type":"welcome","echo":"plaza-orchestrator"}"#]);
    }

    #[test]
    fn a_dropped_keepalive_connection_is_a_loss_and_a_silent_one_is_reaped() {
        let script = [KEEPALIVE_HELLO, r#"{"type":"heartbeat","seq":1}"#];
        let (result, _, sent) = scripted(&script, true, keepalive_limits(), &SystemClock);
        let err = result.expect_err("the connection dropped");
        assert!(err.to_string().starts_with("connection lost:"), "{err}");
        assert_eq!(sent, [KEEPALIVE_WELCOME]);

        /// Every reading is 100 s after the last.
        struct StepClock(Mutex<Instant>);
        impl Clock for StepClock {
            fn now(&self) -> Instant {
                let mut now = self.0.lock().unwrap();
                *now += Duration::from_secs(100);
                *now
            }
        }
        let clock = StepClock(Mutex::new(Instant::now()));
        let (result, log, _) = scripted(&[KEEPALIVE_HELLO], false, keepalive_limits(), &clock);
        assert!(matches!(result, Err(HostError::Reaped)), "{result:?}");
        assert!(log.contains("vsockd: agent 3 reaped idle > 300s"), "{log}");
    }

    /// The whole keepalive exchange over a real socket, steered through the
    /// hub the way the HTTP surface steers it.
    #[test]
    fn the_hub_steers_a_keepalive_session_end_to_end() {
        let (guest, host) = UnixStream::pair().unwrap();
        guest.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let (log, hub) = hub_with(None);
        let hub = Arc::new(hub);
        let session = {
            let (hub, log) = (Arc::clone(&hub), Arc::clone(&log));
            thread::spawn(move || handle_session(&mut Lines::new(host), 3, keepalive_limits(), &*log, &hub, &SystemClock))
        };
        let mut guest = Lines::new(guest);
        guest.send(KEEPALIVE_HELLO).unwrap();
        assert_eq!(recv(&mut guest), KEEPALIVE_WELCOME);
        guest.send(r#"{"type":"heartbeat","seq":1,"busy":true}"#).unwrap();

        let tell = |name: &'static str| {
            let hub = Arc::clone(&hub);
            thread::spawn(move || hub.tell(3, name, serde_json::json!({}), Duration::from_secs(5)))
        };
        let answered = tell("list");
        let dispatched: serde_json::Value = serde_json::from_str(&recv(&mut guest)).unwrap();
        assert_eq!((dispatched["type"].as_str(), dispatched["call"].as_u64()), (Some("tool_call"), Some(1)));
        guest.send(r#"{"type":"tool_result","call":1,"ok":true,"output":"[]"}"#).unwrap();
        assert_eq!(answered.join().unwrap(), TellOutcome::Answered { ok: true, output: "[]".into() });
        assert!(hub.snapshot().rows[0].busy);

        let cancel = HostMessage::Cancel { work: 7, reason: "operator".into() };
        assert_eq!(hub.send(3, cancel.clone(), Duration::from_secs(5)), SendOutcome::Sent);
        assert_eq!(recv(&mut guest), cancel.to_line().unwrap());

        let abandoned = tell("read_file");
        recv(&mut guest);
        let stopper = {
            let hub = Arc::clone(&hub);
            thread::spawn(move || hub.stop(3, "slice proof", Duration::from_secs(5)))
        };
        assert_eq!(recv(&mut guest), r#"{"type":"stop","reason":"slice proof"}"#);
        guest.send(r#"{"type":"bye","calls":2}"#).unwrap();
        assert_eq!(stopper.join().unwrap(), StopOutcome::Ended { calls: 2 });
        assert_eq!(abandoned.join().unwrap(), TellOutcome::NotConnected, "nobody is left to answer");
        assert_eq!(session.join().unwrap().expect("a clean end").calls, 2);
        assert!(log.text().contains("vsockd: agent 3 session ended cleanly after 2 tool calls"), "{}", log.text());
    }
}
