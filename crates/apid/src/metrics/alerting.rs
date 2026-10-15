//! Alert rules on the hub: the starter rules plus the owner's, their states, mutes, acknowledgements and history,
//! saved through the store (`alerts` saved value). Checks run on the tick against the open 1-minute windows (and,
//! for daily sums, the day so far); a transition is published (`metrics.alert` on the bus, which the consoles turn
//! into a banner and a desktop notification), recorded as an `alert` event, and optionally posted to the board.

use std::sync::Mutex;

use apz_metrics::alerts::{Rule, Tracker, Transition};
use apz_metrics_view::source::AlertsState;

use super::Metrics;
use super::store::Store;
use crate::state::App;

/// Saved alert state.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    /// The owner's rules and the starters' on/off.
    pub rules: Vec<Rule>,
    pub tracker: Tracker,
    pub muted: Vec<(String, u64)>,
    pub acked: Vec<(String, String)>,
    /// Newest first, at most 500.
    pub history: Vec<(u64, String, String, String, f64, Option<u64>)>,
}

pub struct Alerts {
    inner: Mutex<Saved>,
}

impl Alerts {
    /// Loads saved alerts, adding any starter rule not yet saved.
    pub fn load(store: &dyn Store) -> Alerts {
        let _ = store;
        todo!()
    }

    pub fn rules(&self) -> Vec<Rule> {
        todo!()
    }

    /// Adds or replaces a rule (a new id when empty). A starter rule may only be switched on or off.
    pub fn put_rule(&self, store: &dyn Store, rule: Rule) -> Result<Rule, String> {
        let _ = (store, rule, &self.inner);
        todo!()
    }

    /// Deletes an owner's rule (starters cannot be deleted).
    pub fn delete(&self, store: &dyn Store, id: &str) -> Result<(), String> {
        let _ = (store, id);
        todo!()
    }

    pub fn mute(&self, store: &dyn Store, id: &str, until_ms: u64, now_ms: u64) {
        let _ = (store, id, until_ms, now_ms);
        todo!()
    }

    pub fn ack(&self, store: &dyn Store, id: &str, subject: &str, now_ms: u64) {
        let _ = (store, id, subject, now_ms);
        todo!()
    }

    /// Checks every rule that is on against current values, recording transitions.
    pub fn check(&self, m: &Metrics, app: &App, now_ms: u64) -> Vec<Transition> {
        let _ = (m, app, now_ms);
        todo!()
    }

    /// What the Alerts tab shows.
    pub fn state(&self, m: &Metrics, app: &App, now_ms: u64) -> AlertsState {
        let _ = (m, app, now_ms);
        todo!()
    }
}
