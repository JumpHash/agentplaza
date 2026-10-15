//! Window aggregates and the in-memory rollup. Every sample lands in three open windows (10 s, 1 min, 1 h) of its
//! series; a window closes when a sample past its end plus the late allowance arrives, or when `flush_due` is
//! called after that time. Each closed window is written once; nothing re-reads raw data to roll it up.

use crate::hist::HistSnapshot;
use crate::registry::{Sample, Value};
use crate::SeriesKey;
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
        let _ = value;
        todo!("Counter(d) -> count 1, sum d, min=max=last=d; Gauge(v) -> count 1, sum v, min=max=last=v; Hist(h) -> hist Some, count/sum/min/max from h, last = h.max")
    }

    /// Adds a later aggregate into this one (`last` follows the later one).
    pub fn merge(&mut self, later: &Agg) {
        let _ = later;
        todo!("empty self takes later; else count+=, sum+=, min/max, last=later.last, hists merge")
    }

    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
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
        let _ = (&mut self.open, &mut self.late, s);
        todo!()
    }

    /// Closes every window whose end plus [`LATE_MS`] is at or before `now_ms`.
    pub fn flush_due(&mut self, now_ms: u64) -> Vec<Closed> {
        let _ = now_ms;
        todo!()
    }

    /// Closes everything (shutdown).
    pub fn flush_all(&mut self) -> Vec<Closed> {
        todo!()
    }

    /// The open window of a series at a tier: its start and what it holds so far.
    pub fn current(&self, key: &SeriesKey, tier: Tier) -> Option<(u64, &Agg)> {
        self.open.get(&(key.clone(), tier)).map(|(t, a)| (*t, a))
    }

    /// Every open window at a tier (for overview tiles and alert checks, which read no table).
    pub fn open_at(&self, tier: Tier) -> impl Iterator<Item = (&SeriesKey, u64, &Agg)> {
        self.open.iter().filter(move |((_, t), _)| *t == tier).map(|((k, _), (s, a))| (k, *s, a))
    }

    /// Points that arrived past the late allowance.
    pub fn late(&self) -> u64 {
        self.late
    }
}
