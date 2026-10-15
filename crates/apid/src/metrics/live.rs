//! Scopes watched live. A console holds a lease (`POST /v1/metrics/live`, renewed every 10 s while visible); while
//! any lease is active the hub scrapes every 2 s, asks the nodes under the leased scopes for 2 s batches, and every
//! 2 s recomposes each leased tab's live panels into a `metrics` event ([`apz_metrics_view::LiveFrame`]).

use std::collections::HashMap;
use std::sync::Mutex;

use apz_metrics_view::LiveLease;

use super::Metrics;
use crate::state::App;

/// A lease lapses this long after its last renewal.
pub const LEASE_MS: u64 = 15_000;

#[derive(Default)]
pub struct Leases {
    inner: Mutex<HashMap<LiveLease, u64>>,
}

impl Leases {
    /// Takes or renews a lease.
    pub fn lease(&self, l: LiveLease, now_ms: u64) {
        super::lock(&self.inner).insert(l, now_ms + LEASE_MS);
    }

    /// The leases still active (lapsed ones are dropped).
    pub fn active(&self, now_ms: u64) -> Vec<LiveLease> {
        let mut map = super::lock(&self.inner);
        map.retain(|_, until| *until > now_ms);
        map.keys().cloned().collect()
    }
}

/// Publishes one frame per active lease, and keeps the nodes under leased scopes streaming.
pub fn frames(m: &Metrics, app: &App, now_ms: u64) {
    let _ = (m, app, now_ms);
    todo!()
}
