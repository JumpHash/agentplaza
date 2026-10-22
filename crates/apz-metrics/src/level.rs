//! Collection levels (Settings → Metrics) and the budget governor that steps collection down when the metrics
//! system costs more than its budget, and back up when it has been under budget long enough.

use serde::{Deserialize, Serialize};

/// How much is collected. Ordered: `Off < Basic < Standard < Full`.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    apz_store::Column,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Only what billing needs (model calls); alert rules on spend still run.
    Off,
    /// Host and VM CPU and memory at 60 s, lifecycle events, latency histograms, hub timers, alerts, forecasts.
    Basic,
    /// Plus disk, IO, network and pressure at 10 s, context split, agent state, sampled traces. The default.
    #[default]
    Standard,
    /// Plus 2 s everywhere, every trace for 24 h, per-process guest stats; reverts to Standard after 24 h.
    Full,
}

impl Level {
    /// One level down (`Off` stays `Off`).
    pub fn down(self) -> Level {
        match self {
            Level::Full => Level::Standard,
            Level::Standard => Level::Basic,
            _ => Level::Off,
        }
    }

    /// One level up (`Full` stays `Full`).
    pub fn up(self) -> Level {
        match self {
            Level::Off => Level::Basic,
            Level::Basic => Level::Standard,
            _ => Level::Full,
        }
    }

    /// The sampling interval for host and VM gauges at this level, in ms. `live` (someone watches the scope) and
    /// `idle` (a VM idle over 5 min) adjust it: live is 2 s at any level above Off, idle is 60 s.
    pub fn sample_ms(self, live: bool, idle: bool) -> u64 {
        if self == Level::Off {
            return 0;
        }
        if live || self == Level::Full {
            return 2_000;
        }
        if idle || self == Level::Basic {
            return 60_000;
        }
        10_000
    }
}

/// How long a budget may be exceeded before collection steps down, and how long it must stay under before it
/// steps back up.
pub const STEP_DOWN_AFTER_MS: u64 = 5 * 60_000;
pub const STEP_UP_AFTER_MS: u64 = 30 * 60_000;
/// Full reverts to Standard after this long.
pub const FULL_LASTS_MS: u64 = 24 * 3_600_000;

/// Steps the effective level down after [`STEP_DOWN_AFTER_MS`] over budget and back up (never above the
/// owner's chosen level) after [`STEP_UP_AFTER_MS`] under it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Governor {
    /// The level the owner chose.
    pub chosen: Level,
    /// The level in force.
    pub effective: Level,
    /// When the current over- or under-budget stretch began.
    pub since_ms: u64,
    pub over: bool,
    /// When Full was chosen, so it can revert.
    pub full_since_ms: Option<u64>,
}

impl Governor {
    pub fn new(chosen: Level, now_ms: u64) -> Governor {
        Governor {
            chosen,
            effective: chosen,
            since_ms: now_ms,
            over: false,
            full_since_ms: (chosen == Level::Full).then_some(now_ms),
        }
    }

    /// Feeds one budget check. Returns the new effective level when it changed.
    pub fn observe(&mut self, over_budget: bool, now_ms: u64) -> Option<Level> {
        if let Some(t) = self.full_since_ms
            && now_ms.saturating_sub(t) >= FULL_LASTS_MS
        {
            self.full_since_ms = None;
            self.chosen = Level::Standard;
            if self.effective > Level::Standard {
                self.effective = Level::Standard;
                self.since_ms = now_ms;
                return Some(self.effective);
            }
        }
        if over_budget != self.over {
            self.over = over_budget;
            self.since_ms = now_ms;
            return None;
        }
        let held = now_ms.saturating_sub(self.since_ms);
        if self.over && held >= STEP_DOWN_AFTER_MS && self.effective > Level::Off {
            self.effective = self.effective.down();
            self.since_ms = now_ms;
            return Some(self.effective);
        }
        if !self.over && held >= STEP_UP_AFTER_MS && self.effective < self.chosen {
            self.effective = self.effective.up();
            self.since_ms = now_ms;
            return Some(self.effective);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_governor_steps_down_after_five_minutes_over_and_back_up_after_thirty_under() {
        let mut g = Governor::new(Level::Standard, 0);
        assert_eq!(g.observe(true, 1_000), None);
        assert_eq!(
            g.observe(true, 1_000 + STEP_DOWN_AFTER_MS),
            Some(Level::Basic)
        );
        assert_eq!(g.observe(false, 1_000 + STEP_DOWN_AFTER_MS + 1), None);
        let t = 1_000 + STEP_DOWN_AFTER_MS + 1 + STEP_UP_AFTER_MS;
        assert_eq!(g.observe(false, t), Some(Level::Standard));
        assert_eq!(
            g.observe(false, t + STEP_UP_AFTER_MS),
            None,
            "never above the chosen level"
        );
    }

    #[test]
    fn full_reverts_to_standard_after_a_day() {
        let mut g = Governor::new(Level::Full, 0);
        assert_eq!(g.observe(false, FULL_LASTS_MS), Some(Level::Standard));
        assert_eq!(g.chosen, Level::Standard);
    }
}
