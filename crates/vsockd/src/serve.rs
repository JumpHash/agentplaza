//! The host side of one guest session, chosen by its `hello`:
//!
//! * **v1** — welcome, the guest's tool calls (`echo`, `uptime`), `bye`.
//! * **keepalive** — registered with the [`Hub`]; the hub's commands go out
//!   as `tool_call`s, raw lines or `stop`, heartbeats keep it alive, and it is
//!   reaped after [`SessionLimits::keepalive_idle`] without one. Its hello asks
//!   apid for the agent's profile (the welcome), and its v3 lines are forwarded
//!   to apid on threads, at most [`MAX_FORWARDS`] at once. Every reply, and the
//!   refusal that stands in for a failed forward, is written here: this thread
//!   is the guest's only writer.
//!
//! A tool that cannot run answers `ok: false`; a line that is not a valid
//! guest message drops the session. The CID is the socket's, never the line's.

use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::line::{Lines, ReadTimeout, Recv};
use apz_proto::wire::{AgentProfile, GuestMessage, HostFacts, HostMessage};

use crate::apid::{self, Apid};
use crate::hub::{Command, Hub, ToolReply};
use crate::waker::Waker;

/// Forwards to apid in flight per session; the next one is refused at once.
pub const MAX_FORWARDS: usize = 4;
/// How often a keepalive session without a profile asks apid again.
const HELLO_RETRY: Duration = Duration::from_secs(10);

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

/// The longest a quiet keepalive session (a dozing VM: paused, silent) waits between passes.
const QUIET_MAX_WAIT: Duration = Duration::from_secs(60);

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
    /// The longest a keepalive session waits for its guest, a hub command or an apid answer before it rechecks its deadlines.
    pub keepalive_max_wait: Duration,
}

/// A guest stream a session can wait on together with its waker.
pub trait WaitReady {
    /// Waits at most `timeout` until the stream is readable (or closed) or `waker` was woken; `Ok(true)` when the
    /// stream is readable.
    fn wait_ready(&self, waker: &Waker, timeout: Duration) -> io::Result<bool>;
}

