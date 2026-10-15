//! Handles registered once, recorded into with one atomic operation. A component asks the registry for a handle
//! when it first needs one (`registry.hist(key)`) and keeps it; the hot path never looks a name up.
//!
//! Each entity kind has a cap on its series. Past the cap, a new key's handle is the shared overflow series of
//! that metric, whose entity is [`OTHER`]; the number of folded keys is counted (`Registry::folded`).
//!
//! `scrape` turns the registry into [`Sample`]s: a counter yields its increase since the last scrape, a gauge
//! its current value (only when set since the last scrape, or always when `all_gauges`), a histogram the values
//! recorded since the last scrape. Series with nothing new yield nothing.

use crate::hist::{AtomicHist, HistSnapshot};
use crate::{EntityKind, SeriesKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

/// The entity of the overflow series.
pub const OTHER: &str = "other";
/// Default series cap per entity kind.
pub const DEFAULT_CAP: usize = 4_000;

/// A running total that may be fractional (spend in USD). Stored as f64 bits; adds use compare-and-swap.
#[derive(Clone, Debug, Default)]
pub struct Counter(Arc<CounterCell>);

#[derive(Debug, Default)]
pub struct CounterCell {
    total_bits: AtomicU64,
    scraped_bits: AtomicU64,
}

impl Counter {
    pub fn add(&self, v: f64) {
        let _ = (&self.0.total_bits, v);
        todo!("CAS loop adding v to total_bits")
    }
    pub fn inc(&self) {
        self.add(1.0)
    }
    /// The total so far.
    pub fn total(&self) -> f64 {
        todo!()
    }
}

/// A level at a moment. Stored as f64 bits; `set` also marks it fresh for the next scrape.
#[derive(Clone, Debug, Default)]
pub struct Gauge(Arc<GaugeCell>);

#[derive(Debug, Default)]
pub struct GaugeCell {
    bits: AtomicU64,
    fresh: std::sync::atomic::AtomicBool,
}

impl Gauge {
    pub fn set(&self, v: f64) {
        let _ = (&self.0.bits, &self.0.fresh, v);
        todo!()
    }
    pub fn get(&self) -> f64 {
        todo!()
    }
}

/// A distribution.
#[derive(Clone, Default)]
pub struct Hist(pub Arc<AtomicHist>);

impl Hist {
    pub fn record(&self, v: f64) {
        self.0.record(v)
    }
}

/// What a scrape yields for one series.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    /// The increase since the last scrape.
    Counter(f64),
    Gauge(f64),
    /// The values recorded since the last scrape.
    Hist(HistSnapshot),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub key: SeriesKey,
    pub t_ms: u64,
    pub value: Value,
}

#[derive(Default)]
struct Inner {
    counters: HashMap<SeriesKey, Counter>,
    gauges: HashMap<SeriesKey, Gauge>,
    hists: HashMap<SeriesKey, Hist>,
    per_kind: HashMap<EntityKind, usize>,
    folded: u64,
}

/// The handles of one process.
pub struct Registry {
    inner: Mutex<Inner>,
    cap: usize,
}

impl Default for Registry {
    fn default() -> Self {
        Registry::new(DEFAULT_CAP)
    }
}

impl Registry {
    /// A registry allowing `cap` series per entity kind.
    pub fn new(cap: usize) -> Registry {
        Registry {
            inner: Mutex::new(Inner::default()),
            cap,
        }
    }

    /// The counter for `key` (created on first use; the overflow series past the cap).
    pub fn counter(&self, key: SeriesKey) -> Counter {
        let _ = (&self.inner, self.cap, key);
        todo!()
    }

    pub fn gauge(&self, key: SeriesKey) -> Gauge {
        let _ = key;
        todo!()
    }

    pub fn hist(&self, key: SeriesKey) -> Hist {
        let _ = key;
        todo!()
    }

    /// Everything new since the last scrape, stamped `now_ms`. `all_gauges` includes gauges not set since.
    pub fn scrape(&self, now_ms: u64, all_gauges: bool) -> Vec<Sample> {
        let _ = (now_ms, all_gauges);
        todo!()
    }

    /// Live series.
    pub fn len(&self) -> usize {
        todo!()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keys folded into overflow series so far.
    pub fn folded(&self) -> u64 {
        todo!()
    }

    /// Drops every series of an entity (an agent deleted, a VM gone), freeing its place under the cap.
    pub fn forget(&self, kind: EntityKind, entity: &str) {
        let _ = (kind, entity);
        todo!()
    }
}
