//! The takeover policies: what happens when the owner takes a workstation's screen session, or touches its keyboard
//! or pointer (docs/design/workstations.md sections 5 and 7). Each workstation's settings name one policy:
//!
//! - `agent_waits`: the agent's pixel work on the session (screenshots, clicks, keys) is refused with "the owner has
//!   the screen; wait", and its lease is marked paused, until the owner gives the session back.
//! - `release_lease`: the session's pixel lease is released (its session closed, the next queued lease granted), and
//!   `lease_released` is published with reason `owner_takeover`, so the task can go back to the queue.
//! - `keep_working_pointer_paused`: only the agent's pointer and keyboard are refused; screenshots, the UI tree,
//!   shells, runs and files continue.
//!
//! A session is taken two ways. The owner's `/control` socket takes it and holds it until the owner gives it back or
//! the socket closes. The station's `OwnerInput` event (the owner touched a session the agent is using) takes it too,
//! and that take is given back by itself once the owner has been quiet for [`QUIET`].
//!
//! Bus events: `station_takeover {station, session, by, policy, source}` (source `control` or `input`) and
//! `station_give_back {station, session, by, reason}` (reason `owner`, `quiet` or `session_ended`).
//!
//! The decisions are pure: they take the time as an argument and do their effects through [`Effects`], so the
//! policies are tested with a clock and a fake. [`Hub`] is the live effects.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_station::{SessionId, Takeover};
use serde_json::{Value, json};

use super::ops;
use crate::state::App;

/// How long the owner must be quiet before a session taken by its input is given back, in ms.
pub const QUIET: u64 = 5_000;

/// A session the owner has taken.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Taken {
    policy: Takeover,
    /// The control socket that holds the session (a control take), or none (an input take).
    holder: Option<u64>,
    /// When the owner last touched the session: the quiet timer of an input take.
    last_input: u64,
}

impl Taken {
    /// Whether the takeover still stands at `now`: a control take always does; an input take while the owner is not
    /// quiet for [`QUIET`].
    fn stands(&self, now: u64) -> bool {
        self.holder.is_some() || now.saturating_sub(self.last_input) < QUIET
    }
}

/// What a takeover does outside the table: the station audit, the bus, and the leases.
pub trait Effects {
    /// Writes one audit row for `session`. An error refuses the change the row records.
    fn audit(
        &self,
        station: &str,
        session: SessionId,
        kind: &str,
        data: Value,
    ) -> Result<(), String>;
    fn publish(&self, kind: &str, data: Value);
    /// Marks the lease holding `session` paused (or not).
    fn set_paused(&self, station: &str, session: SessionId, paused: bool);
    /// Releases the pixel lease holding `session`, when one does (the `release_lease` policy).
    fn release(&self, station: &str, session: SessionId) -> Result<(), String>;
}

type Key = (String, SessionId);

/// The sessions the owner has taken, by station and session.
#[derive(Default)]
pub struct Sessions {
    taken: Mutex<BTreeMap<Key, Taken>>,
    holders: AtomicU64,
}

