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

/// Most windows kept per series: 6 h of 10 s windows, 24 h of 1-minute windows, 400 days of 1-hour windows.
const RAW10S_WINDOWS: usize = 2_160;
const MIN1_WINDOWS: usize = 1_440;
const HOUR1_WINDOWS: usize = 9_600;

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;

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

/// The most windows one series keeps in a tier. Older windows go first.
fn window_cap(tier: Tier) -> usize {
    match tier {
        Tier::Raw10s => RAW10S_WINDOWS,
        Tier::Min1 => MIN1_WINDOWS,
        Tier::Hour1 => HOUR1_WINDOWS,
    }
}

/// A saved-value key is a plain name: no path separators and no parent references.
fn valid_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('/') && !key.contains('\\') && !key.contains("..")
}

impl Store for MemStore {
    fn put(&self, closed: &[Closed]) {
        let mut inner = super::lock(&self.inner);
        for window in closed {
            let series = inner
                .tiers
                .entry((window.tier, window.key.metric.clone()))
                .or_default()
                .entry(window.key.clone())
                .or_default();
            match series.binary_search_by_key(&window.start_ms, |(start, _)| *start) {
                Ok(index) => series[index].1 = window.agg.clone(),
                Err(index) => series.insert(index, (window.start_ms, window.agg.clone())),
            }
            while series.len() > window_cap(window.tier) {
                series.pop_front();
            }
        }
    }

