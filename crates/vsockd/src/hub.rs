//! The hub: the in-memory registry of connected keepalive agents, and the
//! outbound channel each keepalive session is steered by.
//!
//! The registry is deliberately small and in memory. One [`Agent`] row per
//! connected keepalive session is kept for the session's lifetime and removed
//! when it ends — cleanly, or because the session was reaped for going idle.
//! Every transition is logged in the daemon's existing voice (`vsockd: agent
//! <cid> ...`); the registry itself is a later slice's status surface, so
//! nothing here is public API yet.
//!
//! Liveness is the *session's* idle deadline, not a lock: the hub never holds
//! an application-level lock as the correctness mechanism for a session. The
//! mutexes here only guard the two maps, and the log line is always written
//! after the lock is released, so a slow stdout cannot couple two agents.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::proto::HostMessage;
use crate::serve::SessionLog;

/// How many heartbeats pass between logged heartbeat lines.
///
/// A keepalive agent beats every five seconds, and a fleet of them would flood
/// the operator log at one line per beat. Every agent's *first* heartbeat is
/// logged (a connection that is visibly alive), then one line per
/// [`HEARTBEAT_LOG_EVERY`] beats — one line a minute per agent at the contract's
/// 5 s cadence. The registry's `last_heartbeat` is updated on every beat
/// either way; only the log line is thinned.
pub const HEARTBEAT_LOG_EVERY: u64 = 12;

/// One connected keepalive agent.
///
/// The row the contract names: `{cid, role, connected_at, last_heartbeat,
/// calls}`. `calls` counts the tool calls the session has served, in either
/// direction (the v1 guest-initiated calls and the hub's own dispatches), so
/// it is the same budget the session enforces.
#[derive(Debug, Clone)]
pub struct Agent {
    /// The guest CID the session was accepted for.
    pub cid: u32,
    /// The role the hub accepted at the handshake — the agent's identity for
    /// its lifetime.
    pub role: String,
    /// When the session connected.
    pub connected_at: Instant,
    /// When the most recent heartbeat arrived.
    pub last_heartbeat: Instant,
    /// How many tool calls the session has served.
    pub calls: u64,
}

/// The hub's mutable state: one [`Agent`] per connected keepalive session.
#[derive(Default)]
struct Registry {
    agents: HashMap<u32, Agent>,
    /// Heartbeats received per CID, for the log rate limiter. Cleared on
    /// connect/disconnect, so a reconnecting agent starts a fresh count.
    beats: HashMap<u32, u64>,
}

/// The hub every keepalive session registers with.
pub struct Hub {
    registry: Mutex<Registry>,
    /// The outbound channel per live session — the hub's way to send a
    /// keepalive agent a `tool_call` or a `stop` at will. The row carries the
    /// session token beside the sender, so a predecessor's cleanup can never
    /// evict the session that replaced it.
    steering: Mutex<HashMap<u32, (u64, Sender<HostMessage>)>>,
    /// Bumped on every connect: a session's token.
    epoch: AtomicU64,
    log: Arc<dyn SessionLog + Send + Sync>,
}