impl Sessions {
    fn guard(&self) -> MutexGuard<'_, BTreeMap<Key, Taken>> {
        self.taken.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new control socket's holder id.
    pub fn next_holder(&self) -> u64 {
        self.holders.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Takes `session` for the control socket `holder` under `policy`, and answers the policy in force. A session
    /// another control socket holds is refused; one an input take holds passes to this socket. Nothing changes when
    /// the audit or the policy's first step fails.
    pub fn take(
        &self,
        fx: &dyn Effects,
        now: u64,
        station: &str,
        session: SessionId,
        holder: u64,
        policy: Takeover,
    ) -> Result<Takeover, String> {
        let key = (station.to_string(), session);
        let mut taken = self.guard();
        if let Some(current) = taken.get(&key).cloned() {
            if current.stands(now) {
                return match current.holder {
                    Some(held) if held == holder => Ok(current.policy),
                    Some(_) => Err("another owner view holds this session".into()),
                    None => {
                        taken.insert(
                            key,
                            Taken {
                                holder: Some(holder),
                                ..current.clone()
                            },
                        );
                        Ok(current.policy)
                    }
                };
            }
            // A stale input take ends before this one begins.
            taken.remove(&key);
            finish(fx, station, session, current.policy, "owner", "quiet");
        }
        fx.audit(
            station,
            session,
            "takeover",
            json!({"by": "owner", "source": "control", "policy": policy}),
        )?;
        start(fx, station, session, policy)?;
        taken.insert(
            key,
            Taken {
                policy,
                holder: Some(holder),
                last_input: now,
            },
        );
        fx.publish("station_takeover", json!({"station": station, "session": session, "by": "owner", "policy": policy, "source": "control"}));
        Ok(policy)
    }

    /// Gives `session` back from the control socket `holder`. Only the socket that holds it may.
    pub fn give_back(
        &self,
        fx: &dyn Effects,
        now: u64,
        station: &str,
        session: SessionId,
        holder: u64,
    ) -> Result<(), String> {
        let key = (station.to_string(), session);
        let mut taken = self.guard();
        let current = match taken.get(&key) {
            Some(t) if t.holder == Some(holder) && t.stands(now) => t.clone(),
            _ => return Err("this view does not hold the session".into()),
        };
        fx.audit(
            station,
            session,
            "give_back",
            json!({"by": "owner", "reason": "owner"}),
        )?;
        taken.remove(&key);
        finish(fx, station, session, current.policy, "owner", "owner");
        Ok(())
    }

    /// Whether the control socket `holder` holds `session` now.
    pub fn holds(&self, now: u64, station: &str, session: SessionId, holder: u64) -> bool {
        self.guard()
            .get(&(station.to_string(), session))
            .is_some_and(|t| t.holder == Some(holder) && t.stands(now))
    }

    /// The owner touched `session`: an input take starts (unless the session is already taken), or the quiet timer
    /// restarts.
    pub fn owner_input(
        &self,
        fx: &dyn Effects,
        now: u64,
        station: &str,
        session: SessionId,
        policy: Takeover,
    ) {
        let key = (station.to_string(), session);
        let mut taken = self.guard();
        if let Some(current) = taken.get_mut(&key) && current.stands(now) {
            current.last_input = now;
            return;
        }
        if let Some(stale) = taken.remove(&key) {
            finish(fx, station, session, stale.policy, "owner", "quiet");
        }
        if let Err(reason) = fx.audit(
            station,
            session,
            "takeover",
            json!({"by": "owner", "source": "input", "policy": policy}),
        ) {
            eprintln!(
                "apid: workstation {station}: the takeover of session {session} is not in the audit: {reason}"
            );
        }
        if let Err(reason) = start(fx, station, session, policy) {
            eprintln!(
                "apid: workstation {station}: the takeover of session {session} did not apply: {reason}"
            );
        }
        taken.insert(
            key,
            Taken {
                policy,
                holder: None,
                last_input: now,
            },
        );
        fx.publish("station_takeover", json!({"station": station, "session": session, "by": "owner", "policy": policy, "source": "input"}));
    }

    /// Ends the input takes the owner has been quiet on for [`QUIET`].
    pub fn sweep(&self, fx: &dyn Effects, now: u64) {
        let mut taken = self.guard();
        let quiet: Vec<Key> = taken
            .iter()
            .filter(|(_, t)| !t.stands(now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in quiet {
            if let Some(ended) = taken.remove(&key) {
                if let Err(reason) = fx.audit(
                    &key.0,
                    key.1,
                    "give_back",
                    json!({"by": "owner", "reason": "quiet"}),
                ) {
                    eprintln!(
                        "apid: workstation {}: the give back of session {} is not in the audit: {reason}",
                        key.0, key.1
                    );
                }
                finish(fx, &key.0, key.1, ended.policy, "owner", "quiet");
            }
        }
    }

    /// The session ended on the machine: its takeover ends with it.
    pub fn session_ended(&self, fx: &dyn Effects, station: &str, session: SessionId) {
        let ended = self.guard().remove(&(station.to_string(), session));
        if let Some(ended) = ended {
            finish(
                fx,
                station,
                session,
                ended.policy,
                "station",
                "session_ended",
            );
        }
    }

    /// Whether the agent's pixel work on `session` is refused: its screenshots and its pointer and keyboard, under
    /// `agent_waits` and `release_lease`.
    pub fn pixel_paused(&self, now: u64, station: &str, session: SessionId) -> bool {
        self.guard()
            .get(&(station.to_string(), session))
            .is_some_and(|t| t.stands(now) && t.policy != Takeover::KeepWorkingPointerPaused)
    }

    /// Whether the agent's pointer and keyboard on `session` are refused: under every policy while the owner holds it.
    pub fn pointer_paused(&self, now: u64, station: &str, session: SessionId) -> bool {
        self.guard()
            .get(&(station.to_string(), session))
            .is_some_and(|t| t.stands(now))
    }
}

/// What starts a takeover: `agent_waits` pauses the lease, `release_lease` releases it, and the pointer policy
/// changes nothing on the lease.
fn start(
    fx: &dyn Effects,
    station: &str,
    session: SessionId,
    policy: Takeover,
) -> Result<(), String> {
    match policy {
        Takeover::AgentWaits => {
            fx.set_paused(station, session, true);
            Ok(())
        }
        Takeover::ReleaseLease => fx.release(station, session),
        Takeover::KeepWorkingPointerPaused => Ok(()),
    }
}

/// What ends a takeover: the lease resumes, and the bus hears the give back.
fn finish(
    fx: &dyn Effects,
    station: &str,
    session: SessionId,
    policy: Takeover,
    by: &str,
    reason: &str,
) {
    if policy == Takeover::AgentWaits {
        fx.set_paused(station, session, false);
    }
    fx.publish(
        "station_give_back",
        json!({"station": station, "session": session, "by": by, "reason": reason}),
    );
}

/// The live effects: the station audit, the bus and the leases of the hub.
pub struct Hub<'a>(pub &'a App);

impl Effects for Hub<'_> {
    fn audit(
        &self,
        station: &str,
        session: SessionId,
        kind: &str,
        mut data: Value,
    ) -> Result<(), String> {
        if let Value::Object(map) = &mut data {
            map.insert("session".into(), json!(session));
        }
        ops::audit(self.0, Some(station), None, kind, "owner", data)
    }

    fn publish(&self, kind: &str, data: Value) {
        self.0.bus.publish(kind, &data);
    }

    fn set_paused(&self, station: &str, session: SessionId, paused: bool) {
        self.0.stations.leases.set_paused(station, session, paused);
    }

    fn release(&self, station: &str, session: SessionId) -> Result<(), String> {
        let Some(lease) = self.0.stations.leases.by_session(station, session) else {
            return Ok(());
        };
        ops::release_for_takeover(self.0, station, &lease.id)
            .map(|_| ())
            .map_err(|(_, message)| message)
    }
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

/// The control socket's take of `session`: the policy in force on the station.
pub fn take(app: &App, station: &str, session: SessionId, holder: u64) -> Result<Takeover, String> {
    let policy = app
        .stations
        .station(station)
        .ok_or("no such workstation")?
        .settings
        .takeover;
    app.stations
        .taken
        .take(&Hub(app), now(), station, session, holder, policy)
}

/// The control socket's give back of `session`.
pub fn give_back(app: &App, station: &str, session: SessionId, holder: u64) -> Result<(), String> {
    app.stations
        .taken
        .give_back(&Hub(app), now(), station, session, holder)
}

/// Whether the control socket `holder` holds `session` now.
pub fn holds(app: &App, station: &str, session: SessionId, holder: u64) -> bool {
    app.stations.taken.holds(now(), station, session, holder)
}

/// The station's `OwnerInput` event. It runs on its own thread: a takeover may release a lease, which waits on the
/// station's link, and the link's thread must keep answering.
pub fn owner_input(app: &App, station: &str, session: SessionId) {
    let Some(policy) = app.stations.station(station).map(|s| s.settings.takeover) else {
        return;
    };
    let weak = app.handle();
    let station = station.to_string();
    std::thread::spawn(move || {
        if let Some(app) = weak.upgrade() {
            app.stations
                .taken
                .owner_input(&Hub(app.as_ref()), now(), &station, session, policy);
        }
    });
}

/// The station's `SessionEnded` event, on its own thread for the same reason as [`owner_input`].
pub fn session_ended(app: &App, station: &str, session: SessionId) {
    let weak = app.handle();
    let station = station.to_string();
    std::thread::spawn(move || {
        if let Some(app) = weak.upgrade() {
            app.stations
                .taken
                .session_ended(&Hub(app.as_ref()), &station, session);
        }
    });
}

/// Ends the input takes whose owner has gone quiet. The ticker calls it.
pub fn sweep(app: &App) {
    app.stations.taken.sweep(&Hub(app), now());
}

/// Whether the agent's pixel work on `session` of `station` is refused: the owner has the screen (`agent_waits`
/// and `release_lease`). The agent's screenshots call it.
#[allow(dead_code)]
pub fn pixel_paused(app: &App, station: &str, session: SessionId) -> bool {
    app.stations.taken.pixel_paused(now(), station, session)
}

/// Whether the agent's pointer and keyboard on `session` of `station` are refused: the owner has the session under
/// any policy, including `keep_working_pointer_paused`, where screenshots still go through. Input calls it.
#[allow(dead_code)]
pub fn pointer_paused(app: &App, station: &str, session: SessionId) -> bool {
    app.stations.taken.pointer_paused(now(), station, session)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// The effects, recorded in order.
    #[derive(Default)]
    struct Fake {
        log: RefCell<Vec<String>>,
        refuse_audit: bool,
        refuse_release: bool,
    }

    impl Effects for Fake {
        fn audit(
            &self,
            station: &str,
            session: SessionId,
            kind: &str,
            _data: Value,
        ) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("audit {kind} {station}/{session}"));
            if self.refuse_audit {
                Err("the station audit cannot be written".into())
            } else {
                Ok(())
            }
        }

        fn publish(&self, kind: &str, data: Value) {
            self.log.borrow_mut().push(format!("publish {kind} {data}"));
        }

        fn set_paused(&self, station: &str, session: SessionId, paused: bool) {
            self.log
                .borrow_mut()
                .push(format!("paused {station}/{session} {paused}"));
        }

        fn release(&self, station: &str, session: SessionId) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("release {station}/{session}"));
            if self.refuse_release {
                Err("the lease could not be released".into())
            } else {
                Ok(())
            }
        }
    }

    impl Fake {
        fn lines(&self) -> Vec<String> {
            self.log.borrow().clone()
        }

        fn count(&self, prefix: &str) -> usize {
            self.log
                .borrow()
                .iter()
                .filter(|l| l.starts_with(prefix))
                .count()
        }
    }

    const T0: u64 = 1_000_000;

    #[test]
    fn agent_waits_refuses_the_pixels_until_the_owner_gives_the_session_back() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        assert_eq!(
            sessions.take(&fx, T0, "w1", 9, 1, Takeover::AgentWaits),
            Ok(Takeover::AgentWaits)
        );
        assert!(sessions.pixel_paused(T0, "w1", 9));
        assert!(sessions.pointer_paused(T0, "w1", 9));
        assert!(
            fx.lines().contains(&"paused w1/9 true".to_string()),
            "the lease is marked paused"
        );
        assert!(
            fx.lines()
                .iter()
                .any(|l| l.starts_with("publish station_takeover")
                    && l.contains("\"policy\":\"agent_waits\""))
        );

        sessions.give_back(&fx, T0 + 1, "w1", 9, 1).unwrap();
        assert!(!sessions.pixel_paused(T0 + 1, "w1", 9));
        assert!(!sessions.pointer_paused(T0 + 1, "w1", 9));
        assert!(
            fx.lines().contains(&"paused w1/9 false".to_string()),
            "and resumes"
        );
        assert!(
            fx.lines()
                .iter()
                .any(|l| l.starts_with("publish station_give_back")
                    && l.contains("\"reason\":\"owner\""))
        );
    }

    #[test]
    fn keep_working_refuses_only_the_pointer_and_leaves_the_lease_alone() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions
            .take(&fx, T0, "w1", 9, 1, Takeover::KeepWorkingPointerPaused)
            .unwrap();
        assert!(!sessions.pixel_paused(T0, "w1", 9), "screenshots continue");
        assert!(
            sessions.pointer_paused(T0, "w1", 9),
            "the pointer and keyboard pause"
        );
        assert_eq!(fx.count("paused"), 0);
        assert_eq!(fx.count("release"), 0);
    }

    #[test]
    fn release_lease_releases_the_session_once_and_the_owner_holds_it() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions
            .take(&fx, T0, "w1", 9, 1, Takeover::ReleaseLease)
            .unwrap();
        assert_eq!(fx.count("release w1/9"), 1);
        assert_eq!(fx.count("paused"), 0, "the lease is released, not paused");
        assert!(sessions.pixel_paused(T0, "w1", 9));
    }

    #[test]
    fn a_failed_release_or_audit_refuses_the_takeover_and_changes_nothing() {
        let sessions = Sessions::default();
        let fx = Fake {
            refuse_release: true,
            ..Fake::default()
        };
        assert!(
            sessions
                .take(&fx, T0, "w1", 9, 1, Takeover::ReleaseLease)
                .is_err()
        );
        assert!(!sessions.holds(T0, "w1", 9, 1));

        let fx = Fake {
            refuse_audit: true,
            ..Fake::default()
        };
        assert!(
            sessions
                .take(&fx, T0, "w1", 9, 1, Takeover::AgentWaits)
                .is_err()
        );
        assert_eq!(fx.count("paused"), 0, "no pause without the audit row");
        assert!(!sessions.pixel_paused(T0, "w1", 9));
    }

    #[test]
    fn one_owner_view_holds_a_session_at_a_time() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions
            .take(&fx, T0, "w1", 9, 1, Takeover::AgentWaits)
            .unwrap();
        assert!(
            sessions
                .take(&fx, T0, "w1", 9, 2, Takeover::AgentWaits)
                .is_err(),
            "another view is refused"
        );
        assert!(
            sessions.give_back(&fx, T0, "w1", 9, 2).is_err(),
            "and cannot give it back"
        );
        assert_eq!(
            sessions.take(&fx, T0, "w1", 9, 1, Takeover::AgentWaits),
            Ok(Takeover::AgentWaits),
            "the holder takes it again"
        );
        sessions.give_back(&fx, T0, "w1", 9, 1).unwrap();
        assert!(
            sessions
                .take(&fx, T0, "w1", 9, 2, Takeover::AgentWaits)
                .is_ok(),
            "free once given back"
        );
    }

    #[test]
    fn an_owner_input_pauses_the_agent_and_gives_the_session_back_after_five_seconds_quiet() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions.owner_input(&fx, T0, "w1", 9, Takeover::AgentWaits);
        assert!(sessions.pixel_paused(T0, "w1", 9));
        assert!(
            sessions.pixel_paused(T0 + QUIET - 1, "w1", 9),
            "still held just before the quiet time"
        );

        sessions.sweep(&fx, T0 + QUIET);
        assert!(!sessions.pixel_paused(T0 + QUIET, "w1", 9));
        assert!(!sessions.pointer_paused(T0 + QUIET, "w1", 9));
        assert!(fx.lines().contains(&"paused w1/9 false".to_string()));
        assert!(
            fx.lines()
                .iter()
                .any(|l| l.starts_with("publish station_give_back")
                    && l.contains("\"reason\":\"quiet\""))
        );
    }

    #[test]
    fn each_owner_input_restarts_the_quiet_time_and_only_the_first_is_a_takeover() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions.owner_input(&fx, T0, "w1", 9, Takeover::KeepWorkingPointerPaused);
        sessions.owner_input(&fx, T0 + 4_000, "w1", 9, Takeover::KeepWorkingPointerPaused);
        assert!(
            sessions.pointer_paused(T0 + 8_000, "w1", 9),
            "the timer restarted at 4 s, so 8 s is still inside it"
        );
        sessions.sweep(&fx, T0 + 8_999);
        assert!(sessions.pointer_paused(T0 + 8_999, "w1", 9));
        assert!(
            !sessions.pointer_paused(T0 + 9_000, "w1", 9),
            "five seconds after the last input"
        );
        assert_eq!(
            fx.count("publish station_takeover"),
            1,
            "one takeover event for a run of input"
        );
    }

    #[test]
    fn a_control_take_holds_through_any_quiet_time() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions
            .take(&fx, T0, "w1", 9, 1, Takeover::AgentWaits)
            .unwrap();
        sessions.sweep(&fx, T0 + 10 * QUIET);
        assert!(
            sessions.pixel_paused(T0 + 10 * QUIET, "w1", 9),
            "held until the owner gives it back"
        );
    }

    #[test]
    fn an_input_take_passes_to_the_view_that_takes_it_and_is_held_there() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions.owner_input(&fx, T0, "w1", 9, Takeover::AgentWaits);
        assert_eq!(
            sessions.take(&fx, T0 + 1_000, "w1", 9, 3, Takeover::AgentWaits),
            Ok(Takeover::AgentWaits)
        );
        sessions.sweep(&fx, T0 + 60_000);
        assert!(
            sessions.holds(T0 + 60_000, "w1", 9, 3),
            "the view holds it, so the quiet time does not end it"
        );
        assert_eq!(fx.count("publish station_takeover"), 1);
    }

    #[test]
    fn a_session_that_ends_ends_its_takeover_and_the_bus_hears_why() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions
            .take(&fx, T0, "w1", 9, 1, Takeover::AgentWaits)
            .unwrap();
        sessions.session_ended(&fx, "w1", 9);
        assert!(!sessions.pixel_paused(T0, "w1", 9));
        assert!(fx.lines().contains(&"paused w1/9 false".to_string()));
        assert!(
            fx.lines()
                .iter()
                .any(|l| l.starts_with("publish station_give_back")
                    && l.contains("\"reason\":\"session_ended\""))
        );
    }

    #[test]
    fn a_stale_input_take_is_ended_before_a_view_takes_the_session() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions.owner_input(&fx, T0, "w1", 9, Takeover::AgentWaits);
        assert_eq!(
            sessions.take(&fx, T0 + QUIET + 1, "w1", 9, 4, Takeover::AgentWaits),
            Ok(Takeover::AgentWaits)
        );
        assert!(sessions.holds(T0 + QUIET + 1, "w1", 9, 4));
        assert!(
            fx.lines()
                .iter()
                .any(|l| l.starts_with("publish station_give_back")
                    && l.contains("\"reason\":\"quiet\""))
        );
    }

    #[test]
    fn a_takeover_is_for_one_station_and_one_session() {
        let fx = Fake::default();
        let sessions = Sessions::default();
        sessions
            .take(&fx, T0, "w1", 9, 1, Takeover::AgentWaits)
            .unwrap();
        assert!(!sessions.pixel_paused(T0, "w2", 9), "another station");
        assert!(!sessions.pixel_paused(T0, "w1", 10), "another session");
    }
}
