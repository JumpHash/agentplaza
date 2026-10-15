//! The store without Postgres: every tier in bounded memory (10 s for the chunk retention, capped at 6 h; 1 min
//! for 24 h; 1 h for the hour retention), events, calls and traces in bounded rings, and saved values as JSON files
//! under `<state>/metrics/<key>.json` so the owner's settings, rules and pins survive a restart.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

use apz_metrics::agg::{Agg, Closed, Tier};
use apz_metrics::trace::Trace;
use apz_metrics::{Event, SeriesKey};

use super::settings::Retention;
use super::store::{CallRecord, Store};

/// Most events, calls and traces kept in memory.
pub const MAX_EVENTS: usize = 20_000;
pub const MAX_CALLS: usize = 20_000;
pub const MAX_TRACES: usize = 2_000;

#[derive(Default)]
struct Inner {
    tiers: BTreeMap<(Tier, String), BTreeMap<SeriesKey, VecDeque<(u64, Agg)>>>,
    events: VecDeque<Event>,
    calls: VecDeque<CallRecord>,
    traces: VecDeque<Trace>,
    kv: BTreeMap<String, String>,
}

pub struct MemStore {
    dir: Option<PathBuf>,
    inner: Mutex<Inner>,
}

impl MemStore {
    /// A store whose saved values live under `dir` (none: in memory only, for tests).
    pub fn new(dir: Option<PathBuf>) -> MemStore {
        MemStore {
            dir,
            inner: Mutex::new(Inner::default()),
        }
    }
}

impl Store for MemStore {
    fn put(&self, closed: &[Closed]) {
        let _ = (closed, &self.inner);
        todo!()
    }
    fn rows(
        &self,
        tier: Tier,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&SeriesKey) -> bool,
    ) -> Vec<(SeriesKey, Vec<(u64, Agg)>)> {
        let _ = (tier, metric, from_ms, to_ms, keep);
        todo!()
    }
    fn put_events(&self, events: &[Event]) {
        let _ = events;
        todo!()
    }
    fn events(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Event) -> bool,
        limit: usize,
    ) -> Vec<Event> {
        let _ = (from_ms, to_ms, keep, limit);
        todo!()
    }
    fn put_calls(&self, calls: &[CallRecord]) {
        let _ = calls;
        todo!()
    }
    fn calls(
        &self,
        before_ms: u64,
        limit: usize,
        keep: &dyn Fn(&CallRecord) -> bool,
    ) -> Vec<CallRecord> {
        let _ = (before_ms, limit, keep);
        todo!()
    }
    fn put_trace(&self, trace: &Trace) {
        let _ = trace;
        todo!()
    }
    fn traces(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Trace) -> bool,
        limit: usize,
    ) -> Vec<Trace> {
        let _ = (from_ms, to_ms, keep, limit);
        todo!()
    }
    fn trace(&self, id: &str) -> Option<Trace> {
        let _ = id;
        todo!()
    }
    fn kv_get(&self, key: &str) -> Option<String> {
        let _ = (key, &self.dir);
        todo!()
    }
    fn kv_put(&self, key: &str, json: &str) {
        let _ = (key, json);
        todo!()
    }
    fn prune(&self, now_ms: u64, retention: &Retention) {
        let _ = (now_ms, retention);
        todo!()
    }
    fn sizes(&self) -> Vec<(String, u64)> {
        todo!()
    }
    fn flush(&self) {}
    fn kind(&self) -> &'static str {
        "memory"
    }
}
