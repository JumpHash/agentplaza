//! The hub: the registry of connected keepalive agents, and the steering
//! channel each keepalive session reads its [`Command`]s from.
//!
//! The HTTP surface ([`crate::http`]) reaches a session only through here:
//! [`Hub::tell`] (one tool call, answered back to the caller), [`Hub::send`]
//! (one raw host line) and [`Hub::stop`] (waits for the session to end). Tool
//! carries apid's client ([`Hub::apid`]). Locks guard only the maps, and log
//! lines are written after they are released.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use apz_proto::wire::HostMessage;
use serde::Serialize;

use crate::apid::Apid;
use crate::serve::SessionLog;

/// One heartbeat line is logged per this many beats (and the first).
pub const HEARTBEAT_LOG_EVERY: u64 = 12;

/// One connected keepalive agent.
#[derive(Debug, Clone)]
pub struct Agent {
    pub cid: u32,
    /// The role accepted at the handshake: the agent's identity.
    pub role: String,
    pub connected_at: Instant,
    pub last_heartbeat: Instant,
    /// What the last heartbeat said about a work item in progress.
    pub busy: bool,
    /// Tool calls served, in either direction.
    pub calls: u64,
}

#[derive(Default)]
struct Registry {
    agents: HashMap<u32, Agent>,
    /// Beats per CID, for the log rate limit.
    beats: HashMap<u32, u64>,
}

/// One registry row with wall-clock times (ms since the Unix epoch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub cid: u32,
    pub role: String,
    pub connected_at_ms: u64,
    pub last_heartbeat_ms: u64,
    pub busy: bool,
    pub calls: u64,
}

/// The registry, copied out for the status view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Ordered by CID.
    pub rows: Vec<Row>,
    pub sessions_active: usize,
}

fn epoch_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// What the hub asks of a live keepalive session.
#[derive(Debug)]
pub enum Command {
    /// Run one tool call; the matching `tool_result` goes to `reply`.
    Call {
        name: String,
        args: serde_json::Value,
        reply: Option<Sender<ToolReply>>,
    },
    /// Write one host line as is; `written` hears when it is on the wire.
    Send {
        message: HostMessage,
        written: Sender<()>,
    },
    /// End the session with the wire's `stop`.
    Stop { reason: String },
}

/// An agent's answer to a dispatched tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReply {
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TellOutcome {
    Answered {
        ok: bool,
        output: String,
    },
    /// No answer within the wait; the session is untouched.
    TimedOut,
    NotConnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Sent,
    /// The session did not take the line within the wait.
    TimedOut,
    NotConnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// The session ended; `calls` is the guest's `bye` count when it said one.
    Ended {
        calls: u64,
    },
    /// The stop went out but the session was still up when the wait lapsed.
    /// The hub cannot kill a session; its idle deadline still applies.
    StillUp,
    NotConnected,
}

/// A live session's control handles.
#[derive(Clone)]
struct Live {
    /// Distinguishes this session from a predecessor on the same CID.
    token: u64,
    commands: Sender<Command>,
    ended: Arc<SessionEnd>,
}

/// Fired once, by whichever path ends the session; [`Hub::stop`] waits on it.
#[derive(Default)]
struct SessionEnd {
    ended: Mutex<Option<Ended>>,
    signal: Condvar,
}

#[derive(Debug, Clone, Copy)]
struct Ended {
    calls: u64,
    guest_calls: Option<u64>,
}

impl SessionEnd {
    fn fire(&self, ended: Ended) {
        let mut slot = self.ended.lock().unwrap_or_else(|err| err.into_inner());
        slot.get_or_insert(ended);
        self.signal.notify_all();
    }

    fn wait(&self, wait: Duration) -> Option<Ended> {
        let slot = self.ended.lock().unwrap_or_else(|err| err.into_inner());
        let (slot, _) = self
            .signal
            .wait_timeout_while(slot, wait, |ended| ended.is_none())
            .unwrap_or_else(|err| err.into_inner());
        *slot
    }
}

/// The hub every keepalive session registers with.
pub struct Hub {
    registry: Mutex<Registry>,
    steering: Mutex<HashMap<u32, Live>>,
    epoch: AtomicU64,
    /// `Instant`s are reported as this wall-clock anchor plus their offset.
    anchor: (Instant, u64),
    /// The client for apid's internal surface; sessions forward through it.
    apid: Arc<Apid>,
    log: Arc<dyn SessionLog + Send + Sync>,
}

