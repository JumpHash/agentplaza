//! Window aggregates and the in-memory rollup. Every sample lands in three open windows (10 s, 1 min, 1 h) of its
//! series; a window closes when a sample past its end plus the late allowance arrives, or when `flush_due` is
//! called after that time. Each closed window is written once; nothing re-reads raw data to roll it up.

use crate::SeriesKey;
use crate::hist::HistSnapshot;
use crate::registry::{Sample, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A storage tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// 10-second points (kept 48 h).
    Raw10s,
    /// 1-minute aggregates (kept 14 d).
    Min1,
    /// 1-hour aggregates (kept 400 d).
    Hour1,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Raw10s, Tier::Min1, Tier::Hour1];

    pub fn step_ms(self) -> u64 {
        match self {
            Tier::Raw10s => 10_000,
            Tier::Min1 => 60_000,
            Tier::Hour1 => 3_600_000,
        }
    }

    /// The window start containing `t_ms`.
    pub fn floor(self, t_ms: u64) -> u64 {
        t_ms - t_ms % self.step_ms()
    }

    /// Words for the consoles: "10 s chunks", "1-minute aggregates", "1-hour aggregates".
    pub fn words(self) -> &'static str {
        match self {
            Tier::Raw10s => "10 s chunks",
            Tier::Min1 => "1-minute aggregates",
            Tier::Hour1 => "1-hour aggregates",
        }
    }
}

/// Points later than this past their window's end go to the next open window and are counted as late.
pub const LATE_MS: u64 = 120_000;

/// What a window holds. For a counter, `sum` is the increase over the window; for a gauge, min/max/sum/count of
/// the readings and `last`; for a histogram, `hist` plus count/sum/min/max mirroring it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Agg {
    pub count: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub last: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hist: Option<HistSnapshot>,
}

impl Agg {
    /// The aggregate of one scraped value.
    pub fn of(value: &Value) -> Agg {
        match value {
            Value::Counter(d) => point(*d),
            Value::Gauge(v) => point(*v),
            Value::Hist(h) => Agg {
                count: h.count,
                sum: h.sum,
                min: h.min,
                max: h.max,
                last: h.max,
                hist: Some(h.clone()),
            },
        }
    }

    /// Adds a later aggregate into this one (`last` follows the later one).
    pub fn merge(&mut self, later: &Agg) {
        if self.count == 0 {
            *self = later.clone();
            return;
        }
        self.count += later.count;
        self.sum += later.sum;
        self.min = self.min.min(later.min);
        self.max = self.max.max(later.max);
        self.last = later.last;
        if let Some(theirs) = &later.hist {
            match &mut self.hist {
                Some(mine) => mine.merge(theirs),
                None => self.hist = Some(theirs.clone()),
            }
        }
    }

    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

/// The aggregate of a single reading.
fn point(v: f64) -> Agg {
    Agg {
        count: 1,
        sum: v,
        min: v,
        max: v,
        last: v,
        hist: None,
    }
}

/// A window that closed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Closed {
    pub key: SeriesKey,
    pub tier: Tier,
    pub start_ms: u64,
    pub agg: Agg,
}

/// The open windows of every series.
#[derive(Default)]
pub struct Rollup {
    open: HashMap<(SeriesKey, Tier), (u64, Agg)>,
    late: u64,
}

impl Rollup {
    pub fn new() -> Rollup {
        Rollup::default()
    }

    /// Adds a sample to its series' three open windows; returns the windows it closed.
    pub fn ingest(&mut self, s: &Sample) -> Vec<Closed> {
        let fresh = Agg::of(&s.value);
        let mut closed = Vec::new();
        let mut late = false;
        for tier in Tier::ALL {
            let start = tier.floor(s.t_ms);
            let slot = (s.key.clone(), tier);
            match self.open.get_mut(&slot) {
                None => {
                    self.open.insert(slot, (start, fresh.clone()));
                }
                Some((open_start, agg)) if start == *open_start => agg.merge(&fresh),
                Some((open_start, agg)) if start > *open_start => {
                    let done = std::mem::replace(agg, fresh.clone());
                    closed.push(Closed {
                        key: s.key.clone(),
                        tier,
                        start_ms: *open_start,
                        agg: done,
                    });
                    *open_start = start;
                }
                Some((open_start, agg)) => {
                    // An older point joins the open window. It counts as late when its own window ended more
                    // than LATE_MS before the open window started; `last` keeps the newest reading.
                    if start + tier.step_ms() + LATE_MS < *open_start {
                        late = true;
                    }
                    let newest_last = agg.last;
                    agg.merge(&fresh);
                    agg.last = newest_last;
                }
            }
        }
        if late {
            self.late += 1;
        }
        closed
    }

    /// Closes every window whose end plus [`LATE_MS`] is at or before `now_ms`.
    pub fn flush_due(&mut self, now_ms: u64) -> Vec<Closed> {
        let due: Vec<(SeriesKey, Tier)> = self
            .open
            .iter()
            .filter(|(slot, (start, _))| *start + slot.1.step_ms() + LATE_MS <= now_ms)
            .map(|(slot, _)| slot.clone())
            .collect();
        self.close_slots(due)
    }

    /// Closes everything (shutdown).
    pub fn flush_all(&mut self) -> Vec<Closed> {
        let all: Vec<(SeriesKey, Tier)> = self.open.keys().cloned().collect();
        self.close_slots(all)
    }

    /// Removes the named windows and returns them sorted by tier, series and start.
    fn close_slots(&mut self, slots: Vec<(SeriesKey, Tier)>) -> Vec<Closed> {
        let mut out: Vec<Closed> = slots
            .into_iter()
            .filter_map(|slot| {
                let (start_ms, agg) = self.open.remove(&slot)?;
                Some(Closed {
                    key: slot.0,
                    tier: slot.1,
                    start_ms,
                    agg,
                })
            })
            .collect();
        out.sort_by(|a, b| (a.tier, &a.key, a.start_ms).cmp(&(b.tier, &b.key, b.start_ms)));
        out
    }

