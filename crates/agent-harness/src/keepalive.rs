//! The keepalive session: the long-lived agent's half of protocol v2.
//!
//! One connection, one explicit sequence: `hello` (with the role and the
//! keepalive flag) → `welcome` (the hub's acceptance, which is this agent's
//! identity for its lifetime) → then, until the hub says otherwise, a
//! heartbeat every [`HEARTBEAT_SECS`] and a `tool_result` for every
//! `tool_call` the hub sends. The one thing this loop never does is decide:
//! it never opens a tool call, never picks an action and never ends a session
//! on its own account. `{"type":"stop"}` is the hub's decision, and the answer
//! to it is `bye`; anything else is a failure the caller reconnects from (see
//! [`backoff_delay`] and `main.rs`'s reconnect loop).
//!
//! The loop is driven by bounded waits, not by a read that may block forever:
//! it waits *at most* until its next heartbeat, and a wait that expires is
//! what makes the beat. That is also why the wait is a parameter
//! ([`session`]'s `heartbeat`): a test can shorten the clock to milliseconds
//! and observe the same schedule without sleeping for five seconds.
//!
//! Every wait's timeout is re-derived from the next beat, so a session that
//! is busy serving tool calls still beats on the interval, and a beat is never
//! sent late because a previous wait was long.

use std::fmt;
use std::io::Write;
use std::time::{Duration, Instant};

use crate::files::Workspace;
use crate::line::{LineTransport, Received};
use crate::proto::{GuestMessage, HostMessage, WELCOME_ECHO};
use crate::session::{
    Identity, MAX_SKIPPED_LINES, SessionError, escape_for_transcript, kind_of, note, send,
};
use crate::tools;

/// How often a keepalive agent beats, in seconds (`HEARTBEAT_SECS`).
pub const HEARTBEAT_SECS: u64 = 5;

/// The first reconnect delay, in milliseconds (the contract's 250 ms).
pub const BACKOFF_MIN_MS: u64 = 250;

/// The reconnect delay's cap, in milliseconds (the contract's 30 s).
pub const BACKOFF_MAX_MS: u64 = 30_000;

/// The heartbeat interval the shipped agent beats at.
pub fn heartbeat_interval() -> Duration {
    Duration::from_secs(HEARTBEAT_SECS)
}

/// Why one keepalive connection ended.
#[derive(Debug)]
pub struct KeepaliveError {
    /// Whether the hub had accepted this agent — a welcome it accepted as
    /// keepalive — before the connection ended.
    ///
    /// The reconnect backoff resets on a successful welcome, so this is what
    /// tells the caller to start counting again rather than backing off from a
    /// failure that no longer describes the hub.
    pub adopted: bool,
    /// What went wrong.
    pub error: SessionError,
}

impl KeepaliveError {
    /// A failure after the hub had adopted this agent.
    fn after_adoption(error: SessionError) -> Self {
        Self {
            adopted: true,
            error,
        }
    }
}

impl fmt::Display for KeepaliveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for KeepaliveError {}

impl From<SessionError> for KeepaliveError {
    fn from(error: SessionError) -> Self {
        Self {
            adopted: false,
            error,
        }
    }
}

/// Serves one keepalive connection: `hello` → `welcome` → beats and tool calls
/// until the hub stops the agent.
///
/// `heartbeat` is the interval between beats (the shipped agent uses
/// [`heartbeat_interval`]), and `workspace` is the volume the file tools work
/// in (the shipped agent uses [`Workspace::default`], the guest's
/// `/workspace`). Returns how many tool calls were served when the hub said
/// `stop`, whose `bye` is the last line this function sends. Every other
/// ending is an error; the caller reconnects.
pub fn session<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
    identity: &Identity,
    workspace: &Workspace,
    role: Option<&str>,
    heartbeat: Duration,
) -> Result<u64, KeepaliveError> {
    handshake(transport, log, identity, role)?;
    serve(transport, log, workspace, heartbeat).map_err(KeepaliveError::after_adoption)
}