impl WaitReady for apz_uds::UnixStream {
    fn wait_ready(&self, waker: &Waker, timeout: Duration) -> io::Result<bool> {
        #[cfg(unix)]
        let own = {
            use std::os::fd::AsRawFd;
            self.as_raw_fd()
        };
        #[cfg(windows)]
        let own = {
            use std::os::windows::io::AsRawSocket;
            self.as_raw_socket()
        };
        let ready = crate::sys::wait_readable(&[own, waker.handle()], Some(timeout))?;
        Ok(ready[0])
    }
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
pub fn handle_session<S: Read + Write + ReadTimeout + WaitReady>(
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
    log.line(&format!(
        "[guest {peer_cid} vm? ->] {}",
        sanitize_for_log(&line)
    ));
    let (vm, role, keepalive, caps) = match GuestMessage::from_line(&line) {
        Ok(GuestMessage::Hello {
            vm,
            guest_cid,
            role,
            keepalive,
            caps,
        }) => {
            // Identity is the socket path's; a guest claiming another CID is dropped.
            if guest_cid != peer_cid {
                return Err(violation(format!(
                    "hello claims guest_cid {guest_cid}, but the connection was accepted on the socket for cid {peer_cid}"
                )));
            }
            log.line(&format!("vsockd: guest {peer_cid} vm {vm} says hello (accepted on the socket for cid {peer_cid})"));
            (vm, role, keepalive == Some(true), caps)
        }
        Ok(other) => return Err(violation(format!("{} before hello", guest_kind(&other)))),
        Err(err) => {
            return Err(violation(format!(
                "unparsable guest line ({err}): {line:?}"
            )));
        }
    };
    let session = Session {
        cid: peer_cid,
        vm,
        limits,
        log,
    };
    if !keepalive {
        session.send(lines, &HostMessage::welcome(None, None, None))?;
        return session.v1(lines);
    }
    let Some(idle) = limits.keepalive_idle else {
        return Err(violation(
            "a keepalive hello on a daemon configured for one-shot sessions only",
        ));
    };
    lines
        .get_ref()
        .set_read_timeout(Some(limits.keepalive_read_slice))?;
    let outcome = session.keepalive(
        lines,
        role.as_deref(),
        &caps,
        idle,
        limits.keepalive_max_wait,
        hub,
        clock,
    );
    session.gone(hub, &outcome);
    outcome
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
            if self
                .limits
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
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
                    self.log.line(&format!(
                        "vsockd: answered call {call} ({}) for guest {cid} vm {vm}",
                        sanitize_for_log(&name)
                    ));
                }
                GuestMessage::Bye { calls: guest_calls } => {
                    if guest_calls != calls {
                        self.log.line(&format!("vsockd: guest {cid} vm {vm} counted {guest_calls} tool calls, this side counted {calls}"));
                    }
                    self.log.line(&format!(
                        "vsockd: guest {cid} vm {vm} said bye after {calls} tool calls"
                    ));
                    return Ok(SessionSummary {
                        guest_cid: cid,
                        vm,
                        calls,
                        guest_calls,
                        keepalive: false,
                    });
                }
                GuestMessage::Hello { .. } => {
                    return Err(violation(format!(
                        "a second hello from guest {cid} vm {vm}"
                    )));
                }
                other => {
                    return Err(violation(format!(
                        "{} in a session that did not ask for keepalive",
                        guest_kind(&other)
                    )));
                }
            }
        }
    }

    /// The keepalive body: the hub's commands out, the agent's lines in, until
    /// its `bye`, a drop, or the idle reap.
    fn keepalive<S: Read + Write + WaitReady>(
        &self,
        lines: &mut Lines<S>,
        role: Option<&str>,
        caps: &[String],
        idle: Duration,
        max_wait: Duration,
        hub: &Hub,
        clock: &dyn Clock,
    ) -> Result<SessionSummary, HostError> {
        let cid = self.cid;
        let mut last_heartbeat = clock.now();
        let apid = hub.apid();
        // A hello that names its role registers first, as it always has. One without a role
        // (a guest restored from a warm template boots without `plaza.role=`) asks apid first:
        // the profile names the role the agent is registered under, and with no profile there
        // is no role to register it with, so the session is refused rather than registered
        // with an empty one.
        let (token, steering, waker, role, profile, mut next_hello) = match role
            .filter(|r| !r.is_empty())
        {
            Some(role) => {
                let (token, steering, waker) = hub.connect(cid, role, last_heartbeat)?;
                let (profile, next_hello) = self.ask_apid(&apid, role, caps, clock);
                (
                    token,
                    steering,
                    waker,
                    role.to_string(),
                    profile,
                    next_hello,
                )
            }
            None => {
                let (profile, _) = self.ask_apid(&apid, "", caps, clock);
                let role = match &profile {
                    Some(found) if !found.role.is_empty() => found.role.clone(),
                    _ => {
                        return Err(violation(
                            "a keepalive hello without a role needs the hub to name the agent's role",
                        ));
                    }
                };
                let (token, steering, waker) = hub.connect(cid, &role, last_heartbeat)?;
                (token, steering, waker, role, profile, None)
            }
        };
        // Registered before the welcome; the guard unregisters on every exit.
        let _registered = Registration { hub, cid, token };
        let (settled_tx, settled) = channel();
        let bridge = Bridge {
            apid: Arc::clone(&apid),
            in_flight: Arc::new(AtomicUsize::new(0)),
            settled_tx,
            settled,
            waker: Arc::clone(&waker),
        };
        // The welcome carries the host's clock and fresh entropy, so a guest restored from a
        // snapshot resumes with the right time and its own random stream.
        self.send(
            lines,
            &HostMessage::welcome(Some(role.clone()), Some(true), profile)
                .with_host(HostFacts::now()),
        )?;

        let mut calls = 0u64;
        // Dispatched tool calls not yet answered, and who waits on each.
        let mut outstanding: Vec<(u64, Option<Sender<ToolReply>>)> = Vec::new();
        let mut stopping = false;
        // Whether the previous pass found the session quiet.
        let mut was_quiet = false;
        loop {
            // A quiet agent (a dozing VM, paused, so it sends no heartbeats) is not reaped for its
            // silence: while it is quiet its idle window restarts on every pass, and a full `idle`
            // window starts the pass it stops being quiet.
            let quiet = hub.is_quiet(cid, token);
            if quiet || was_quiet {
                last_heartbeat = clock.now();
            }
            was_quiet = quiet;
            if !quiet && clock.now().saturating_duration_since(last_heartbeat) > idle {
                hub.reap_idle(cid, token, idle);
                return Err(HostError::Reaped);
            }
            // apid's answers, written here: this thread is the guest's only writer.
            while let Ok(done) = bridge.settled.try_recv() {
                self.log.line(&format!("vsockd: agent {cid} {}", done.note));
                if let Some(line) = done.line {
                    if matches!(line, HostMessage::Profile { .. }) {
                        next_hello = None;
                    }
                    self.send(lines, &line)?;
                }
            }
            if next_hello.is_some_and(|due| clock.now() >= due) {
                next_hello = Some(clock.now() + HELLO_RETRY);
                bridge.hello(cid, &role, self.vm, caps);
            }
            // Once a stop is out, nothing more is dispatched.
            while !stopping && let Ok(command) = steering.try_recv() {
                match command {
                    Command::Call { name, args, reply } => {
                        self.charge(&mut calls, &name)?;
                        hub.note_calls(cid, calls);
                        outstanding.push((calls, reply));
                        self.send(
                            lines,
                            &HostMessage::ToolCall {
                                call: calls,
                                name,
                                args,
                            },
                        )?;
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
            // Sleep until the guest, a hub command or an apid answer wakes the session, or a deadline is due.
            if !lines.pending() {
                let mut until = last_heartbeat + idle;
                if let Some(due) = next_hello {
                    until = until.min(due);
                }
                // A quiet (dozing) agent has no deadline to recheck: the flag's change, a hub command or an
                // apid answer wakes the session, so it waits much longer between passes.
                let cap = if quiet { QUIET_MAX_WAIT } else { max_wait };
                let wait = until
                    .saturating_duration_since(clock.now())
                    .clamp(Duration::from_millis(1), cap);
                let readable = lines.get_ref().wait_ready(&waker, wait)?;
                waker.drain();
                crate::metrics::SESSION_WAKEUPS.inc();
                if !readable {
                    continue;
                }
            }
            let line = match lines.recv()? {
                Recv::Line(line) => line,
                Recv::Idle => continue,
                Recv::Closed if stopping => {
                    return Err(HostError::Dropped(
                        "the agent dropped after a stop, before its bye".into(),
                    ));
                }
                Recv::Closed => {
                    return Err(HostError::Dropped(
                        "the agent's connection dropped; it reconnects with backoff".into(),
                    ));
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
                    self.log.line(&format!(
                        "vsockd: answered call {call} ({}) for agent {cid}",
                        sanitize_for_log(&name)
                    ));
                }
                GuestMessage::Bye { calls: guest_calls } => {
                    if guest_calls != calls {
                        self.log.line(&format!("vsockd: agent {cid} counted {guest_calls} tool calls, this side counted {calls}"));
                    }
                    hub.end_clean(cid, token, calls, guest_calls);
                    return Ok(SessionSummary {
                        guest_cid: cid,
                        vm: self.vm,
                        calls,
                        guest_calls,
                        keepalive: true,
                    });
                }
                GuestMessage::Hello { .. } => {
                    return Err(violation(format!("a second hello from agent {cid}")));
                }
                message @ (GuestMessage::ModelCall { .. }
                | GuestMessage::ToolRequest { .. }
                | GuestMessage::EngineUpdate { .. }
                | GuestMessage::ToolReport { .. }
                | GuestMessage::WorkDone { .. }
                | GuestMessage::Compressed { .. }) => {
                    self.forward(lines, &bridge, &line, message)?
                }
            }
        }
    }

    /// Asks apid for the agent's profile. Without one (apid off, down, or none for this agent)
    /// the second value is when the hub asks again, every `HELLO_RETRY`, until one comes.
    fn ask_apid(
        &self,
        apid: &Apid,
        role: &str,
        caps: &[String],
        clock: &dyn Clock,
    ) -> (Option<AgentProfile>, Option<Instant>) {
        if !apid.enabled() {
            return (None, None);
        }
        let profile = match apid.hello(self.cid, role, self.vm, caps) {
            Ok(found) => found,
            Err(err) => {
                self.log.line(&format!(
                    "vsockd: agent {} hello to apid failed: {err}",
                    self.cid
                ));
                None
            }
        };
        let next_hello = profile.is_none().then(|| clock.now() + HELLO_RETRY);
        (profile, next_hello)
    }

    /// Hands one v3 line to apid on a thread, so the session keeps reading. Over
    /// the cap, a line that expects an answer is refused at once.
    fn forward<S: Write>(
        &self,
        lines: &mut Lines<S>,
        bridge: &Bridge,
        line: &str,
        message: GuestMessage,
    ) -> Result<(), HostError> {
        let (cid, kind) = (self.cid, guest_kind(&message));
        if bridge.in_flight.load(Ordering::SeqCst) >= MAX_FORWARDS {
            let reason =
                format!("the hub already has {MAX_FORWARDS} forwards in flight for agent {cid}");
            self.log
                .line(&format!("vsockd: agent {cid} {kind} refused: {reason}"));
            if let Some(reply) = apid::refusal(&message, &reason) {
                self.send(lines, &reply)?;
            }
            return Ok(());
        }
        bridge.in_flight.fetch_add(1, Ordering::SeqCst);
        let (apid, in_flight, tx, waker, line) = (
            Arc::clone(&bridge.apid),
            Arc::clone(&bridge.in_flight),
            bridge.settled_tx.clone(),
            Arc::clone(&bridge.waker),
            line.to_string(),
        );
        thread::spawn(move || {
            let done = match apid.message(cid, &line) {
                Ok(Some(reply)) => Settled {
                    note: format!("apid answered {kind}"),
                    line: Some(reply),
                },
                Ok(None) => Settled {
                    note: format!("apid sent no reply to {kind}"),
                    line: apid::refusal(&message, "apid sent no reply"),
                },
                Err(err) => Settled {
                    note: format!("apid failed {kind}: {err}"),
                    line: apid::refusal(&message, &err),
                },
            };
            in_flight.fetch_sub(1, Ordering::SeqCst);
            let _ = tx.send(done);
            waker.wake();
        });
        Ok(())
    }

    /// Posts the session-end notice to apid (best effort), whatever ended it.
    fn gone(&self, hub: &Hub, outcome: &Result<SessionSummary, HostError>) {
        let apid = hub.apid();
        if !apid.enabled() {
            return;
        }
        let reason = match outcome {
            Ok(_) => "bye".to_string(),
            Err(err) => err.to_string(),
        };
        if let Err(err) = apid.gone(self.cid, &reason) {
            self.log.line(&format!(
                "vsockd: agent {} gone notice to apid failed: {err}",
                self.cid
            ));
        }
    }

    /// Logs a guest line and parses it; an unparsable line drops the session.
    fn parse(&self, line: &str) -> Result<GuestMessage, HostError> {
        self.log.line(&format!(
            "[guest {} vm{} ->] {}",
            self.cid,
            self.vm,
            sanitize_for_log(line)
        ));
        GuestMessage::from_line(line)
            .map_err(|err| violation(format!("unparsable guest line ({err}): {line:?}")))
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
        let line = message
            .to_line()
            .map_err(|err| violation(format!("cannot encode a host line: {err}")))?;
        self.log.line(&format!(
            "[<-] guest {} vm{} {}",
            self.cid,
            self.vm,
            sanitize_for_log(&line)
        ));
        lines.send(&line)?;
        Ok(())
    }
}

/// The keepalive session's side of the apid forwards.
struct Bridge {
    apid: Arc<Apid>,
    /// Forwards not yet settled; a forward's thread decrements it before it reports.
    in_flight: Arc<AtomicUsize>,
    settled_tx: Sender<Settled>,
    settled: Receiver<Settled>,
    /// Woken after each settled answer, so a waiting keepalive session takes it up at once.
    waker: Arc<Waker>,
}

/// A forward's or a profile retry's result: a note for the log, and the line to write, if any.
struct Settled {
    note: String,
    line: Option<HostMessage>,
}

impl Bridge {
    /// Asks apid for the profile again, on a thread (a hello is bounded at 2 s).
    fn hello(&self, cid: u32, role: &str, vm: u32, caps: &[String]) {
        let (apid, tx, waker, role, caps) = (
            Arc::clone(&self.apid),
            self.settled_tx.clone(),
            Arc::clone(&self.waker),
            role.to_string(),
            caps.to_vec(),
        );
        thread::spawn(move || {
            let done = match apid.hello(cid, &role, vm, &caps) {
                Ok(Some(profile)) => Settled {
                    note: "got its profile from apid".to_string(),
                    line: Some(HostMessage::Profile { profile }),
                },
                Ok(None) => Settled {
                    note: "has no profile from apid yet".to_string(),
                    line: None,
                },
                Err(err) => Settled {
                    note: format!("hello to apid failed, retrying: {err}"),
                    line: None,
                },
            };
            let _ = tx.send(done);
            waker.wake();
        });
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
        GuestMessage::EngineUpdate { .. } => "engine_update",
        GuestMessage::ToolCall { .. } => "tool_call",
        GuestMessage::ToolResult { .. } => "tool_result",
        GuestMessage::Heartbeat { .. } => "heartbeat",
        GuestMessage::Bye { .. } => "bye",
        GuestMessage::ModelCall { .. } => "model_call",
        GuestMessage::ToolRequest { .. } => "tool_request",
        GuestMessage::ToolReport { .. } => "tool_report",
        GuestMessage::WorkDone { .. } => "work_done",
        GuestMessage::Compressed { .. } => "compressed",
    }
}

/// The v1 tools the hub runs for a guest; anything else is refused as data.
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
            Err(err) => (false, format!("cannot read the host's uptime: {err}")),
        },
        other => (false, format!("unknown tool: {other}")),
    }
}