    /// The open window of a series at a tier: its start and what it holds so far.
    pub fn current(&self, key: &SeriesKey, tier: Tier) -> Option<(u64, &Agg)> {
        self.open.get(&(key.clone(), tier)).map(|(t, a)| (*t, a))
    }

    /// Every open window at a tier (for overview tiles and alert checks, which read no table).
    pub fn open_at(&self, tier: Tier) -> impl Iterator<Item = (&SeriesKey, u64, &Agg)> {
        self.open
            .iter()
            .filter(move |((_, t), _)| *t == tier)
            .map(|((k, _), (s, a))| (k, *s, a))
    }

    /// Points that arrived past the late allowance.
    pub fn late(&self) -> u64 {
        self.late
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityKind;

    fn sample(metric: &str, t_ms: u64, value: Value) -> Sample {
        Sample {
            key: SeriesKey::new(metric, EntityKind::Node, "node-a"),
            t_ms,
            value,
        }
    }

    fn window<'a>(closed: &'a [Closed], metric: &str, tier: Tier) -> &'a Closed {
        closed
            .iter()
            .find(|c| c.key.metric == metric && c.tier == tier)
            .expect("a window of that series and tier closed")
    }

    #[test]
    fn three_samples_in_one_minute_close_their_ten_second_windows_and_the_minute_at_the_next_minute()
     {
        let mut rollup = Rollup::new();
        assert!(
            rollup
                .ingest(&sample("temp", 5_000, Value::Gauge(1.0)))
                .is_empty()
        );

        let first = rollup.ingest(&sample("temp", 15_000, Value::Gauge(2.0)));
        assert_eq!(first.len(), 1);
        assert_eq!(
            (first[0].tier, first[0].start_ms, first[0].agg.count),
            (Tier::Raw10s, 0, 1)
        );

        let second = rollup.ingest(&sample("temp", 25_000, Value::Gauge(3.0)));
        assert_eq!(second.len(), 1);
        assert_eq!((second[0].tier, second[0].start_ms), (Tier::Raw10s, 10_000));

        let next_minute = rollup.ingest(&sample("temp", 65_000, Value::Gauge(4.0)));
        assert_eq!(next_minute.len(), 2);
        let ten_seconds = window(&next_minute, "temp", Tier::Raw10s);
        assert_eq!(ten_seconds.start_ms, 20_000);
        let minute = window(&next_minute, "temp", Tier::Min1);
        assert_eq!(minute.start_ms, 0);
        assert_eq!(minute.agg.count, 3);
        assert_eq!(
            (
                minute.agg.sum,
                minute.agg.min,
                minute.agg.max,
                minute.agg.last
            ),
            (6.0, 1.0, 3.0, 3.0)
        );
    }

    #[test]
    fn counters_add_up_and_gauges_keep_their_last_reading() {
        let mut rollup = Rollup::new();
        rollup.ingest(&sample("requests", 1_000, Value::Counter(2.0)));
        rollup.ingest(&sample("requests", 2_000, Value::Counter(3.0)));
        rollup.ingest(&sample("queue", 1_000, Value::Gauge(7.0)));
        rollup.ingest(&sample("queue", 2_000, Value::Gauge(4.0)));

        let closed = rollup.flush_all();
        let requests = window(&closed, "requests", Tier::Min1);
        assert_eq!(
            (requests.agg.count, requests.agg.sum, requests.agg.last),
            (2, 5.0, 3.0)
        );
        let queue = window(&closed, "queue", Tier::Min1);
        assert_eq!(
            (queue.agg.sum, queue.agg.min, queue.agg.max, queue.agg.last),
            (11.0, 4.0, 7.0, 4.0)
        );
    }

    #[test]
    fn a_histogram_window_merges_the_snapshots_of_its_scrapes() {
        let mut early = HistSnapshot::default();
        early.record(1.0);
        early.record(2.0);
        let mut late = HistSnapshot::default();
        late.record(10.0);

        let mut rollup = Rollup::new();
        rollup.ingest(&sample("latency", 1_000, Value::Hist(early)));
        rollup.ingest(&sample("latency", 2_000, Value::Hist(late)));

        let closed = rollup.flush_all();
        let minute = window(&closed, "latency", Tier::Min1);
        assert_eq!(
            (
                minute.agg.count,
                minute.agg.sum,
                minute.agg.min,
                minute.agg.max
            ),
            (3, 13.0, 1.0, 10.0)
        );
        assert_eq!(minute.agg.hist.as_ref().map(|h| h.count), Some(3));
    }

    #[test]
    fn flush_due_closes_only_windows_past_their_end_plus_the_late_allowance() {
        let mut rollup = Rollup::new();
        rollup.ingest(&sample("temp", 5_000, Value::Gauge(1.0)));

        // The 10 s window ends at 10 000 and may take late points until 10 000 + LATE_MS = 130 000.
        assert!(rollup.flush_due(129_999).is_empty());
        let ten_seconds = rollup.flush_due(130_000);
        assert_eq!(ten_seconds.len(), 1);
        assert_eq!(ten_seconds[0].tier, Tier::Raw10s);

        // The minute window ends at 60 000 and closes at 180 000; it is still open before that.
        assert!(rollup.flush_due(179_999).is_empty());
        let minute = rollup.flush_due(180_000);
        assert_eq!(minute.len(), 1);
        assert_eq!(minute[0].tier, Tier::Min1);
    }
}
