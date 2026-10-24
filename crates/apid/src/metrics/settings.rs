//! The owner's metrics choices (`GET`/`PUT /v1/metrics/settings`, edited from the Metrics view's settings dialog):
//! collection level, retention, spend budgets, the forecast basis and the opt-in outside endpoints. Saved in the
//! metrics file as one row (`metric_settings`).

use std::sync::Mutex;

use apz_metrics::Level;
use serde::{Deserialize, Serialize};

use super::store::Store;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
pub struct Retention {
    /// 10-second points, hours.
    pub chunks_hours: u32,
    /// 1-minute aggregates, days.
    pub minute_days: u32,
    /// 1-hour aggregates, days.
    pub hour_days: u32,
    /// Model call rows, days.
    pub calls_days: u32,
    /// Kept traces, days, and their size cap in MB.
    pub traces_days: u32,
    pub traces_cap_mb: u32,
    /// Events, days.
    pub events_days: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            chunks_hours: 48,
            minute_days: 14,
            hour_days: 400,
            calls_days: 90,
            traces_days: 7,
            traces_cap_mb: 2048,
            events_days: 90,
        }
    }
}

/// A daily spend cap to report against (laws enforce caps; this only shows them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct BudgetCap {
    /// A scope id: `global`, `role:builder`, `agent:4417`.
    pub scope: String,
    pub cap_usd: f64,
}

/// The opt-in outside endpoints, both off by default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
pub struct Outside {
    /// Serve `/metrics` (OpenMetrics) to holders of `openmetrics_token`.
    pub openmetrics: bool,
    /// The token, made when OpenMetrics is switched on; never shown again after it is made.
    #[serde(skip_serializing)]
    pub openmetrics_token: String,
    /// Send kept traces to an OTLP/HTTP collector the owner names.
    pub otlp: bool,
    pub otlp_url: String,
    /// The owner confirmed the first send.
    pub otlp_confirmed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
#[table(name = "metric_settings")]
pub struct MetricsSettings {
    pub level: Level,
    #[table(flatten)]
    pub retention: Retention,
    #[table(child)]
    pub budgets: Vec<BudgetCap>,
    /// Monthly model-spend budget in USD.
    pub monthly_budget_usd: Option<f64>,
    /// Days the spend forecast is based on: 14 or 30.
    pub basis_days: u32,
    #[table(flatten)]
    pub outside: Outside,
}

impl Default for MetricsSettings {
    fn default() -> Self {
        MetricsSettings {
            level: Level::Standard,
            retention: Retention::default(),
            budgets: Vec::new(),
            monthly_budget_usd: None,
            basis_days: apz_metrics::forecast::DEFAULT_BASIS_DAYS,
            outside: Outside::default(),
        }
    }
}

impl MetricsSettings {
    /// Brings every field into its allowed range (retention at least an hour/day, basis 14 or 30, budgets
    /// positive), returning what it changed.
    pub fn repair(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let r = &mut self.retention;
        for (name, v, min, max) in [
            ("chunks_hours", &mut r.chunks_hours, 1, 24 * 14),
            ("minute_days", &mut r.minute_days, 1, 90),
            ("hour_days", &mut r.hour_days, 1, 3650),
            ("calls_days", &mut r.calls_days, 1, 3650),
            ("traces_days", &mut r.traces_days, 1, 90),
            ("traces_cap_mb", &mut r.traces_cap_mb, 16, 65_536),
            ("events_days", &mut r.events_days, 1, 3650),
        ] {
            let clamped = (*v).clamp(min, max);
            if clamped != *v {
                notes.push(format!("retention.{name} set to {clamped}"));
                *v = clamped;
            }
        }
        if self.basis_days != 14 && self.basis_days != 30 {
            notes.push("basis_days set to 14".into());
            self.basis_days = 14;
        }
        self.budgets
            .retain(|b| b.cap_usd.is_finite() && b.cap_usd > 0.0);
        if let Some(m) = self.monthly_budget_usd
            && !(m.is_finite() && m > 0.0)
        {
            self.monthly_budget_usd = None;
        }
        notes
    }
}

/// The settings in force, saved through the store.
pub struct Holder {
    current: Mutex<MetricsSettings>,
}

impl Holder {
    pub fn load(store: &dyn Store) -> Holder {
        let mut s = store.settings().unwrap_or_default();
        s.repair();
        Holder {
            current: Mutex::new(s),
        }
    }

    pub fn get(&self) -> MetricsSettings {
        super::lock(&self.current).clone()
    }

    /// Replaces the settings (repaired) and saves them. Keeps the OpenMetrics token unless OpenMetrics was
    /// switched off.
    pub fn put(&self, store: &dyn Store, mut new: MetricsSettings) -> MetricsSettings {
        new.repair();
        let mut cur = super::lock(&self.current);
        if new.outside.openmetrics {
            new.outside.openmetrics_token = if cur.outside.openmetrics_token.is_empty() {
                new_token()
            } else {
                cur.outside.openmetrics_token.clone()
            };
        } else {
            new.outside.openmetrics_token.clear();
        }
        *cur = new.clone();
        store.put_settings(&new);
        new
    }
}

/// A random token for the OpenMetrics endpoint (24 bytes from the OS, hex).
fn new_token() -> String {
    apz_trust::random::<24>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