/// Offers `hello` and waits for a `welcome` the hub accepted as keepalive.
///
/// Logs `agent-harness: role <role> accepted` for the identity the hub
/// confirmed (its own echo when it names one, otherwise the role this guest was
/// started as).
fn handshake<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
    identity: &Identity,
    role: Option<&str>,
) -> Result<(), KeepaliveError> {
    send(
        transport,
        log,
        &GuestMessage::Hello {
            vm: identity.vm,
            guest_cid: identity.guest_cid,
            role: role.map(str::to_string),
            keepalive: true,
        },
    )?;

    let mut skipped = 0usize;
    loop {
        let line = match receive(transport, log, None)? {
            Received::Line(line) => line,
            Received::Closed => return Err(SessionError::HostClosed.into()),
            // A wait with no deadline cannot expire; a transport that says it
            // did has not honoured the request, so treat it as a failure.
            Received::TimedOut => {
                return Err(SessionError::Protocol(
                    "the transport timed out a receive that had no deadline".to_string(),
                )
                .into());
            }
        };
        match HostMessage::from_line(&line) {
            Ok(HostMessage::Welcome {
                echo,
                role: hub_role,
                keepalive,
            }) => {
                if echo != WELCOME_ECHO {
                    return Err(SessionError::Protocol(format!(
                        "welcome echo {echo:?} is not {WELCOME_ECHO:?}"
                    ))
                    .into());
                }
                // A hub that does not confirm keepalive is not a v2 hub: this
                // agent refuses the session rather than guessing which of the
                // two protocols the rest of the connection will be.
                if keepalive != Some(true) {
                    return Err(SessionError::Protocol(format!(
                        "the hub did not confirm a keepalive session (welcome.keepalive is {})",
                        match keepalive {
                            Some(value) => value.to_string(),
                            None => "absent".to_string(),
                        }
                    ))
                    .into());
                }
                let accepted = hub_role.or_else(|| role.map(str::to_string));
                match accepted {
                    Some(role) => note(
                        log,
                        &format!(
                            "agent-harness: role {} accepted",
                            escape_for_transcript(&role)
                        ),
                    ),
                    None => note(log, "agent-harness: keepalive session accepted (no role)"),
                }
                return Ok(());
            }
            Ok(other) => {
                skipped += 1;
                note(
                    log,
                    &format!(
                        "agent-harness: ignoring {} while waiting for welcome",
                        kind_of(&other)
                    ),
                );
            }
            Err(err) => {
                skipped += 1;
                note(
                    log,
                    &format!("agent-harness: ignoring malformed host line ({err})"),
                );
            }
        }
        if skipped > MAX_SKIPPED_LINES {
            return Err(
                SessionError::Protocol(format!("no welcome within {skipped} lines")).into(),
            );
        }
    }
}

/// Beats and serves tool calls until the hub stops the agent.
fn serve<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
    workspace: &Workspace,
    heartbeat: Duration,
) -> Result<u64, SessionError> {
    let mut calls = 0u64;
    let mut seq = 0u64;
    // The first beat is one interval from the welcome, and every beat after it
    // is one interval from the last: an interval, not a burst.
    let mut next_beat = Instant::now() + heartbeat;
    let mut skipped = 0usize;

    loop {
        let wait = next_beat.saturating_duration_since(Instant::now());
        match receive(transport, log, Some(wait))? {
            Received::TimedOut => {
                seq += 1;
                send(transport, log, &GuestMessage::Heartbeat { seq })?;
                note(log, &format!("agent-harness: heartbeat {seq}"));
                next_beat = Instant::now() + heartbeat;
            }
            Received::Closed => return Err(SessionError::HostClosed),
            Received::Line(line) => match HostMessage::from_line(&line) {
                Ok(HostMessage::ToolCall { call, name, args }) => {
                    // Serving is progress: the flood bound counts *consecutive*
                    // lines that are neither a tool call nor a stop.
                    skipped = 0;
                    calls += 1;
                    let answer = match tools::serve(workspace, &name, &args) {
                        Ok(output) => GuestMessage::ToolResult {
                            call,
                            ok: true,
                            output,
                        },
                        Err(err) => {
                            // A refused tool is data: the hub learns which tool
                            // refused and why, and the session goes on.
                            note(
                                log,
                                &format!(
                                    "agent-harness: tool {} call {} refused: {}",
                                    escape_for_transcript(&name),
                                    call,
                                    err
                                ),
                            );
                            GuestMessage::ToolResult {
                                call,
                                ok: false,
                                output: err.to_string(),
                            }
                        }
                    };
                    send(transport, log, &answer)?;
                }
                Ok(HostMessage::Stop { reason }) => {
                    note(
                        log,
                        &format!(
                            "agent-harness: the hub stopped the agent ({})",
                            escape_for_transcript(reason.as_deref().unwrap_or("no reason given"))
                        ),
                    );
                    send(transport, log, &GuestMessage::Bye { calls })?;
                    return Ok(calls);
                }
                Ok(other) => {
                    skipped += 1;
                    note(
                        log,
                        &format!(
                            "agent-harness: ignoring {} while serving the hub",
                            kind_of(&other)
                        ),
                    );
                }
                Err(err) => {
                    skipped += 1;
                    note(
                        log,
                        &format!("agent-harness: ignoring malformed host line ({err})"),
                    );
                }
            },
        }
        if skipped > MAX_SKIPPED_LINES {
            return Err(SessionError::Protocol(format!(
                "no tool_call or stop within {skipped} lines"
            )));
        }
    }
}