impl Hub {
    /// A hub that logs its transitions to `log`.
    pub fn new(log: Arc<dyn SessionLog + Send + Sync>) -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            steering: Mutex::new(HashMap::new()),
            epoch: AtomicU64::new(0),
            log,
        }
    }

    /// The registry, poison-proof: a panicking session must not take the
    /// hub's other agents down with it.
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// The steering map, poison-proof.
    fn steering(&self) -> MutexGuard<'_, HashMap<u32, (u64, Sender<HostMessage>)>> {
        self.steering.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Registers a newly connected agent and returns the session token and the
    /// receiver the session is steered by.
    ///
    /// A CID that is already registered is **adopted**: a reconnecting agent is
    /// the same agent, so the new session replaces the row rather than being
    /// refused. The predecessor's token stops matching, so its later cleanup
    /// cannot evict its successor's row.
    pub fn connect(&self, cid: u32, role: &str, now: Instant) -> (u64, Receiver<HostMessage>) {
        let token = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let (sender, receiver) = channel();
        self.steering().insert(cid, (token, sender));
        let previous = {
            let mut registry = self.registry();
            registry.beats.remove(&cid);
            registry.agents.insert(
                cid,
                Agent {
                    cid,
                    role: role.to_string(),
                    connected_at: now,
                    last_heartbeat: now,
                    calls: 0,
                },
            )
        };
        self.log
            .line(&format!("vsockd: agent {cid} role {role} connected"));
        if let Some(previous) = previous {
            // A reconnect is adopted, not refused — §1: a hub restart does not
            // kill an agent, it re-adopts it — so say whose session this
            // replaced.
            self.log.line(&format!(
                "vsockd: agent {} re-adopted (previous session role {}, connected {}s ago)",
                previous.cid,
                previous.role,
                now.saturating_duration_since(previous.connected_at).as_secs()
            ));
        }
        (token, receiver)
    }

    /// Records one heartbeat, logging at most one line per
    /// [`HEARTBEAT_LOG_EVERY`] beats.
    pub fn heartbeat(&self, cid: u32, seq: u64, now: Instant) {
        let line = {
            let mut registry = self.registry();
            let Some(agent) = registry.agents.get_mut(&cid) else {
                return;
            };
            agent.last_heartbeat = now;
            let beats = registry.beats.entry(cid).or_insert(0);
            *beats += 1;
            if *beats == 1 || *beats % HEARTBEAT_LOG_EVERY == 0 {
                Some(format!("vsockd: agent {cid} heartbeat {seq}"))
            } else {
                None
            }
        };
        // Outside the lock: the log is the operator's stdout and must not
        // serialize the fleet behind it.
        if let Some(line) = line {
            self.log.line(&line);
        }
    }

    /// Records the session's running tool-call count.
    pub fn note_calls(&self, cid: u32, calls: u64) {
        if let Some(agent) = self.registry().agents.get_mut(&cid) {
            agent.calls = calls;
        }
    }

    /// Ends a session that said `bye`: logs the clean end and drops the row.
    ///
    /// `token` is checked against the live session, so a reconnect that
    /// replaced this session keeps its registration.
    pub fn end_clean(&self, cid: u32, token: u64, calls: u64) {
        if self.forget(cid, token) {
            self.log.line(&format!(
                "vsockd: agent {cid} session ended cleanly after {calls} tool calls"
            ));
        }
    }

    /// Reaps a session that stopped heartbeating: logs the reap and drops the
    /// row, so a dead guest is never leaked.
    pub fn reap_idle(&self, cid: u32, token: u64, idle: Duration) {
        if self.forget(cid, token) {
            self.log.line(&format!(
                "vsockd: agent {cid} reaped idle > {}s",
                idle.as_secs()
            ));
        }
    }

    /// Unregisters a session that ended without a clean `bye` or a reap — an
    /// I/O failure, a protocol violation, a `stop` never answered. Logs
    /// nothing: the session layer logs why it ended.
    pub fn hangup(&self, cid: u32, token: u64) {
        self.forget(cid, token);
    }

    /// Removes a session's steering channel and row, if `token` still owns
    /// them. Returns whether it did.
    fn forget(&self, cid: u32, token: u64) -> bool {
        {
            let mut steering = self.steering();
            match steering.get(&cid) {
                Some((owner, _)) if *owner == token => {
                    steering.remove(&cid);
                }
                _ => return false,
            }
        }
        let mut registry = self.registry();
        registry.agents.remove(&cid);
        registry.beats.remove(&cid);
        true
    }

    /// The outbound channel for a live session: what the hub sends a keepalive
    /// agent at will. `None` once the session has ended.
    ///
    /// This slice wires no hub-side producer (the control plane — the status
    /// surface and the dispatch of real work — is a later slice), so the
    /// channel is exercised by the session tests; the hub's own consumer half
    /// is production code either way.
    #[cfg(test)]
    pub(crate) fn steer(&self, cid: u32) -> Option<Sender<HostMessage>> {
        self.steering().get(&cid).map(|(_, sender)| sender.clone())
    }

    /// The row for `cid`, if the agent is connected.
    #[cfg(test)]
    pub(crate) fn agent(&self, cid: u32) -> Option<Agent> {
        self.registry().agents.get(&cid).cloned()
    }

    /// How many agents are connected right now.
    #[cfg(test)]
    pub(crate) fn agent_count(&self) -> usize {
        self.registry().agents.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use crate::serve::SessionLog;

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

    fn hub() -> (Arc<Recording>, Hub) {
        let log = Arc::new(Recording::default());
        let hub = Hub::new(Arc::clone(&log) as Arc<dyn SessionLog + Send + Sync>);
        (log, hub)
    }

    /// Every registry transition is logged in the daemon's voice, and a
    /// session's row is dropped by whichever path ends it.
    #[test]
    fn every_registry_transition_is_logged() {
        let (log, hub) = hub();
        let now = Instant::now();
        let (token, _steering) = hub.connect(10, "builder", now);
        assert_eq!(hub.agent_count(), 1);
        hub.heartbeat(10, 1, now);
        hub.note_calls(10, 1);
        hub.end_clean(10, token, 1);
        assert_eq!(hub.agent_count(), 0);

        let (token, _steering) = hub.connect(11, "", now);
        hub.reap_idle(11, token, Duration::from_secs(300));
        assert_eq!(hub.agent_count(), 0);

        assert_eq!(
            log.text(),
            [
                "vsockd: agent 10 role builder connected",
                "vsockd: agent 10 heartbeat 1",
                "vsockd: agent 10 session ended cleanly after 1 tool calls",
                "vsockd: agent 11 role  connected",
                "vsockd: agent 11 reaped idle > 300s",
            ]
            .join("\n")
        );
    }

    /// Hundreds of heartbeats must not flood the log: the first is logged
    /// (visibly alive) and then one line per [`HEARTBEAT_LOG_EVERY`] beats —
    /// and the count is the hub's own, not the guest's `seq`, so a guest that
    /// repeats one `seq` cannot beat the limiter.
    #[test]
    fn heartbeat_logs_are_rate_limited() {
        let (log, hub) = hub();
        let now = Instant::now();
        hub.connect(10, "builder", now);
        for _ in 0..(HEARTBEAT_LOG_EVERY * 3) {
            hub.heartbeat(10, 1, now);
        }
        let text = log.text();
        let logged: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("heartbeat"))
            .collect();
        assert_eq!(
            logged,
            vec![
                // the first beat, then every HEARTBEAT_LOG_EVERY-th of 36.
                "vsockd: agent 10 heartbeat 1",
                "vsockd: agent 10 heartbeat 1",
                "vsockd: agent 10 heartbeat 1",
                "vsockd: agent 10 heartbeat 1",
            ],
            "3 x HEARTBEAT_LOG_EVERY beats must log 4 lines, not 36: {text}"
        );
    }

    /// A reconnecting agent is adopted, and its predecessor's cleanup cannot
    /// evict it: the same CID is one agent, not two.
    #[test]
    fn a_reconnect_is_adopted_and_the_old_session_cannot_evict_it() {
        let (_log, hub) = hub();
        let now = Instant::now();
        let (first, _steering) = hub.connect(10, "builder", now);
        let (_second, _steering) = hub.connect(10, "builder", now);
        assert_eq!(hub.agent_count(), 1, "one CID is one agent");
        // The dropped session's cleanup runs after the reconnect.
        hub.hangup(10, first);
        assert_eq!(
            hub.agent_count(),
            1,
            "the predecessor's token no longer owns the row"
        );
    }

    /// A stale token cannot unregister a session it does not own, and the
    /// steering channel follows the row.
    #[test]
    fn a_stale_token_is_refused_and_the_steering_channel_follows_the_row() {
        let (_log, hub) = hub();
        let now = Instant::now();
        let (token, _steering) = hub.connect(10, "builder", now);
        assert!(hub.steer(10).is_some(), "a live session is steerable");
        hub.reap_idle(10, token, Duration::from_secs(300));
        assert!(hub.steer(10).is_none(), "an ended session is not steerable");
        assert!(!hub.forget(10, token), "a stale token owns nothing");
        assert_eq!(hub.agent_count(), 0);
    }

    /// The hub is shared by every session thread, so its registry has to
    /// survive concurrent connects, beats and ends — and end up empty when
    /// every session is gone.
    #[test]
    fn the_registry_survives_concurrent_sessions() {
        use std::thread;

        const SESSIONS: u32 = 16;
        let (log, hub) = hub();
        let hub = Arc::new(hub);
        let now = Instant::now();
        let sessions: Vec<_> = (0..SESSIONS)
            .map(|index| {
                let hub = Arc::clone(&hub);
                thread::spawn(move || {
                    let cid = 200 + index;
                    let (token, _steering) = hub.connect(cid, "builder", now);
                    for seq in 1..=20 {
                        hub.heartbeat(cid, seq, now);
                    }
                    hub.note_calls(cid, 3);
                    if index % 2 == 0 {
                        hub.end_clean(cid, token, 3);
                    } else {
                        hub.reap_idle(cid, token, Duration::from_secs(300));
                    }
                })
            })
            .collect();
        for session in sessions {
            session.join().expect("a session thread finished");
        }
        assert_eq!(hub.agent_count(), 0, "every session unregistered itself");
        let text = log.text();
        assert_eq!(
            text.lines().filter(|line| line.contains("connected")).count(),
            SESSIONS as usize,
            "{text}"
        );
        assert_eq!(
            text.lines().filter(|line| line.contains("ended cleanly")).count(),
            SESSIONS as usize / 2,
            "{text}"
        );
    }
}
