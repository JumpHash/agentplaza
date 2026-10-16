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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The entity of the overflow series.
pub const OTHER: &str = "other";
/// Default series cap per entity kind: room for a plaza of a few thousand agents (about 24 series each on the hub,
/// measured by `tests/scale.rs`), while a runaway label still cannot grow memory without bound.
pub const DEFAULT_CAP: usize = 100_000;

/// A running total that may be fractional (spend in USD). Stored as f64 bits; adds use compare-and-swap.
#[derive(Clone, Debug, Default)]
pub struct Counter(Arc<CounterCell>);

#[derive(Debug, Default)]
pub struct CounterCell {
    total_bits: AtomicU64,
    scraped_bits: AtomicU64,
}

impl Counter {
    /// Adds `v` to the total. A compare-and-swap loop, so concurrent adds never lose an increment.
    pub fn add(&self, v: f64) {
        let cell = &self.0.total_bits;
        let mut current = cell.load(Ordering::Relaxed);
        loop {
            let next = (f64::from_bits(current) + v).to_bits();
            match cell.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return,
                Err(seen) => current = seen,
            }
        }
    }
    pub fn inc(&self) {
        self.add(1.0)
    }
    /// The total so far.
    pub fn total(&self) -> f64 {
        f64::from_bits(self.0.total_bits.load(Ordering::Relaxed))
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
    /// Sets the level and marks it fresh, so the next scrape yields it.
    pub fn set(&self, v: f64) {
        self.0.bits.store(v.to_bits(), Ordering::Relaxed);
        self.0.fresh.store(true, Ordering::Release);
    }
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.bits.load(Ordering::Relaxed))
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
        let mut guard = self.inner.lock().unwrap();
        let inner = &mut *guard;
        series(
            &mut inner.counters,
            &mut inner.per_kind,
            &mut inner.folded,
            self.cap,
            key,
        )
    }

    pub fn gauge(&self, key: SeriesKey) -> Gauge {
        let mut guard = self.inner.lock().unwrap();
        let inner = &mut *guard;
        series(
            &mut inner.gauges,
            &mut inner.per_kind,
            &mut inner.folded,
            self.cap,
            key,
        )
    }

    pub fn hist(&self, key: SeriesKey) -> Hist {
        let mut guard = self.inner.lock().unwrap();
        let inner = &mut *guard;
        series(
            &mut inner.hists,
            &mut inner.per_kind,
            &mut inner.folded,
            self.cap,
            key,
        )
    }

    /// Everything new since the last scrape, stamped `now_ms`. `all_gauges` includes gauges not set since.
    pub fn scrape(&self, now_ms: u64, all_gauges: bool) -> Vec<Sample> {
        let inner = self.inner.lock().unwrap();
        let mut samples = Vec::new();
        for (key, counter) in &inner.counters {
            let total = counter.total();
            let scraped = f64::from_bits(counter.0.scraped_bits.load(Ordering::Relaxed));
            let increase = total - scraped;
            if increase > 0.0 {
                counter
                    .0
                    .scraped_bits
                    .store(total.to_bits(), Ordering::Relaxed);
                samples.push(Sample {
                    key: key.clone(),
                    t_ms: now_ms,
                    value: Value::Counter(increase),
                });
            }
        }
        for (key, gauge) in &inner.gauges {
            let fresh = gauge.0.fresh.swap(false, Ordering::Acquire);
            if fresh || all_gauges {
                samples.push(Sample {
                    key: key.clone(),
                    t_ms: now_ms,
                    value: Value::Gauge(gauge.get()),
                });
            }
        }
        for (key, hist) in &inner.hists {
            let snapshot = hist.0.take();
            if !snapshot.is_empty() {
                samples.push(Sample {
                    key: key.clone(),
                    t_ms: now_ms,
                    value: Value::Hist(snapshot),
                });
            }
        }
        samples.sort_by(|a, b| a.key.cmp(&b.key));
        samples
    }

    /// Live series.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.counters.len() + inner.gauges.len() + inner.hists.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keys folded into overflow series so far.
    pub fn folded(&self) -> u64 {
        self.inner.lock().unwrap().folded
    }

    /// Drops every series of an entity (an agent deleted, a VM gone), freeing its place under the cap.
    pub fn forget(&self, kind: EntityKind, entity: &str) {
        let mut guard = self.inner.lock().unwrap();
        let inner = &mut *guard;
        let mut freed = 0usize;
        let mut keep = |key: &SeriesKey| {
            let gone = key.kind == kind && key.entity == entity;
            if gone && counts_toward_cap(key) {
                freed += 1;
            }
            !gone
        };
        inner.counters.retain(|key, _| keep(key));
        inner.gauges.retain(|key, _| keep(key));
        inner.hists.retain(|key, _| keep(key));
        if let Some(used) = inner.per_kind.get_mut(&kind) {
            *used = used.saturating_sub(freed);
        }
    }
}