/// The host's uptime and idle time in seconds, as `/proc/uptime` gives them
/// (`"<up> <idle>"`). Windows has no `/proc`; [`crate::sys::host_uptime`]
/// reads the same two numbers from the system there.
#[cfg(windows)]
fn host_uptime() -> io::Result<String> {
    crate::sys::host_uptime()
}

/// The first two fields of `/proc/uptime`.
#[cfg(not(windows))]
fn host_uptime() -> io::Result<String> {
    let text = std::fs::read_to_string("/proc/uptime")?;
    match text.split_whitespace().take(2).collect::<Vec<_>>()[..] {
        [up, idle] => Ok(format!("{up} {idle}")),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "/proc/uptime has fewer than two fields",
        )),
    }
}

/// The most of one protocol line the log keeps (model calls carry whole
/// conversations; the transcript in apid is the full record).
const LOG_LINE_CHARS: usize = 240;

/// Escapes control characters so a guest cannot forge or erase log lines, and
/// cuts long lines to [`LOG_LINE_CHARS`].
fn sanitize_for_log(line: &str) -> String {
    let mut sanitized = String::with_capacity(line.len().min(LOG_LINE_CHARS + 32));
    let total = line.chars().count();
    for character in line.chars().take(LOG_LINE_CHARS) {
        if character.is_control() {
            sanitized.push_str(&format!("\\x{:02x}", character as u32));
        } else {
            sanitized.push(character);
        }
    }
    if total > LOG_LINE_CHARS {
        sanitized.push_str(&format!(" … ({total} chars)"));
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::tests::{Recording, hub_with};
    use crate::hub::{SendOutcome, StopOutcome, TellOutcome};
    use apz_uds::UnixStream;
    use serde_json::{Value, json};
    use std::fs;
    use std::net::Shutdown;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// What a fake apid saw: (path, token header, body), in arrival order.
    type Requests = Arc<Mutex<Vec<(String, Option<String>, String)>>>;
    type SessionThread = thread::JoinHandle<Result<SessionSummary, HostError>>;

    /// A fake apid on loopback that records every request it gets.
    struct FakeApid {
        apid: Apid,
        seen: Requests,
        token: PathBuf,
        _server: apz_http::Handle,
    }

    impl Drop for FakeApid {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.token);
        }
    }

    /// Starts a fake apid that answers each request with `answer(path, body)`.
    fn fake_apid(answer: impl Fn(&str, &str) -> String + Send + Sync + 'static) -> FakeApid {
        let seen: Requests = Arc::default();
        let recorded = Arc::clone(&seen);
        let server = apz_http::Server::bind(0)
            .unwrap()
            .spawn(move |request| {
                let body = String::from_utf8_lossy(&request.body).into_owned();
                let token = request.header("x-apz-internal").map(str::to_string);
                let reply = answer(&request.path, &body);
                recorded
                    .lock()
                    .unwrap()
                    .push((request.path.clone(), token, body));
                apz_http::Response::new(200, "application/json", reply)
            })
            .unwrap();
        let token = std::env::temp_dir().join(format!(
            "vsockd-serve-token-{}-{}",
            std::process::id(),
            server.port()
        ));
        fs::write(&token, "s3cret\n").unwrap();
        let apid = Apid::new(
            &format!("http://127.0.0.1:{}", server.port()),
            Some(token.clone()),
        )
        .unwrap();
        FakeApid {
            apid,
            seen,
            token,
            _server: server,
        }
    }

    /// Starts a keepalive session over a socket pair, with `apid` as its forwarding target.
    fn keepalive_session(
        apid: Apid,
    ) -> (Lines<UnixStream>, Arc<Hub>, Arc<Recording>, SessionThread) {
        let (guest, host) = UnixStream::pair().unwrap();
        guest
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let (log, hub) = hub_with(Some(apid));
        let hub = Arc::new(hub);
        let session = {
            let (hub, log) = (Arc::clone(&hub), Arc::clone(&log));
            thread::spawn(move || {
                handle_session(
                    &mut Lines::new(host),
                    3,
                    keepalive_limits(),
                    &*log,
                    &hub,
                    &SystemClock,
                )
            })
        };
        (Lines::new(guest), hub, log, session)
    }

    fn json_line(line: &str) -> Value {
        serde_json::from_str(line).unwrap_or_else(|err| panic!("{line:?} is not JSON: {err}"))
    }

    #[test]
    fn the_hello_puts_apids_profile_in_the_welcome_and_is_sent_with_the_token() {
        let profile = json!({
            "name": "builder-1", "role": "builder", "model": "anthropic/claude-opus-5-5",
            "system": "You build.", "tools": [], "max_turns": 8
        });
        let fake = fake_apid(move |_, _| json!({ "profile": profile }).to_string());
        let (mut guest, _hub, _log, session) = keepalive_session(fake.apid.clone());
        guest.send(KEEPALIVE_HELLO).unwrap();
        let welcome = json_line(&recv(&mut guest));
        assert_eq!(welcome["profile"]["name"], "builder-1", "{welcome}");
        guest.send(r#"{"type":"bye","calls":0}"#).unwrap();
        session.join().unwrap().expect("a clean end");
        let seen = fake.seen.lock().unwrap();
        let (path, token, body) = &seen[0];
        assert_eq!(
            (path.as_str(), token.as_deref()),
            ("/internal/v1/agents/3/hello", Some("s3cret"))
        );
        assert_eq!(json_line(body), json!({ "role": "builder", "vm": 0 }));
    }

    /// A keepalive hello that names no role, as a guest restored from a warm template sends it.
    const ROLELESS_KEEPALIVE_HELLO: &str =
        r#"{"type":"hello","vm":0,"guest_cid":3,"keepalive":true}"#;
    const REFUSED_ROLELESS: &str =
        "a keepalive hello without a role needs the hub to name the agent's role";

    /// A keepalive welcome without its `host`, for the tests that are not about the host facts.
    fn without_host(line: &str) -> Value {
        let mut welcome = json_line(line);
        welcome
            .as_object_mut()
            .expect("a welcome is an object")
            .remove("host");
        welcome
    }

    #[test]
    fn a_roleless_keepalive_hello_is_registered_under_the_role_apid_names() {
        let profile = json!({
            "name": "reviewer-1", "role": "reviewer", "model": "anthropic/claude-opus-5-5",
            "system": "You review.", "tools": [], "max_turns": 8
        });
        let fake = fake_apid(move |_, _| json!({ "profile": profile }).to_string());
        let (mut guest, hub, _log, session) = keepalive_session(fake.apid.clone());
        guest.send(ROLELESS_KEEPALIVE_HELLO).unwrap();
        let welcome = json_line(&recv(&mut guest));
        assert_eq!(
            (
                welcome["role"].as_str(),
                welcome["profile"]["role"].as_str()
            ),
            (Some("reviewer"), Some("reviewer")),
            "{welcome}"
        );
        let registered: Vec<(u32, String)> = hub
            .snapshot()
            .rows
            .into_iter()
            .map(|row| (row.cid, row.role))
            .collect();
        assert_eq!(registered, [(3, "reviewer".to_string())]);
        guest.send(r#"{"type":"bye","calls":0}"#).unwrap();
        session.join().unwrap().expect("a clean end");
        let seen = fake.seen.lock().unwrap();
        let (path, token, body) = &seen[0];
        assert_eq!(
            (path.as_str(), token.as_deref()),
            ("/internal/v1/agents/3/hello", Some("s3cret"))
        );
        assert_eq!(json_line(body), json!({ "role": "", "vm": 0 }));
    }

    #[test]
    fn a_roleless_keepalive_hello_with_no_profile_is_refused_and_never_registered() {
        let fake = fake_apid(|_, _| json!({ "profile": null }).to_string());
        let (mut guest, hub, _log, session) = keepalive_session(fake.apid.clone());
        guest.send(ROLELESS_KEEPALIVE_HELLO).unwrap();
        let Err(err) = session.join().unwrap() else {
            panic!("a session with no role and no profile was not refused");
        };
        assert!(err.to_string().contains(REFUSED_ROLELESS), "{err}");
        assert!(hub.snapshot().rows.is_empty(), "nothing registered");
        // apid is asked for the profile first; the session-end notice follows it.
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].0, "/internal/v1/agents/3/hello", "{seen:?}");
    }

    #[test]
    fn with_apid_off_a_roleless_keepalive_hello_is_refused_with_no_welcome() {
        let (result, log, sent) = scripted(
            &[ROLELESS_KEEPALIVE_HELLO],
            true,
            keepalive_limits(),
            &SystemClock,
        );
        let Err(err) = result else {
            panic!("a session with no role and no apid was not refused");
        };
        assert!(err.to_string().contains(REFUSED_ROLELESS), "{err}");
        assert!(sent.is_empty(), "no welcome: {sent:?}");
        assert!(!log.contains("[<-]"), "{log}");
    }

    #[test]
    fn a_keepalive_welcome_carries_the_hosts_clock_and_fresh_entropy() {
        let epoch_ms = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        };
        let mut entropies = Vec::new();
        for _ in 0..2 {
            let before = epoch_ms();
            let (mut guest, _hub, _log, session) = keepalive_session(Apid::off());
            guest.send(KEEPALIVE_HELLO).unwrap();
            let welcome = json_line(&recv(&mut guest));
            let after = epoch_ms();
            let host = &welcome["host"];
            let time_ms = host["time_ms"].as_u64().expect("a host clock");
            assert!(
                (before..=after).contains(&time_ms),
                "{time_ms} is not between {before} and {after}: {welcome}"
            );
            let entropy = host["entropy"].as_str().expect("host entropy");
            assert_eq!(entropy.len(), 64, "{welcome}");
            assert!(entropy.bytes().all(|b| b.is_ascii_hexdigit()), "{entropy}");
            entropies.push(entropy.to_string());
            guest.send(r#"{"type":"bye","calls":0}"#).unwrap();
            session.join().unwrap().expect("a clean end");
        }
        assert_ne!(
            entropies[0], entropies[1],
            "two welcomes, one random stream"
        );
    }

    #[test]
    fn the_v1_welcome_carries_no_host() {
        let (result, _, sent) = scripted(
            &[HELLO, r#"{"type":"bye","calls":0}"#],
            true,
            v1_limits(),
            &SystemClock,
        );
        result.expect("a v1 session");
        assert!(json_line(&sent[0]).get("host").is_none(), "{}", sent[0]);
    }

    #[test]
    fn a_model_call_is_forwarded_and_its_reply_written_while_heartbeats_are_still_read() {
        let (release, gate) = channel::<()>();
        let gate = Mutex::new(gate);
        let fake = fake_apid(move |_, body| {
            if !body.contains(r#""type":"model_call""#) {
                return json!({ "reply": null }).to_string();
            }
            // The model call is out until the test releases it.
            let _ = gate.lock().unwrap().recv();
            json!({ "reply": { "type": "model_result", "req": 1, "ok": false, "error": "fake model" } }).to_string()
        });
        let (mut guest, hub, _log, session) = keepalive_session(fake.apid.clone());
        guest.send(KEEPALIVE_HELLO).unwrap();
        recv(&mut guest);
        guest
            .send(r#"{"type":"model_call","req":1,"work":1,"request":{"messages":[]}}"#)
            .unwrap();
        guest
            .send(r#"{"type":"heartbeat","seq":1,"busy":true}"#)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !hub.snapshot().rows.first().is_some_and(|row| row.busy) {
            assert!(
                Instant::now() < deadline,
                "the heartbeat was not read while the model call was out"
            );
            thread::sleep(Duration::from_millis(10));
        }
        release.send(()).unwrap();
        let reply = json_line(&recv(&mut guest));
        assert_eq!(
            reply,
            json!({ "type": "model_result", "req": 1, "ok": false, "error": "fake model" })
        );
        guest.send(r#"{"type":"bye","calls":0}"#).unwrap();
        session.join().unwrap().expect("a clean end");
        let seen = fake.seen.lock().unwrap();
        let (path, token, body) = seen
            .iter()
            .find(|entry| entry.0.ends_with("/message"))
            .expect("forwarded");
        assert_eq!(
            (path.as_str(), token.as_deref()),
            ("/internal/v1/agents/3/message", Some("s3cret"))
        );
        assert_eq!(json_line(body)["type"], "model_call");
    }

    #[test]
    fn with_apid_down_a_failed_model_call_and_tool_request_are_refused_and_the_guest_never_waits() {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let token = std::env::temp_dir().join(format!("vsockd-serve-down-{}", std::process::id()));
        fs::write(&token, "s3cret").unwrap();
        let apid = Apid::new(&format!("http://127.0.0.1:{port}"), Some(token.clone())).unwrap();
        let (mut guest, _hub, log, session) = keepalive_session(apid);
        guest.send(KEEPALIVE_HELLO).unwrap();
        let welcome = json_line(&recv(&mut guest));
        assert!(
            welcome.get("profile").is_none(),
            "no profile without apid: {welcome}"
        );

        guest
            .send(r#"{"type":"model_call","req":7,"work":1,"request":{"messages":[]}}"#)
            .unwrap();
        let model = json_line(&recv(&mut guest));
        assert_eq!(
            (
                model["type"].as_str(),
                model["req"].as_u64(),
                model["ok"].as_bool()
            ),
            (Some("model_result"), Some(7), Some(false))
        );
        assert!(
            model["error"]
                .as_str()
                .unwrap_or_default()
                .starts_with("apid unreachable"),
            "{model}"
        );

        guest
            .send(r#"{"type":"tool_request","work":1,"call":"toolu_1","name":"bash","input":{}}"#)
            .unwrap();
        let decision = json_line(&recv(&mut guest));
        assert_eq!(
            (
                decision["type"].as_str(),
                decision["call"].as_str(),
                decision["decision"].as_str(),
                decision["is_error"].as_bool()
            ),
            (
                Some("tool_decision"),
                Some("toolu_1"),
                Some("deny"),
                Some(true)
            ),
            "{decision}"
        );

        // A report and a work_done expect no answer, so nothing else is written.
        guest
            .send(r#"{"type":"tool_report","work":1,"call":"toolu_1","ok":true,"output":"x"}"#)
            .unwrap();
        guest
            .send(r#"{"type":"work_done","work":1,"status":"done","summary":""}"#)
            .unwrap();
        guest.send(r#"{"type":"bye","calls":0}"#).unwrap();
        session.join().unwrap().expect("a clean end");
        let rest = String::from_utf8(crate::read_until_closed(
            guest.get_ref(),
            Duration::from_secs(20),
        ))
        .unwrap();
        assert_eq!(rest, "", "nothing is written after the refusals");
        let log = log.text();
        assert!(
            log.contains("hello to apid failed") && log.contains("apid failed model_call"),
            "{log}"
        );
        fs::remove_file(token).unwrap();
    }

    #[test]
    fn the_session_end_is_posted_to_apid_as_gone_with_its_reason() {
        let fake = fake_apid(|_, _| json!({ "reply": null, "profile": null }).to_string());
        let (mut guest, _hub, _log, session) = keepalive_session(fake.apid.clone());
        guest.send(KEEPALIVE_HELLO).unwrap();
        recv(&mut guest);
        guest.send(r#"{"type":"bye","calls":0}"#).unwrap();
        session.join().unwrap().expect("a clean end");
        let seen = fake.seen.lock().unwrap();
        let (path, token, body) = seen
            .iter()
            .find(|entry| entry.0.ends_with("/gone"))
            .expect("a gone notice");
        assert_eq!(
            (path.as_str(), token.as_deref()),
            ("/internal/v1/agents/3/gone", Some("s3cret"))
        );
        assert_eq!(json_line(body), json!({ "reason": "bye" }));
    }

    const HELLO: &str = r#"{"type":"hello","vm":0,"guest_cid":3}"#;
    const KEEPALIVE_HELLO: &str =
        r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder","keepalive":true}"#;
    const KEEPALIVE_WELCOME: &str =
        r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#;

    fn v1_limits() -> SessionLimits {
        SessionLimits {
            max_calls: 64,
            deadline: None,
            keepalive_idle: None,
            keepalive_read_slice: Duration::from_secs(1),
            keepalive_max_wait: Duration::from_millis(20),
        }
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
        let sent =
            String::from_utf8(crate::read_until_closed(&guest, Duration::from_secs(20))).unwrap();
        (
            result,
            log.text(),
            sent.lines().map(str::to_string).collect(),
        )
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
        assert_eq!(
            (summary.calls, summary.guest_calls, summary.keepalive),
            (3, 3, false)
        );
        assert_eq!(sent[0], r#"{"type":"welcome","echo":"plaza-orchestrator"}"#);
        assert_eq!(
            sent[1],
            r#"{"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}"#
        );
        let uptime: serde_json::Value = serde_json::from_str(&sent[2]).unwrap();
        assert_eq!(
            uptime["output"]
                .as_str()
                .unwrap()
                .split_whitespace()
                .count(),
            2,
            "{uptime}"
        );
        assert_eq!(
            sent[3],
            r#"{"type":"tool_result","call":3,"ok":false,"output":"unknown tool: rm_rf"}"#
        );
        assert!(
            log.contains("[guest 3 vm? ->] {\"type\":\"hello\"")
                && log.contains("said bye after 3 tool calls"),
            "{log}"
        );
    }

    #[test]
    fn protocol_violations_drop_the_session() {
        let budget = SessionLimits {
            max_calls: 1,
            ..v1_limits()
        };
        let past = SessionLimits {
            deadline: Some(Instant::now()),
            ..v1_limits()
        };
        let echo = r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"a"}}"#;
        let cases: [(&[&str], SessionLimits, &str); 9] = [
            (&[echo], v1_limits(), "tool_call before hello"),
            (&["not json"], v1_limits(), "unparsable guest line"),
            (
                &[r#"{"type":"hello","vm":0,"guest_cid":4}"#],
                v1_limits(),
                "claims guest_cid 4",
            ),
            (&[HELLO, HELLO], v1_limits(), "second hello"),
            (
                &[HELLO, r#"{"type":"heartbeat","seq":1}"#],
                v1_limits(),
                "did not ask for keepalive",
            ),
            (
                &[HELLO, echo, echo],
                budget,
                "tool-call budget (1) exceeded",
            ),
            (&[HELLO], past, "session deadline exceeded"),
            (
                &[KEEPALIVE_HELLO],
                v1_limits(),
                "configured for one-shot sessions",
            ),
            (&[], v1_limits(), "closed before sending hello"),
        ];
        for (script, limits, expected) in cases {
            let (result, _, _) = scripted(script, true, limits, &SystemClock);
            let err = result.expect_err(expected);
            assert!(
                matches!(&err, HostError::Protocol(detail) if detail.contains(expected)),
                "{expected}: {err:?}"
            );
        }
    }

    #[test]
    fn control_characters_in_a_guest_line_are_escaped_in_the_log() {
        let (_, log, _) = scripted(
            &["not json\u{1b}[2K\rforged line"],
            true,
            v1_limits(),
            &SystemClock,
        );
        assert!(log.contains("\\x1b") && log.contains("\\x0d"), "{log}");
        assert!(!log.contains('\u{1b}') && !log.contains('\r'), "{log}");
    }

    #[test]
    fn a_role_without_keepalive_gets_the_v1_welcome() {
        let script = [
            r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder"}"#,
            r#"{"type":"bye","calls":0}"#,
        ];
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
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(without_host(&sent[0]), json_line(KEEPALIVE_WELCOME));

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

    /// A clock the test moves by hand.
    struct ManualClock(Mutex<Instant>);

    impl ManualClock {
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> Instant {
            *self.0.lock().unwrap()
        }
    }

    #[test]
    fn a_quiet_keepalive_is_not_reaped_and_a_loud_one_gets_a_full_idle_window() {
        let (guest, host) = UnixStream::pair().unwrap();
        guest
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let (log, hub) = hub_with(None);
        let hub = Arc::new(hub);
        let clock = Arc::new(ManualClock(Mutex::new(Instant::now())));
        let session = {
            let (hub, log, clock) = (Arc::clone(&hub), Arc::clone(&log), Arc::clone(&clock));
            thread::spawn(move || {
                handle_session(
                    &mut Lines::new(host),
                    3,
                    keepalive_limits(),
                    &*log,
                    &hub,
                    &*clock,
                )
            })
        };
        let settle = || thread::sleep(Duration::from_millis(200));
        let mut guest = Lines::new(guest);
        guest.send(KEEPALIVE_HELLO).unwrap();
        recv(&mut guest);

        // Silent far past the idle window, but quiet: still connected.
        assert!(hub.set_quiet(3, true));
        clock.advance(Duration::from_secs(1000));
        settle();
        assert!(!session.is_finished(), "a quiet agent is not reaped");

        // Loud again: the window starts now, so 299 s of silence is still inside it.
        assert!(hub.set_quiet(3, false));
        settle();
        clock.advance(Duration::from_secs(299));
        settle();
        assert!(!session.is_finished(), "inside the idle window");

        clock.advance(Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !session.is_finished() {
            assert!(Instant::now() < deadline, "the loud agent was not reaped");
            thread::sleep(Duration::from_millis(10));
        }
        let result = session.join().unwrap();
        assert!(matches!(result, Err(HostError::Reaped)), "{result:?}");
        assert!(log.text().contains("vsockd: agent 3 reaped idle > 300s"));
    }

    /// The whole keepalive exchange over a real socket, steered through the
    /// hub the way the HTTP surface steers it.
    #[test]
    fn the_hub_steers_a_keepalive_session_end_to_end() {
        let (guest, host) = UnixStream::pair().unwrap();
        guest
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let (log, hub) = hub_with(None);
        let hub = Arc::new(hub);
        let session = {
            let (hub, log) = (Arc::clone(&hub), Arc::clone(&log));
            thread::spawn(move || {
                handle_session(
                    &mut Lines::new(host),
                    3,
                    keepalive_limits(),
                    &*log,
                    &hub,
                    &SystemClock,
                )
            })
        };
        let mut guest = Lines::new(guest);
        guest.send(KEEPALIVE_HELLO).unwrap();
        assert_eq!(
            without_host(&recv(&mut guest)),
            json_line(KEEPALIVE_WELCOME)
        );
        guest
            .send(r#"{"type":"heartbeat","seq":1,"busy":true}"#)
            .unwrap();

        let tell = |name: &'static str| {
            let hub = Arc::clone(&hub);
            thread::spawn(move || hub.tell(3, name, serde_json::json!({}), Duration::from_secs(5)))
        };
        let answered = tell("list");
        let dispatched: serde_json::Value = serde_json::from_str(&recv(&mut guest)).unwrap();
        assert_eq!(
            (dispatched["type"].as_str(), dispatched["call"].as_u64()),
            (Some("tool_call"), Some(1))
        );
        guest
            .send(r#"{"type":"tool_result","call":1,"ok":true,"output":"[]"}"#)
            .unwrap();
        assert_eq!(
            answered.join().unwrap(),
            TellOutcome::Answered {
                ok: true,
                output: "[]".into()
            }
        );
        assert!(hub.snapshot().rows[0].busy);

        let cancel = HostMessage::Cancel {
            work: 7,
            reason: "operator".into(),
        };
        assert_eq!(
            hub.send(3, cancel.clone(), Duration::from_secs(5)),
            SendOutcome::Sent
        );
        assert_eq!(recv(&mut guest), cancel.to_line().unwrap());

        let abandoned = tell("read_file");
        recv(&mut guest);
        let stopper = {
            let hub = Arc::clone(&hub);
            thread::spawn(move || hub.stop(3, "slice proof", Duration::from_secs(5)))
        };
        assert_eq!(
            recv(&mut guest),
            r#"{"type":"stop","reason":"slice proof"}"#
        );
        guest.send(r#"{"type":"bye","calls":2}"#).unwrap();
        assert_eq!(stopper.join().unwrap(), StopOutcome::Ended { calls: 2 });
        assert_eq!(
            abandoned.join().unwrap(),
            TellOutcome::NotConnected,
            "nobody is left to answer"
        );
        assert_eq!(session.join().unwrap().expect("a clean end").calls, 2);
        assert!(
            log.text()
                .contains("vsockd: agent 3 session ended cleanly after 2 tool calls"),
            "{}",
            log.text()
        );
    }
}