    fn rows(
        &self,
        tier: Tier,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&SeriesKey) -> bool,
    ) -> Vec<(SeriesKey, Vec<(u64, Agg)>)> {
        let inner = super::lock(&self.inner);
        let series_of_metric = match inner.tiers.get(&(tier, metric.to_string())) {
            Some(series) => series,
            None => return Vec::new(),
        };
        // The map is ordered by series key, so the rows come out sorted.
        series_of_metric
            .iter()
            .filter(|(key, _)| keep(*key))
            .filter_map(|(key, windows)| {
                let in_range: Vec<(u64, Agg)> = windows
                    .iter()
                    .filter(|(start, _)| *start >= from_ms && *start < to_ms)
                    .cloned()
                    .collect();
                (!in_range.is_empty()).then(|| (key.clone(), in_range))
            })
            .collect()
    }

    fn put_events(&self, events: &[Event]) {
        let mut inner = super::lock(&self.inner);
        for event in events {
            inner.events.push_back(event.clone());
        }
        while inner.events.len() > MAX_EVENTS {
            inner.events.pop_front();
        }
    }

    fn events(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Event) -> bool,
        limit: usize,
    ) -> Vec<Event> {
        let inner = super::lock(&self.inner);
        inner
            .events
            .iter()
            .rev()
            .filter(|&event| event.t_ms >= from_ms && event.t_ms < to_ms && keep(event))
            .take(limit)
            .cloned()
            .collect()
    }

    fn put_calls(&self, calls: &[CallRecord]) {
        let mut inner = super::lock(&self.inner);
        for call in calls {
            inner.calls.push_back(call.clone());
        }
        while inner.calls.len() > MAX_CALLS {
            inner.calls.pop_front();
        }
    }

    fn calls(
        &self,
        before_ms: u64,
        limit: usize,
        keep: &dyn Fn(&CallRecord) -> bool,
    ) -> Vec<CallRecord> {
        let inner = super::lock(&self.inner);
        inner
            .calls
            .iter()
            .rev()
            .filter(|&call| call.at_ms < before_ms && keep(call))
            .take(limit)
            .cloned()
            .collect()
    }

    fn put_trace(&self, trace: &Trace) {
        let mut inner = super::lock(&self.inner);
        inner.traces.push_back(trace.clone());
        while inner.traces.len() > MAX_TRACES {
            inner.traces.pop_front();
        }
    }

    fn traces(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Trace) -> bool,
        limit: usize,
    ) -> Vec<Trace> {
        let inner = super::lock(&self.inner);
        inner
            .traces
            .iter()
            .rev()
            .filter(|&trace| trace.start_ms >= from_ms && trace.start_ms < to_ms && keep(trace))
            .take(limit)
            .cloned()
            .collect()
    }

    fn trace(&self, id: &str) -> Option<Trace> {
        let inner = super::lock(&self.inner);
        inner
            .traces
            .iter()
            .rev()
            .find(|trace| trace.id == id)
            .cloned()
    }

    fn kv_get(&self, key: &str) -> Option<String> {
        if !valid_key(key) {
            return None;
        }
        let mut inner = super::lock(&self.inner);
        if let Some(json) = inner.kv.get(key) {
            return Some(json.clone());
        }
        let dir = self.dir.as_ref()?;
        let json = std::fs::read_to_string(dir.join(format!("{}.json", key))).ok()?;
        inner.kv.insert(key.to_string(), json.clone());
        Some(json)
    }

    fn kv_put(&self, key: &str, json: &str) {
        if !valid_key(key) {
            return;
        }
        let mut inner = super::lock(&self.inner);
        inner.kv.insert(key.to_string(), json.to_string());
        if let Some(dir) = &self.dir {
            // Written to a temporary file and renamed, so a crash never leaves half a value behind.
            let path = dir.join(format!("{}.json", key));
            let tmp = dir.join(format!("{}.json.tmp", key));
            let written = std::fs::create_dir_all(dir)
                .and_then(|()| std::fs::write(&tmp, json))
                .and_then(|()| std::fs::rename(&tmp, &path));
            if let Err(e) = written {
                eprintln!("metrics: saving {} failed ({e})", path.display());
            }
        }
    }

    fn prune(&self, now_ms: u64, retention: &Retention) {
        let mut inner = super::lock(&self.inner);
        let window_cutoff = |tier: Tier| -> u64 {
            let span_ms = match tier {
                Tier::Raw10s => u64::from(retention.chunks_hours) * HOUR_MS,
                Tier::Min1 => u64::from(retention.minute_days) * DAY_MS,
                Tier::Hour1 => u64::from(retention.hour_days) * DAY_MS,
            };
            now_ms.saturating_sub(span_ms)
        };
        for ((tier, _), series) in inner.tiers.iter_mut() {
            let cutoff = window_cutoff(*tier);
            for windows in series.values_mut() {
                windows.retain(|(start, _)| *start >= cutoff);
            }
            series.retain(|_, windows| !windows.is_empty());
        }
        let events_cutoff = now_ms.saturating_sub(u64::from(retention.events_days) * DAY_MS);
        inner.events.retain(|event| event.t_ms >= events_cutoff);
        let calls_cutoff = now_ms.saturating_sub(u64::from(retention.calls_days) * DAY_MS);
        inner.calls.retain(|call| call.at_ms >= calls_cutoff);
        let traces_cutoff = now_ms.saturating_sub(u64::from(retention.traces_days) * DAY_MS);
        inner.traces.retain(|trace| trace.start_ms >= traces_cutoff);
    }

    fn sizes(&self) -> Vec<(String, u64)> {
        let inner = super::lock(&self.inner);
        let windows_in = |tier: Tier| -> u64 {
            inner
                .tiers
                .iter()
                .filter(|((t, _), _)| *t == tier)
                .flat_map(|(_, series)| series.values())
                .map(|windows| windows.len() as u64)
                .sum()
        };
        vec![
            ("memory · 10 s".to_string(), windows_in(Tier::Raw10s) * 24),
            ("memory · 1 min".to_string(), windows_in(Tier::Min1) * 64),
            ("memory · 1 h".to_string(), windows_in(Tier::Hour1) * 64),
            (
                "memory · events".to_string(),
                inner.events.len() as u64 * 128,
            ),
            ("memory · calls".to_string(), inner.calls.len() as u64 * 160),
            (
                "memory · traces".to_string(),
                inner.traces.len() as u64 * 1024,
            ),
        ]
    }

    fn flush(&self) {}
    fn kind(&self) -> &'static str {
        "memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_metrics::EntityKind;

    fn window(tier: Tier, entity: &str, start_ms: u64, sum: f64) -> Closed {
        Closed {
            key: SeriesKey::new("cpu", EntityKind::Vm, entity),
            tier,
            start_ms,
            agg: Agg {
                count: 1,
                sum,
                min: sum,
                max: sum,
                last: sum,
                hist: None,
            },
        }
    }

    fn event(t_ms: u64) -> Event {
        Event {
            t_ms,
            kind: "boot".to_string(),
            entity_kind: EntityKind::Vm,
            entity: "vm-1".to_string(),
            duration_ms: None,
            detail: String::new(),
            parts: Vec::new(),
        }
    }

    #[test]
    fn rows_return_the_windows_in_range_sorted_and_only_for_kept_series() {
        let store = MemStore::new(None);
        store.put(&[
            window(Tier::Raw10s, "vm-1", 30_000, 3.0),
            window(Tier::Raw10s, "vm-1", 10_000, 1.0),
            window(Tier::Raw10s, "vm-1", 20_000, 2.0),
            window(Tier::Raw10s, "vm-1", 40_000, 4.0),
            window(Tier::Raw10s, "vm-2", 20_000, 9.0),
            window(Tier::Raw10s, "vm-3", 50_000, 9.0),
            window(Tier::Min1, "vm-1", 20_000, 9.0),
        ]);
        // A window with an equal start replaces the one already kept.
        store.put(&[window(Tier::Raw10s, "vm-1", 20_000, 5.0)]);

        let rows = store.rows(Tier::Raw10s, "cpu", 10_000, 40_000, &|key| {
            key.entity == "vm-1"
        });
        assert_eq!(rows.len(), 1);
        let (key, windows) = &rows[0];
        assert_eq!(key.entity, "vm-1");
        let starts: Vec<u64> = windows.iter().map(|(start, _)| *start).collect();
        assert_eq!(starts, vec![10_000, 20_000, 30_000]);
        assert_eq!(windows[1].1.sum, 5.0);

        // Series with nothing in range are left out; vm-3 starts after the range.
        let all = store.rows(Tier::Raw10s, "cpu", 10_000, 40_000, &|_| true);
        let entities: Vec<&str> = all.iter().map(|(key, _)| key.entity.as_str()).collect();
        assert_eq!(entities, vec!["vm-1", "vm-2"]);
    }

    #[test]
    fn prune_drops_what_is_past_its_retention() {
        let now = 1_000 * DAY_MS;
        let store = MemStore::new(None);
        store.put(&[
            window(Tier::Min1, "vm-1", now - 20 * DAY_MS, 1.0),
            window(Tier::Min1, "vm-1", now - 2 * DAY_MS, 2.0),
            window(Tier::Hour1, "vm-1", now - 500 * DAY_MS, 3.0),
            window(Tier::Hour1, "vm-1", now - 10 * DAY_MS, 4.0),
        ]);
        store.put_events(&[event(now - 100 * DAY_MS), event(now - HOUR_MS)]);

        store.prune(now, &Retention::default());

        let minute: Vec<u64> = store
            .rows(Tier::Min1, "cpu", 0, now, &|_| true)
            .iter()
            .flat_map(|(_, windows)| windows.iter().map(|(start, _)| *start))
            .collect();
        assert_eq!(minute, vec![now - 2 * DAY_MS]);
        let hour: Vec<u64> = store
            .rows(Tier::Hour1, "cpu", 0, now, &|_| true)
            .iter()
            .flat_map(|(_, windows)| windows.iter().map(|(start, _)| *start))
            .collect();
        assert_eq!(hour, vec![now - 10 * DAY_MS]);
        assert_eq!(store.events(0, now + 1, &|_| true, 10).len(), 1);
    }

    #[test]
    fn a_saved_value_is_read_back_by_a_new_store_on_the_same_directory() {
        let dir = std::env::temp_dir().join(format!("apid-mem-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let json = r#"{"level":"basic"}"#;

        MemStore::new(Some(dir.clone())).kv_put("settings", json);
        let reopened = MemStore::new(Some(dir.clone()));
        assert_eq!(reopened.kv_get("settings").as_deref(), Some(json));
        assert_eq!(reopened.kv_get("../settings"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