/// Receives one line, waiting at most `wait`, and logs what arrived.
fn receive<T: LineTransport, W: Write>(
    transport: &mut T,
    log: &mut W,
    wait: Option<Duration>,
) -> Result<Received, SessionError> {
    match transport.recv_line_by(wait) {
        Ok(Received::Line(line)) => {
            note(log, &format!("[vsock<-] {}", escape_for_transcript(&line)));
            Ok(Received::Line(line))
        }
        Ok(other) => Ok(other),
        Err(err) => Err(SessionError::from(err)),
    }
}

/// The delay before reconnect attempt `failures` (1-based), with `jitter`
/// reducing it.
///
/// The nominal schedule is the contract's: [`BACKOFF_MIN_MS`] doubling up to
/// the [`BACKOFF_MAX_MS`] cap. The returned delay is that nominal value reduced
/// by up to an eighth, derived from `jitter`: a fleet of agents that lost the
/// same hub must not knock together, and pulling the delay *down* keeps the
/// fast path fast (a hub that restarts is adopted again within a few hundred
/// milliseconds) while still spreading the herd. It never exceeds the cap.
pub fn backoff_delay(failures: u32, jitter: u64) -> Duration {
    // 250 ms << 31 overflows u64; saturating keeps the arithmetic total, and
    // the cap makes everything past the seventh failure the same delay anyway.
    let nominal = BACKOFF_MIN_MS
        .saturating_mul(1u64 << failures.saturating_sub(1).min(31))
        .min(BACKOFF_MAX_MS);
    let spread = nominal / 8 + 1;
    Duration::from_millis(nominal - jitter % spread)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io;
    use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// A [`LineTransport`] over channels, so a test can put a scripted hub on
    /// the other end without a socket.
    ///
    /// It honours the receive deadline with `recv_timeout`, so the session
    /// under test beats when the hub is quiet, and it records every requested
    /// wait so a test can assert the cadence the session asked for.
    struct Channel {
        out: Sender<String>,
        input: Receiver<String>,
        waits: Arc<Mutex<Vec<Duration>>>,
    }

    impl LineTransport for Channel {
        fn send_line(&mut self, line: &str) -> io::Result<()> {
            self.out
                .send(line.to_string())
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "peer gone"))
        }

        fn recv_line_by(&mut self, timeout: Option<Duration>) -> io::Result<Received> {
            match timeout {
                Some(wait) => {
                    self.waits
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(wait);
                    match self.input.recv_timeout(wait) {
                        Ok(line) => Ok(Received::Line(line)),
                        Err(RecvTimeoutError::Timeout) => Ok(Received::TimedOut),
                        Err(RecvTimeoutError::Disconnected) => Ok(Received::Closed),
                    }
                }
                None => match self.input.recv() {
                    Ok(line) => Ok(Received::Line(line)),
                    Err(_) => Ok(Received::Closed),
                },
            }
        }
    }

    /// An in-process, full-duplex pair of transports.
    fn pair() -> (Channel, Channel, Arc<Mutex<Vec<Duration>>>) {
        let (guest_out, host_in) = channel();
        let (host_out, guest_in) = channel();
        let waits = Arc::new(Mutex::new(Vec::new()));
        (
            Channel {
                out: guest_out,
                input: guest_in,
                waits: Arc::clone(&waits),
            },
            Channel {
                out: host_out,
                input: host_in,
                waits: Arc::new(Mutex::new(Vec::new())),
            },
            waits,
        )
    }

    fn identity() -> Identity {
        Identity {
            guest_cid: 104,
            vm: 4,
        }
    }

    /// The workspace the session under test serves its file tools in. Only a
    /// test that touches the filesystem resolves it: the rest never do.
    fn workspace() -> Workspace {
        Workspace::default()
    }

    /// A heartbeat interval short enough that a test does not wait, but long
    /// enough that the schedule is a schedule: the scripted hub reads until it
    /// has what it wants, so the test is not racing the clock.
    const HEARTBEAT: Duration = Duration::from_millis(20);

    /// Receives one line from the guest, expecting it to be there.
    fn next(host: &mut Channel) -> String {
        host.recv_line()
            .expect("the guest sent a line")
            .expect("the connection is open")
    }

    /// Receives the guest's next line of `kind`, skipping beats.
    ///
    /// The agent beats whenever its own wait expires, which on a coarse clock
    /// (an emulated guest, a loaded host) can fall between any two lines the
    /// hub sends. A scripted hub that assumed its own traffic was always the
    /// next line would be asserting the test host's clock, not the protocol.
    fn next_kind(host: &mut Channel, kind: &str) -> String {
        loop {
            let line = next(host);
            let parsed: serde_json::Value =
                serde_json::from_str(&line).unwrap_or(serde_json::Value::Null);
            if parsed.get("type").and_then(serde_json::Value::as_str) == Some(kind) {
                return line;
            }
        }
    }

    /// The scripted hub from the contract: welcome the agent, take three
    /// beats, ask for `echo` and `uptime`, then stop it. Returns the lines it
    /// read from the guest that the contract names, in order.
    fn scripted_hub(mut host: Channel) -> Vec<String> {
        let hello = next(&mut host);
        host.send_line(
            r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
        )
        .expect("welcome sends");

        let mut beats = Vec::new();
        for _ in 0..3 {
            beats.push(next_kind(&mut host, "heartbeat"));
        }

        host.send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"hi there"}}"#)
            .expect("echo call sends");
        let echo = next_kind(&mut host, "tool_result");

        host.send_line(r#"{"type":"tool_call","call":2,"name":"uptime","args":{}}"#)
            .expect("uptime call sends");
        let uptime = next_kind(&mut host, "tool_result");

        host.send_line(r#"{"type":"stop","reason":"fleet shutdown"}"#)
            .expect("stop sends");
        let bye = next_kind(&mut host, "bye");

        let mut lines = vec![hello];
        lines.extend(beats);
        lines.push(echo);
        lines.push(uptime);
        lines.push(bye);
        lines
    }

    #[test]
    fn the_keepalive_session_beats_serves_and_ends_on_stop() {
        let (guest, host, waits) = pair();
        let hub = thread::spawn(move || scripted_hub(host));

        let mut guest = guest;
        let mut log = Vec::new();
        let calls = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        assert_eq!(calls, 2, "the agent served two tool calls");

        let lines = hub.join().expect("the hub thread finished");
        // The hello offers both v2 facts, and the beats are numbered from 1.
        assert_eq!(
            lines[0],
            r#"{"type":"hello","vm":4,"guest_cid":104,"role":"builder","keepalive":true}"#
        );
        assert_eq!(
            lines[1..4],
            [
                r#"{"type":"heartbeat","seq":1}"#.to_string(),
                r#"{"type":"heartbeat","seq":2}"#.to_string(),
                r#"{"type":"heartbeat","seq":3}"#.to_string(),
            ]
        );
        assert_eq!(
            lines[4],
            r#"{"type":"tool_result","call":1,"ok":true,"output":"hi there"}"#
        );
        let uptime: serde_json::Value = serde_json::from_str(&lines[5]).expect("uptime is JSON");
        assert_eq!(uptime["type"], "tool_result");
        assert_eq!(uptime["call"], 2);
        assert_eq!(uptime["ok"], true);
        assert_eq!(
            uptime["output"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .count(),
            2
        );
        assert_eq!(lines[6], r#"{"type":"bye","calls":2}"#);

        // The transcript is in protocol order, and it carries the v2 lines the
        // contract names.
        let log = String::from_utf8(log).expect("the transcript is UTF-8");
        let positions: Vec<usize> = [
            "[vsock->] {\"type\":\"hello\"",
            "[vsock<-] {\"type\":\"welcome\"",
            "agent-harness: role builder accepted",
            "[vsock->] {\"type\":\"heartbeat\",\"seq\":1}",
            "agent-harness: heartbeat 1",
            "[vsock->] {\"type\":\"heartbeat\",\"seq\":3}",
            "[vsock<-] {\"type\":\"tool_call\",\"call\":1",
            "[vsock->] {\"type\":\"tool_result\",\"call\":1",
            "[vsock<-] {\"type\":\"stop\"",
            "[vsock->] {\"type\":\"bye\",\"calls\":2}",
        ]
        .iter()
        .map(|needle| {
            log.find(needle)
                .unwrap_or_else(|| panic!("{needle} is in the transcript"))
        })
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "the transcript is in protocol order:\n{log}"
        );

        // No beat waited longer than the interval: the cadence is the
        // interval, not luck. (The wait is recomputed from the next beat, so
        // it is always at most one interval.)
        let waits = waits.lock().expect("the wait log");
        assert!(waits.len() >= 3, "the session waited {} times", waits.len());
        assert!(
            waits.iter().all(|wait| *wait <= HEARTBEAT),
            "a wait exceeded the heartbeat interval: {waits:?}"
        );
    }

    #[test]
    fn an_unknown_tool_is_answered_as_data_and_the_session_goes_on() {
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"qa","keepalive":true}"#,
            )
            .expect("welcome sends");
            host.send_line(r#"{"type":"tool_call","call":1,"name":"exec","args":{"cmd":"id"}}"#)
                .expect("the unknown tool call sends");
            let answer = next_kind(&mut host, "tool_result");
            host.send_line(r#"{"type":"stop"}"#).expect("stop sends");
            let bye = next_kind(&mut host, "bye");
            (answer, bye)
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let calls = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("qa"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        assert_eq!(calls, 1, "a refused call is still a call served");

        let (answer, bye) = hub.join().expect("the hub thread finished");
        assert_eq!(
            answer,
            r#"{"type":"tool_result","call":1,"ok":false,"output":"no such tool: exec"}"#
        );
        assert_eq!(bye, r#"{"type":"bye","calls":1}"#);
    }

    #[test]
    fn a_welcome_without_a_role_falls_back_to_the_command_line_role() {
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(r#"{"type":"welcome","echo":"plaza-orchestrator","keepalive":true}"#)
                .expect("welcome sends");
            host.send_line(r#"{"type":"stop"}"#).expect("stop sends");
            let _bye = next(&mut host);
        });

        let mut guest = guest;
        let mut log = Vec::new();
        session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("auditor"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        hub.join().expect("the hub thread finished");
        let log = String::from_utf8(log).expect("the transcript is UTF-8");
        assert!(
            log.contains("agent-harness: role auditor accepted"),
            "{log}"
        );
    }

    #[test]
    fn a_hub_that_does_not_confirm_keepalive_is_refused_not_guessed_at() {
        // A v1 hub's welcome (echo only) is not a v2 session: the agent
        // refuses it rather than guessing which protocol the rest of the
        // connection is, and the caller reconnects (the hub is not adopted).
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .expect("welcome sends");
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let err = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect_err("a v1 welcome is not a keepalive session");
        assert!(!err.adopted, "the agent was never adopted");
        assert!(
            matches!(err.error, SessionError::Protocol(_)),
            "got {:?}",
            err.error
        );
        assert!(
            err.to_string()
                .contains("did not confirm a keepalive session")
        );
        hub.join().expect("the hub thread finished");
    }

    #[test]
    fn a_wrong_welcome_echo_is_a_protocol_error() {
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"someone-else","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let err = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect_err("the echo is the contract's");
        assert!(!err.adopted);
        assert!(matches!(err.error, SessionError::Protocol(_)));
        hub.join().expect("the hub thread finished");
    }

    #[test]
    fn a_connection_lost_after_adoption_is_reported_as_adopted() {
        // The backoff resets on a successful welcome, so a drop *after* one
        // must say the agent had been adopted.
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
            // And away it goes, without a stop.
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let err = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect_err("the peer went away mid-session");
        assert!(err.adopted);
        assert!(matches!(err.error, SessionError::HostClosed), "got {err:?}");
        hub.join().expect("the hub thread finished");
    }

    #[test]
    fn welcome_repeats_and_stale_results_are_skipped_not_fatal() {
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(r#"not json at all"#).expect("garbage sends");
            // A v1-shaped result, for a call this agent never made.
            host.send_line(r#"{"type":"tool_result","call":9,"ok":true,"output":"stale"}"#)
                .expect("stale result sends");
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
            host.send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"a"}}"#)
                .expect("echo call sends");
            let echo = next_kind(&mut host, "tool_result");
            // A second welcome in the middle of the session is not fatal.
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
            host.send_line(r#"{"type":"stop"}"#).expect("stop sends");
            let bye = next_kind(&mut host, "bye");
            (echo, bye)
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let calls = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        assert_eq!(calls, 1);
        let (echo, bye) = hub.join().expect("the hub thread finished");
        assert_eq!(
            echo,
            r#"{"type":"tool_result","call":1,"ok":true,"output":"a"}"#
        );
        assert_eq!(bye, r#"{"type":"bye","calls":1}"#);
        let log = String::from_utf8(log).expect("the transcript is UTF-8");
        assert!(
            log.contains("ignoring welcome while serving the hub"),
            "{log}"
        );
        assert!(log.contains("ignoring malformed host line"), "{log}");
    }

    #[test]
    fn a_peer_that_only_sends_garbage_is_failed_not_looped_on() {
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
            // The agent fails the session after MAX_SKIPPED_LINES lines and
            // drops its end, so the last sends may fail; that is the agent
            // stopping, not a test failure.
            for _ in 0..MAX_SKIPPED_LINES + 2 {
                let _ = host.send_line("garbage");
            }
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let err = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect_err("a peer that only sends garbage is a protocol failure");
        assert!(err.adopted);
        assert!(
            matches!(err.error, SessionError::Protocol(_)),
            "got {err:?}"
        );
        hub.join().expect("the hub thread finished");
    }

    #[test]
    fn the_backoff_is_the_contracts_schedule() {
        use std::time::Duration as Wait;
        // The nominal schedule: 250 ms doubling to a 30 s cap.
        assert_eq!(backoff_delay(0, 0), Wait::from_millis(250));
        assert_eq!(backoff_delay(1, 0), Wait::from_millis(250));
        assert_eq!(backoff_delay(2, 0), Wait::from_millis(500));
        assert_eq!(backoff_delay(3, 0), Wait::from_millis(1_000));
        assert_eq!(backoff_delay(7, 0), Wait::from_millis(16_000));
        assert_eq!(backoff_delay(8, 0), Wait::from_millis(30_000));
        // Past the cap it stays at the cap, however many failures pile up.
        for failures in [9, 32, 64, u32::MAX] {
            assert_eq!(backoff_delay(failures, 0), Wait::from_millis(30_000));
        }

        // Jitter only ever pulls the delay down, never past the cap, and never
        // below seven eighths of the nominal value (so the fast path stays
        // fast: the first reconnect is ~220-250 ms, not seconds).
        for failures in [1u32, 2, 3, 8, 40] {
            for jitter in [0u64, 1, 7, 1000, u64::MAX] {
                let nominal = backoff_delay(failures, 0).as_millis() as u64;
                let delay = backoff_delay(failures, jitter).as_millis() as u64;
                assert!(delay <= BACKOFF_MAX_MS, "{failures}/{jitter}: {delay}");
                assert!(
                    delay >= nominal - nominal / 8 - 1 && delay <= nominal,
                    "{failures}/{jitter}: {delay} is outside [{}, {nominal}]",
                    nominal - nominal / 8 - 1
                );
            }
        }
        // Different jitter decorrelates the herd: two agents that lost the same
        // hub do not reconnect on the same millisecond by construction.
        assert_ne!(backoff_delay(1, 5), backoff_delay(1, 9));
    }

    #[test]
    fn the_heartbeat_interval_is_the_contracts_five_seconds() {
        assert_eq!(HEARTBEAT_SECS, 5);
        assert_eq!(heartbeat_interval(), Duration::from_secs(5));
    }

    #[test]
    fn the_heartbeat_seq_is_monotonic_across_a_long_quiet_stretch() {
        // The hub says nothing at all: the agent keeps beating, numbered, and
        // only the hub's stop (or a failure) ends it. This is the "never exits
        // on its own account" rule at the loop's level.
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
            let mut beats = Vec::new();
            for _ in 0..5 {
                beats.push(next(&mut host));
            }
            host.send_line(r#"{"type":"stop"}"#).expect("stop sends");
            let _bye = next(&mut host);
            beats
        });

        let mut guest = guest;
        let mut log = Vec::new();
        session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        let beats = hub.join().expect("the hub thread finished");
        assert_eq!(
            beats,
            (1..=5)
                .map(|seq| format!(r#"{{"type":"heartbeat","seq":{seq}}}"#))
                .collect::<Vec<String>>()
        );
    }

    #[test]
    fn a_tool_call_with_bad_arguments_is_a_refusal_not_a_disconnect() {
        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
            host.send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{}}"#)
                .expect("echo call sends");
            let answer = next_kind(&mut host, "tool_result");
            host.send_line(r#"{"type":"stop"}"#).expect("stop sends");
            let _bye = next_kind(&mut host, "bye");
            answer
        });

        let mut guest = guest;
        let mut log = Vec::new();
        session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        let answer = hub.join().expect("the hub thread finished");
        let answer: serde_json::Value = serde_json::from_str(&answer).expect("a JSON line");
        assert_eq!(answer["ok"], false);
        assert!(
            answer["output"]
                .as_str()
                .unwrap_or_default()
                .contains("invalid input"),
            "{answer}"
        );
    }

    #[test]
    fn a_jittered_wait_never_exceeds_the_interval() {
        // The loop's wait is derived from the next beat, so it can only ever
        // be *at most* one interval: this is the property the cadence rests on.
        let (guest, mut host, waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");
            let _beat = next_kind(&mut host, "heartbeat");
            host.send_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"x"}}"#)
                .expect("echo call sends");
            let _answer = next_kind(&mut host, "tool_result");
            let _beat2 = next_kind(&mut host, "heartbeat");
            host.send_line(r#"{"type":"stop"}"#).expect("stop sends");
            let _bye = next_kind(&mut host, "bye");
        });

        let mut guest = guest;
        let mut log = Vec::new();
        session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace(),
            Some("builder"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        hub.join().expect("the hub thread finished");
        let waits = waits.lock().expect("the wait log");
        assert!(!waits.is_empty());
        // The property is the ceiling, not a floor: on a coarse clock (an
        // emulated guest) the beat can already be due, and a zero wait is the
        // beat, not a burst.
        assert!(waits.iter().all(|wait| *wait <= HEARTBEAT), "{waits:?}");
        // And a served call does not swallow the next beat.
        let log = String::from_utf8(log).expect("the transcript is UTF-8");
        assert!(log.contains("agent-harness: heartbeat 1"), "{log}");
        assert!(log.contains("agent-harness: heartbeat 2"), "{log}");
    }

    #[test]
    fn the_file_tools_are_served_over_the_wire_and_refuse_escapes() {
        // The hub drives the file tools, and the social channel goes through
        // the same dispatch: every call is one `tool_call` and one
        // `tool_result` whose `output` is the tool's own JSON, and a refused
        // call is `ok:false` data on the same connection, not a disconnect.
        let dir = std::env::temp_dir().join(format!("apz-harness-wire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        let workspace = Workspace::at(&dir);

        let (guest, mut host, _waits) = pair();
        let hub = thread::spawn(move || {
            let _hello = next(&mut host);
            host.send_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#,
            )
            .expect("welcome sends");

            let calls = [
                r#"{"type":"tool_call","call":1,"name":"write_file","args":{"path":"notes.txt","content":"one\ntwo\n"}}"#,
                r#"{"type":"tool_call","call":2,"name":"read_file","args":{"path":"notes.txt"}}"#,
                r#"{"type":"tool_call","call":3,"name":"list","args":{}}"#,
                r#"{"type":"tool_call","call":4,"name":"search","args":{"pattern":"two"}}"#,
                r#"{"type":"tool_call","call":5,"name":"edit_file","args":{"path":"notes.txt","find":"two","replace":"TWO"}}"#,
                r#"{"type":"tool_call","call":6,"name":"read_file","args":{"path":"notes.txt"}}"#,
                r#"{"type":"tool_call","call":7,"name":"append_file","args":{"path":"notes.txt","content":"three\n"}}"#,
                r#"{"type":"tool_call","call":8,"name":"read_file","args":{"path":"../escape"}}"#,
                r#"{"type":"tool_call","call":9,"name":"read_file","args":{"path":"/etc/passwd"}}"#,
                r#"{"type":"tool_call","call":10,"name":"conversation","args":{"from_role":"builder","from_cid":100,"text":"hello from the hub"}}"#,
            ];
            let mut answers = Vec::new();
            for call in calls {
                host.send_line(call).expect("the call sends");
                answers.push(next_kind(&mut host, "tool_result"));
            }
            host.send_line(r#"{"type":"stop","reason":"done"}"#)
                .expect("stop sends");
            let bye = next_kind(&mut host, "bye");
            (answers, bye)
        });

        let mut guest = guest;
        let mut log = Vec::new();
        let calls = session(
            &mut guest,
            &mut log,
            &identity(),
            &workspace,
            Some("builder"),
            HEARTBEAT,
        )
        .expect("the hub stops the agent cleanly");
        assert_eq!(calls, 10, "every call is served, refused or not");

        let (answers, bye) = hub.join().expect("the hub thread finished");
        // Each answer is the wire's shape: the call it answers, its `ok`, and
        // the tool's output as a string.
        let parsed: Vec<serde_json::Value> = answers
            .iter()
            .enumerate()
            .map(|(index, answer)| {
                let answer: serde_json::Value = serde_json::from_str(answer).expect("a JSON line");
                assert_eq!(answer["type"], "tool_result", "{answer}");
                assert_eq!(answer["call"], index as u64 + 1, "{answer}");
                answer
            })
            .collect();
        // The `output` is the tool's own JSON: the wire escapes it, and the
        // hub parses it back.
        let payload = |index: usize| -> serde_json::Value {
            let output = parsed[index]["output"].as_str().expect("an output");
            serde_json::from_str(output).expect("a tool's output is JSON")
        };
        assert_eq!(parsed[0]["ok"], true);
        assert_eq!(payload(0)["path"], "notes.txt");
        assert_eq!(payload(0)["created"], true);
        assert_eq!(payload(1)["content"], "one\ntwo\n");
        assert_eq!(payload(1)["bytes"], 8);
        assert_eq!(payload(1)["truncated"], false);
        assert_eq!(payload(2)["path"], ".");
        assert_eq!(payload(2)["entries"][0]["name"], "notes.txt");
        assert_eq!(payload(2)["entries"][0]["kind"], "file");
        assert_eq!(payload(3)["matches"][0]["file"], "notes.txt");
        assert_eq!(payload(3)["matches"][0]["line"], 2);
        assert_eq!(payload(3)["truncated"], false);
        assert_eq!(payload(4)["matches"], 1);
        assert_eq!(payload(5)["content"], "one\nTWO\n");
        assert_eq!(payload(6)["size"], 14);
        // The escapes are refusals with the reason, on the same connection.
        assert_eq!(parsed[7]["ok"], false);
        assert!(
            parsed[7]["output"]
                .as_str()
                .unwrap_or_default()
                .starts_with("denied: "),
            "{}",
            parsed[7]
        );
        assert_eq!(parsed[8]["ok"], false);
        assert!(
            parsed[8]["output"]
                .as_str()
                .unwrap_or_default()
                .contains("absolute"),
            "{}",
            parsed[8]
        );
        // The social channel through the same dispatch: the call is served as
        // an ordinary tool, and the line lands in the agent's own log.
        assert_eq!(parsed[9]["ok"], true);
        assert_eq!(payload(9)["received"], true);
        assert_eq!(payload(9)["log"], "messages.log");
        assert_eq!(payload(9)["lines"], 1);
        let messages = std::fs::read_to_string(dir.join("messages.log")).expect("the log");
        let (stamp, rest) = messages
            .trim_end()
            .split_once(' ')
            .expect("a stamp then the message");
        assert!(stamp.ends_with('Z') && stamp.contains('T'), "{stamp:?}");
        assert_eq!(rest, "from builder (cid 100): hello from the hub");
        assert_eq!(bye, r#"{"type":"bye","calls":10}"#);

        // The transcript carries the calls and the refusal in protocol order.
        let log = String::from_utf8(log).expect("the transcript is UTF-8");
        assert!(
            log.contains(r#"[vsock<-] {"type":"tool_call","call":1,"name":"write_file""#),
            "{log}"
        );
        assert!(
            log.contains("agent-harness: tool read_file call 8 refused: denied: "),
            "{log}"
        );
        let (call, refusal) = (
            log.find(r#"[vsock->] {"type":"tool_result","call":8,"ok":false"#),
            log.find("agent-harness: tool read_file call 8 refused"),
        );
        assert!(call.is_some() && refusal.is_some(), "{log}");

        // And the volume holds what the calls wrote.
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.txt")).expect("the file"),
            "one\nTWO\nthree\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
