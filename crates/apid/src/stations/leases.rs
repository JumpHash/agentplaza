//! Leases: an agent's claim on a workstation for one task.
//!
//! A shared lease is the agent's use of the owner's own session: it is recorded, and the daemon is not
//! asked for anything. A pixel lease takes a screen session the agent works in alone (`Op::SessionOpen`,
//! ended with `Op::SessionClose`). How many pixel sessions run at once is what the machine supports
//! ([`pixel_capacity`]); the rest wait in a FIFO queue. An agent holds at most one pixel lease per
//! workstation. Leases live in memory: a lease belongs to a live link, so a restart ends them all, and
//! each change is in the station audit.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use apz_station::{Facts, Form};
use serde::{Deserialize, Serialize};

/// What a lease is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Shared,
    Pixel,
}

/// Where a lease is. Only pixel leases are ever `Queued` or `Opening`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Waiting for a pixel session to be free.
    Queued,
    /// The session is being opened on the machine.
    Opening,
    Active,
}

/// One lease: `l<seq>`, for one agent (its CID) on one station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub id: String,
    pub station: String,
    pub agent: u64,
    pub task: Option<String>,
    pub kind: Kind,
    /// The pixel session the daemon opened (pixel leases, once active).
    pub session: Option<u64>,
    pub started_ms: u64,
    pub state: State,
    /// The owner has taken the session and the agent waits for it to be given back (`agent_waits`, takeover.rs).
    #[serde(default)]
    pub paused: bool,
}

/// A lease as the console and the agents see it: its position in the queue while it waits (1 is next).
#[derive(Debug, Clone, Serialize)]
pub struct LeaseView {
    #[serde(flatten)]
    pub lease: Lease,
    pub position: Option<usize>,
}

/// How many pixel sessions a machine runs at once, or `None` when every agent gets its own. A machine with
/// virtual displays gives one to each agent (Linux and macOS, plan section 5), and so does a server or a VM
/// (one session per agent). A client desktop with no virtual display has one session, shared, so the rest queue.
pub fn pixel_capacity(facts: &Facts) -> Option<usize> {
    if facts.virtual_displays || matches!(facts.form, Form::Server | Form::Vm) {
        None
    } else {
        Some(1)
    }
}

/// Every lease the hub holds, in the order they were opened (the queue's order).
#[derive(Default)]
pub struct Leases {
    list: Mutex<Vec<Lease>>,
    seq: AtomicU64,
}