impl Hub {
    pub fn new(log: Arc<dyn SessionLog + Send + Sync>, apid: Apid) -> Self {
        Self::started_at(log, apid, Instant::now(), epoch_ms_now())
    }

    pub(crate) fn started_at(
        log: Arc<dyn SessionLog + Send + Sync>,
        apid: Apid,
        at: Instant,
        epoch_ms: u64,
    ) -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            steering: Mutex::new(HashMap::new()),
            epoch: AtomicU64::new(0),
            anchor: (at, epoch_ms),
            apid: Arc::new(apid),
            log,
        }
    }

    /// The client for apid's internal surface.
    pub fn apid(&self) -> Arc<Apid> {
        Arc::clone(&self.apid)
    }

    /// Poison-proof: a panicking session must not take the other agents down.
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn steering(&self) -> MutexGuard<'_, HashMap<u32, Live>> {
        self.steering.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Registers a session; a CID already registered is re-adopted (its old
    /// token stops matching, so the old session's cleanup cannot evict it).
    pub fn connect(&self, cid: u32, role: &str, now: Instant) -> (u64, Receiver<Command>) {
        let token = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let (commands, receiver) = channel();
        self.steering().insert(
            cid,
            Live {
                token,
                commands,
                ended: Arc::default(),
            },
        );
        let agent = Agent {
            cid,
            role: role.to_string(),
            connected_at: now,
            last_heartbeat: now,
            busy: false,
            calls: 0,
        };
        let previous = {
            let mut registry = self.registry();
            registry.beats.remove(&cid);
            registry.agents.insert(cid, agent)
        };
        self.log
            .line(&format!("vsockd: agent {cid} role {role} connected"));
        if let Some(previous) = previous {
            self.log.line(&format!(
                "vsockd: agent {cid} re-adopted (previous session role {}, connected {}s ago)",
                previous.role,
                now.saturating_duration_since(previous.connected_at)
                    .as_secs()
            ));
        }
        (token, receiver)
    }

    pub fn heartbeat(&self, cid: u32, seq: u64, busy: bool, now: Instant) {
        let line = {
            let mut registry = self.registry();
            let Some(agent) = registry.agents.get_mut(&cid) else {
                return;
            };
            agent.last_heartbeat = now;
            agent.busy = busy;
            let beats = registry.beats.entry(cid).or_insert(0);
            *beats += 1;
            (*beats == 1 || beats.is_multiple_of(HEARTBEAT_LOG_EVERY))
                .then(|| format!("vsockd: agent {cid} heartbeat {seq}"))
        };
        if let Some(line) = line {
            self.log.line(&line);
        }
    }

    pub fn note_calls(&self, cid: u32, calls: u64) {
        if let Some(agent) = self.registry().agents.get_mut(&cid) {
            agent.calls = calls;
        }
    }

    /// A session that said `bye`.
    pub fn end_clean(&self, cid: u32, token: u64, calls: u64, guest_calls: u64) {
        if self.forget(cid, token, Some(guest_calls)) {
            self.log.line(&format!(
                "vsockd: agent {cid} session ended cleanly after {calls} tool calls"
            ));
        }
    }

    /// A session that stopped heartbeating.
    pub fn reap_idle(&self, cid: u32, token: u64, idle: Duration) {
        if self.forget(cid, token, None) {
            self.log.line(&format!(
                "vsockd: agent {cid} reaped idle > {}s",
                idle.as_secs()
            ));
        }
    }

    /// Any other end; the session logs its own reason.
    pub fn hangup(&self, cid: u32, token: u64) {
        self.forget(cid, token, None);
    }

    /// Drops the session's row if `token` still owns it, and fires its end.
    fn forget(&self, cid: u32, token: u64, guest_calls: Option<u64>) -> bool {
        let live = match self.steering().entry(cid) {
            Entry::Occupied(entry) if entry.get().token == token => entry.remove(),
            _ => return false,
        };
        let calls = {
            let mut registry = self.registry();
            registry.beats.remove(&cid);
            registry.agents.remove(&cid).map_or(0, |agent| agent.calls)
        };
        live.ended.fire(Ended { calls, guest_calls });
        true
    }

    fn live(&self, cid: u32) -> Option<Live> {
        self.steering().get(&cid).cloned()
    }

    /// Runs one tool call on `cid` and waits up to `wait` for its answer. The
    /// session numbers the call.
    pub fn tell(
        &self,
        cid: u32,
        name: &str,
        args: serde_json::Value,
        wait: Duration,
    ) -> TellOutcome {
        let Some(live) = self.live(cid) else {
            return TellOutcome::NotConnected;
        };
        let (reply, answer) = channel();
        if live
            .commands
            .send(Command::Call {
                name: name.to_string(),
                args,
                reply: Some(reply),
            })
            .is_err()
        {
            return TellOutcome::NotConnected;
        }
        match answer.recv_timeout(wait) {
            Ok(ToolReply { ok, output }) => TellOutcome::Answered { ok, output },
            Err(RecvTimeoutError::Timeout) => TellOutcome::TimedOut,
            // The session ended with the call unanswered.
            Err(RecvTimeoutError::Disconnected) => TellOutcome::NotConnected,
        }
    }

    /// Writes `message` to `cid`'s guest, waiting up to `wait` for the session
    /// to take it.
    pub fn send(&self, cid: u32, message: HostMessage, wait: Duration) -> SendOutcome {
        let Some(live) = self.live(cid) else {
            return SendOutcome::NotConnected;
        };
        let (written, taken) = channel();
        if live
            .commands
            .send(Command::Send { message, written })
            .is_err()
        {
            return SendOutcome::NotConnected;
        }
        match taken.recv_timeout(wait) {
            Ok(()) => SendOutcome::Sent,
            Err(RecvTimeoutError::Timeout) => SendOutcome::TimedOut,
            Err(RecvTimeoutError::Disconnected) => SendOutcome::NotConnected,
        }
    }

    /// Sends `cid`'s session a `stop` and waits up to `wait` for it to end.
    pub fn stop(&self, cid: u32, reason: &str, wait: Duration) -> StopOutcome {
        let Some(live) = self.live(cid) else {
            return StopOutcome::NotConnected;
        };
        if live
            .commands
            .send(Command::Stop {
                reason: reason.to_string(),
            })
            .is_err()
        {
            return StopOutcome::NotConnected;
        }
        match live.ended.wait(wait) {
            Some(ended) => StopOutcome::Ended {
                calls: ended.guest_calls.unwrap_or(ended.calls),
            },
            None => StopOutcome::StillUp,
        }
    }

    #[cfg(test)]
    pub(crate) fn steer(&self, cid: u32) -> Option<Sender<Command>> {
        self.live(cid).map(|live| live.commands)
    }

    #[cfg(test)]
    pub(crate) fn agent_count(&self) -> usize {
        self.registry().agents.len()
    }

    /// Copies the registry out; the caller serializes with no lock held.
    pub fn snapshot(&self) -> Snapshot {
        let mut rows: Vec<Row> = self
            .registry()
            .agents
            .values()
            .map(|agent| Row {
                cid: agent.cid,
                role: agent.role.clone(),
                connected_at_ms: self.epoch_ms(agent.connected_at),
                last_heartbeat_ms: self.epoch_ms(agent.last_heartbeat),
                busy: agent.busy,
                calls: agent.calls,
            })
            .collect();
        rows.sort_by_key(|row| row.cid);
        Snapshot {
            rows,
            sessions_active: self.steering().len(),
        }
    }

    fn epoch_ms(&self, at: Instant) -> u64 {
        let (anchor_at, anchor_ms) = self.anchor;
        anchor_ms.saturating_add(at.saturating_duration_since(anchor_at).as_millis() as u64)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::thread;

    /// A [`SessionLog`] that keeps every line.
    #[derive(Default)]
    pub(crate) struct Recording(Mutex<Vec<String>>);

    impl Recording {
        pub(crate) fn text(&self) -> String {
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

    /// A hub whose apid client is `apid` (forwarding off when `None`).
    pub(crate) fn hub_with(apid: Option<Apid>) -> (Arc<Recording>, Hub) {
        let log = Arc::new(Recording::default());
        (
            Arc::clone(&log),
            Hub::new(log, apid.unwrap_or_else(Apid::off)),
        )
    }

    #[test]
    fn transitions_are_logged_and_heartbeat_lines_rate_limited() {
        let (log, hub) = hub_with(None);
        let now = Instant::now();
        let (token, _steering) = hub.connect(10, "builder", now);
        for _ in 0..HEARTBEAT_LOG_EVERY * 2 {
            hub.heartbeat(10, 1, false, now);
        }
        hub.end_clean(10, token, 1, 1);
        let (token, _steering) = hub.connect(11, "", now);
        hub.reap_idle(11, token, Duration::from_secs(300));
        assert_eq!(hub.agent_count(), 0);
        assert_eq!(
            log.text(),
            [
                "vsockd: agent 10 role builder connected",
                "vsockd: agent 10 heartbeat 1",
                "vsockd: agent 10 heartbeat 1",
                "vsockd: agent 10 heartbeat 1",
                "vsockd: agent 10 session ended cleanly after 1 tool calls",
                "vsockd: agent 11 role  connected",
                "vsockd: agent 11 reaped idle > 300s",
            ]
            .join("\n")
        );
    }

    #[test]
    fn a_reconnect_is_adopted_and_the_old_session_cannot_evict_it() {
        let (_log, hub) = hub_with(None);
        let (first, _old) = hub.connect(10, "builder", Instant::now());
        let (_second, _new) = hub.connect(10, "builder", Instant::now());
        hub.hangup(10, first);
        assert_eq!(hub.agent_count(), 1, "the predecessor's token owns nothing");
        assert!(hub.steer(10).is_some());
    }

    #[test]
    fn tell_answers_times_out_or_reports_a_gone_session() {
        let (_log, hub) = hub_with(None);
        assert_eq!(
            hub.tell(99, "echo", serde_json::json!({}), Duration::from_millis(10)),
            TellOutcome::NotConnected
        );

        let (_token, steering) = hub.connect(10, "builder", Instant::now());
        let agent = thread::spawn(move || {
            if let Ok(Command::Call {
                name,
                reply: Some(reply),
                ..
            }) = steering.recv()
            {
                reply
                    .send(ToolReply {
                        ok: true,
                        output: format!("ran {name}"),
                    })
                    .unwrap();
            }
            steering
        });
        let answer = hub.tell(10, "list", serde_json::json!({}), Duration::from_secs(5));
        assert_eq!(
            answer,
            TellOutcome::Answered {
                ok: true,
                output: "ran list".into()
            }
        );
        let steering = agent.join().unwrap();
        // Held but never read: an honest timeout, the session untouched.
        assert_eq!(
            hub.tell(10, "list", serde_json::json!({}), Duration::from_millis(50)),
            TellOutcome::TimedOut
        );
        assert_eq!(hub.agent_count(), 1);
        drop(steering);
        assert_eq!(
            hub.tell(10, "list", serde_json::json!({}), Duration::from_millis(50)),
            TellOutcome::NotConnected
        );
    }

    #[test]
    fn stop_waits_for_the_end_whichever_path_ends_it() {
        let (_log, hub) = hub_with(None);
        let hub = Arc::new(hub);
        for (bye, expected) in [(true, 2), (false, 3)] {
            let (token, steering) = hub.connect(10, "builder", Instant::now());
            let session = {
                let hub = Arc::clone(&hub);
                thread::spawn(move || {
                    assert!(matches!(steering.recv(), Ok(Command::Stop { .. })));
                    thread::sleep(Duration::from_millis(50));
                    hub.note_calls(10, 3);
                    if bye {
                        hub.end_clean(10, token, 3, 2)
                    } else {
                        hub.hangup(10, token)
                    }
                })
            };
            assert_eq!(
                hub.stop(10, "operator stop", Duration::from_secs(5)),
                StopOutcome::Ended { calls: expected }
            );
            session.join().unwrap();
        }
        let (_token, _steering) = hub.connect(10, "builder", Instant::now());
        assert_eq!(
            hub.stop(10, "operator stop", Duration::from_millis(50)),
            StopOutcome::StillUp
        );
        assert_eq!(
            hub.stop(99, "operator stop", Duration::from_millis(50)),
            StopOutcome::NotConnected
        );
    }

    #[test]
    fn the_snapshot_is_ordered_by_cid_in_epoch_milliseconds() {
        let anchor = Instant::now();
        let hub = Hub::started_at(
            Arc::new(Recording::default()),
            Apid::off(),
            anchor,
            1_700_000_000_000,
        );
        hub.connect(11, "builder", anchor + Duration::from_millis(250));
        hub.connect(10, "reviewer", anchor + Duration::from_millis(500));
        hub.note_calls(10, 4);
        hub.heartbeat(10, 7, true, anchor + Duration::from_millis(5_500));
        let snapshot = hub.snapshot();
        assert_eq!(snapshot.sessions_active, 2);
        assert_eq!(
            snapshot.rows,
            [
                Row {
                    cid: 10,
                    role: "reviewer".into(),
                    connected_at_ms: 1_700_000_000_500,
                    last_heartbeat_ms: 1_700_000_005_500,
                    busy: true,
                    calls: 4
                },
                Row {
                    cid: 11,
                    role: "builder".into(),
                    connected_at_ms: 1_700_000_000_250,
                    last_heartbeat_ms: 1_700_000_000_250,
                    busy: false,
                    calls: 0
                },
            ]
        );
    }
}