/// Whether a series takes a place under the cap. The overflow series does not: every folded key shares it, so
/// counting it would let the overflow itself fold.
fn counts_toward_cap(key: &SeriesKey) -> bool {
    !(key.entity == OTHER && key.labels.is_empty())
}

/// The handle for `key` in one of the registry's maps: the existing one, a new one while the kind is under the
/// cap, or else the overflow series of the metric (counted in `folded`).
fn series<T: Default + Clone>(
    map: &mut HashMap<SeriesKey, T>,
    per_kind: &mut HashMap<EntityKind, usize>,
    folded: &mut u64,
    cap: usize,
    key: SeriesKey,
) -> T {
    if let Some(existing) = map.get(&key) {
        return existing.clone();
    }
    if !counts_toward_cap(&key) {
        return map.entry(key).or_default().clone();
    }
    let used = per_kind.entry(key.kind).or_insert(0);
    if *used < cap {
        *used += 1;
        return map.entry(key).or_default().clone();
    }
    *folded += 1;
    let overflow = SeriesKey::new(&key.metric, key.kind, OTHER);
    map.entry(overflow).or_default().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_key(metric: &str, entity: &str) -> SeriesKey {
        SeriesKey::new(metric, EntityKind::Agent, entity)
    }

    #[test]
    fn a_counter_scrape_yields_the_increase_since_the_last_scrape() {
        let registry = Registry::new(10);
        let requests = registry.counter(agent_key("requests", "a"));
        requests.inc();
        requests.add(1.5);
        assert_eq!(requests.total(), 2.5);
        let first = registry.scrape(100, false);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].value, Value::Counter(2.5));
        assert_eq!(first[0].t_ms, 100);
        assert!(registry.scrape(200, false).is_empty());
        requests.inc();
        assert_eq!(registry.scrape(300, false)[0].value, Value::Counter(1.0));
    }

    #[test]
    fn past_the_cap_new_keys_fold_into_the_other_series_and_are_counted() {
        let registry = Registry::new(2);
        registry.counter(agent_key("requests", "a")).inc();
        registry.counter(agent_key("requests", "b")).inc();
        let c = registry.counter(agent_key("requests", "c"));
        let d = registry.counter(agent_key("requests", "d"));
        c.inc();
        d.inc();
        assert_eq!(registry.folded(), 2);
        assert_eq!(registry.len(), 3);
        let other: Vec<Sample> = registry
            .scrape(1, false)
            .into_iter()
            .filter(|s| s.key.entity == OTHER)
            .collect();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].value, Value::Counter(2.0));
    }

    #[test]
    fn a_gauge_is_scraped_only_when_set_unless_all_gauges() {
        let registry = Registry::new(10);
        let set = registry.gauge(agent_key("temperature", "a"));
        let _unset = registry.gauge(agent_key("temperature", "b"));
        set.set(41.5);
        let fresh = registry.scrape(1, false);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].key.entity, "a");
        assert_eq!(fresh[0].value, Value::Gauge(41.5));
        assert!(registry.scrape(2, false).is_empty());
        let all = registry.scrape(3, true);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].value, Value::Gauge(41.5));
        assert_eq!(all[1].key.entity, "b");
        assert_eq!(all[1].value, Value::Gauge(0.0));
    }

    #[test]
    fn forgetting_an_entity_frees_its_place_under_the_cap() {
        let registry = Registry::new(1);
        registry.counter(agent_key("requests", "a")).inc();
        registry.counter(agent_key("requests", "b")).inc();
        assert_eq!(registry.folded(), 1);
        registry.forget(EntityKind::Agent, "a");
        assert_eq!(registry.len(), 1);
        let c = registry.counter(agent_key("requests", "c"));
        c.inc();
        assert_eq!(registry.folded(), 1);
        let entities: Vec<String> = registry
            .scrape(1, false)
            .into_iter()
            .map(|s| s.key.entity)
            .collect();
        assert_eq!(entities, vec!["c".to_string(), OTHER.to_string()]);
    }
}