impl Leases {
    fn guard(&self) -> std::sync::MutexGuard<'_, Vec<Lease>> {
        self.list.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opens a lease. A shared one is active at once; a pixel one is queued until a session is free.
    pub fn open(&self, station: &str, agent: u64, task: Option<String>, kind: Kind, now: u64) -> Result<Lease, String> {
        let mut list = self.guard();
        if kind == Kind::Pixel && list.iter().any(|l| l.station == station && l.agent == agent && l.kind == Kind::Pixel) {
            return Err("this agent already holds a pixel lease on this workstation".into());
        }
        let state = if kind == Kind::Pixel { State::Queued } else { State::Active };
        let lease = Lease {
            id: format!("l{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1),
            station: station.to_string(),
            agent,
            task,
            kind,
            session: None,
            started_ms: now,
            state,
            paused: false,
        };
        list.push(lease.clone());
        Ok(lease)
    }

    /// Takes the first queued pixel lease of `station` when the machine has room for one more session, and marks
    /// it `Opening`. `None` when nothing is queued or the sessions are full.
    pub fn grant_next(&self, station: &str, capacity: Option<usize>) -> Option<Lease> {
        let mut list = self.guard();
        let busy = list.iter().filter(|l| l.station == station && l.kind == Kind::Pixel && l.state != State::Queued).count();
        if capacity.is_some_and(|cap| busy >= cap) {
            return None;
        }
        let next = list.iter_mut().find(|l| l.station == station && l.kind == Kind::Pixel && l.state == State::Queued)?;
        next.state = State::Opening;
        Some(next.clone())
    }

    /// Makes an opening lease active on `session`. False when the lease was released while it opened: the caller
    /// then gives the session back.
    pub fn granted(&self, id: &str, session: u64) -> bool {
        let mut list = self.guard();
        match list.iter_mut().find(|l| l.id == id) {
            Some(lease) if lease.state == State::Opening => {
                lease.state = State::Active;
                lease.session = Some(session);
                true
            }
            _ => false,
        }
    }

    /// Puts an opening lease back in the queue (its link went away before the session opened).
    pub fn requeue(&self, id: &str) {
        if let Some(lease) = self.guard().iter_mut().find(|l| l.id == id && l.state == State::Opening) {
            lease.state = State::Queued;
        }
    }

    pub fn get(&self, id: &str) -> Option<Lease> {
        self.guard().iter().find(|l| l.id == id).cloned()
    }

    /// Removes a lease and returns it.
    pub fn remove(&self, id: &str) -> Option<Lease> {
        let mut list = self.guard();
        let at = list.iter().position(|l| l.id == id)?;
        Some(list.remove(at))
    }

    /// The lease whose pixel session is `session` on `station`.
    pub fn by_session(&self, station: &str, session: u64) -> Option<Lease> {
        self.guard().iter().find(|l| l.station == station && l.session == Some(session)).cloned()
    }

    /// Marks the lease of `session` on `station` paused (or not). False when no lease holds that session.
    pub fn set_paused(&self, station: &str, session: u64, paused: bool) -> bool {
        match self.guard().iter_mut().find(|l| l.station == station && l.session == Some(session)) {
            Some(lease) => {
                lease.paused = paused;
                true
            }
            None => false,
        }
    }

    /// Every lease of a station, in order, with each queued lease's position.
    pub fn views(&self, station: &str) -> Vec<LeaseView> {
        let list = self.guard();
        let mut queued = 0;
        list.iter()
            .filter(|l| l.station == station)
            .map(|lease| {
                let position = (lease.state == State::Queued).then(|| {
                    queued += 1;
                    queued
                });
                LeaseView { lease: lease.clone(), position }
            })
            .collect()
    }

    /// Removes every lease of a station (a revoked station's), and returns them.
    pub fn drop_station(&self, station: &str) -> Vec<Lease> {
        let mut list = self.guard();
        let (gone, kept): (Vec<Lease>, Vec<Lease>) = list.drain(..).partition(|l| l.station == station);
        *list = kept;
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::{AccountMode, Os};

    fn facts(os: Os, form: Form, virtual_displays: bool) -> Facts {
        Facts {
            os,
            os_version: String::new(),
            arch: "x86_64".into(),
            hostname: "box".into(),
            form,
            cpus: 4,
            ram_mb: 8192,
            gpus: vec![],
            displays: vec![],
            account: AccountMode::AgentUser { name: "agentws".into() },
            apps: 0,
            virtual_displays,
            ui_tree: true,
            machine_id: None,
        }
    }

    #[test]
    fn capacity_follows_what_the_machine_supports() {
        assert_eq!(pixel_capacity(&facts(Os::Linux, Form::Desktop, true)), None, "virtual displays: one per agent");
        assert_eq!(pixel_capacity(&facts(Os::Macos, Form::Mini, true)), None);
        assert_eq!(pixel_capacity(&facts(Os::Windows, Form::Laptop, false)), Some(1), "a Windows client shares one session");
        assert_eq!(pixel_capacity(&facts(Os::Windows, Form::Server, false)), None, "a Windows server gives one per agent");
        assert_eq!(pixel_capacity(&facts(Os::Linux, Form::Vm, false)), None, "a VM gives one per agent");
        assert_eq!(pixel_capacity(&facts(Os::Macos, Form::Laptop, false)), Some(1));
    }

    #[test]
    fn shared_leases_are_active_and_unlimited() {
        let leases = Leases::default();
        for agent in 1..=5 {
            let lease = leases.open("w1", agent, None, Kind::Shared, 10).unwrap();
            assert_eq!(lease.state, State::Active);
            assert_eq!(lease.session, None);
        }
        assert_eq!(leases.views("w1").len(), 5);
        assert!(leases.views("w1").iter().all(|v| v.position.is_none()));
    }

    #[test]
    fn a_pixel_lease_queues_when_the_machine_has_no_room_and_leaves_in_order() {
        let leases = Leases::default();
        let first = leases.open("w1", 1, Some("render".into()), Kind::Pixel, 10).unwrap();
        let second = leases.open("w1", 2, None, Kind::Pixel, 11).unwrap();
        let third = leases.open("w1", 3, None, Kind::Pixel, 12).unwrap();
        assert_eq!(first.state, State::Queued);

        let granted = leases.grant_next("w1", Some(1)).expect("the first one fits");
        assert_eq!(granted.id, first.id, "FIFO");
        assert_eq!(granted.state, State::Opening);
        assert!(leases.grant_next("w1", Some(1)).is_none(), "one session is open and the second waits");
        assert!(leases.granted(&first.id, 9));
        assert_eq!(leases.by_session("w1", 9).map(|l| l.id), Some(first.id.clone()));

        let positions: Vec<Option<usize>> = leases.views("w1").iter().map(|v| v.position).collect();
        assert_eq!(positions, vec![None, Some(1), Some(2)]);
        assert_eq!(leases.remove(&first.id).map(|l| l.id), Some(first.id));
        assert_eq!(leases.grant_next("w1", Some(1)).map(|l| l.id), Some(second.id.clone()));
        assert_eq!(leases.grant_next("w1", Some(1)), None, "the session is full again");
        assert_ne!(third.id, second.id);
    }

    #[test]
    fn unlimited_machines_grant_every_pixel_lease_at_once() {
        let leases = Leases::default();
        for agent in 1..=4 {
            leases.open("w2", agent, None, Kind::Pixel, 10).unwrap();
        }
        let mut granted = 0;
        while let Some(lease) = leases.grant_next("w2", None) {
            assert!(leases.granted(&lease.id, 100 + granted));
            granted += 1;
        }
        assert_eq!(granted, 4);
    }

    #[test]
    fn an_agent_holds_one_pixel_lease_per_workstation() {
        let leases = Leases::default();
        leases.open("w1", 7, None, Kind::Pixel, 1).unwrap();
        assert!(leases.open("w1", 7, None, Kind::Pixel, 2).is_err());
        assert!(leases.open("w2", 7, None, Kind::Pixel, 2).is_ok(), "another station is another place");
        assert!(leases.open("w1", 7, None, Kind::Shared, 2).is_ok(), "a shared lease is not a pixel one");
    }

    #[test]
    fn a_release_while_opening_is_seen_by_the_grant_and_a_requeue_puts_it_back() {
        let leases = Leases::default();
        let lease = leases.open("w1", 1, None, Kind::Pixel, 1).unwrap();
        leases.grant_next("w1", None).unwrap();
        leases.remove(&lease.id);
        assert!(!leases.granted(&lease.id, 5), "the grant learns the lease is gone");

        let again = leases.open("w1", 2, None, Kind::Pixel, 2).unwrap();
        leases.grant_next("w1", Some(1)).unwrap();
        leases.requeue(&again.id);
        assert_eq!(leases.get(&again.id).unwrap().state, State::Queued);
    }

    #[test]
    fn dropping_a_station_takes_only_its_leases() {
        let leases = Leases::default();
        leases.open("w1", 1, None, Kind::Shared, 1).unwrap();
        leases.open("w2", 2, None, Kind::Shared, 1).unwrap();
        assert_eq!(leases.drop_station("w1").len(), 1);
        assert!(leases.views("w1").is_empty());
        assert_eq!(leases.views("w2").len(), 1);
    }
}
