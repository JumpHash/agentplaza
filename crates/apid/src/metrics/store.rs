//! Where metrics are kept: the [`Store`] trait, over the metrics file (`metrics.db`, [`super::sqlite`]).

use apz_metrics::agg::{Agg, Closed, Tier};
use apz_metrics::trace::Trace;
use apz_metrics::{Event, SeriesKey};
use serde::{Deserialize, Serialize};

use super::alerting::Stored;
use super::settings::{MetricsSettings, Retention};

/// One model call as `model_calls` keeps it: the usage ledger's call plus its timings and context split.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CallRecord {
    pub at_ms: u64,
    pub cid: u64,
    pub role: String,
    pub run: String,
    pub node: String,
    pub provider: String,
    pub model: String,
    /// plaza harness, ACP engine, hub MCP.
    pub engine: String,
    pub tokens_in: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// Upstream HTTP status (200 when it succeeded and none was seen).
    pub status: u16,
    #[serde(default)]
    pub retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The estimated context split: (part, tokens).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<(String, u64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
}

/// What a store holds. Every method is cheap to call from the tick thread; writes may be buffered and land on
/// [`Store::flush`].
pub trait Store: Send + Sync {
    /// Keeps closed windows.
    fn put(&self, closed: &[Closed]);

    /// The stored windows in `[from, to)` of every series of `metric` that `keep` accepts, ascending by start.
    fn rows(
        &self,
        tier: Tier,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&SeriesKey) -> bool,
    ) -> Vec<(SeriesKey, Vec<(u64, Agg)>)>;

    fn put_events(&self, events: &[Event]);

    /// Events in `[from, to)` that `keep` accepts, newest first, at most `limit`.
    fn events(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Event) -> bool,
        limit: usize,
    ) -> Vec<Event>;

    fn put_calls(&self, calls: &[CallRecord]);

    /// Calls before `before_ms` that `keep` accepts, newest first, at most `limit`.
    fn calls(
        &self,
        before_ms: u64,
        limit: usize,
        keep: &dyn Fn(&CallRecord) -> bool,
    ) -> Vec<CallRecord>;

    fn put_trace(&self, trace: &Trace);

    /// Kept traces in `[from, to)` that `keep` accepts, newest first, at most `limit`.
    fn traces(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Trace) -> bool,
        limit: usize,
    ) -> Vec<Trace>;

    /// The newest kept trace with this id.
    fn trace(&self, id: &str) -> Option<Trace>;

    /// The owner's settings, saved as one row (`None` until they are first saved).
    fn settings(&self) -> Option<MetricsSettings>;

    fn put_settings(&self, settings: &MetricsSettings);

    /// The alert rules, states, mutes, acknowledgements and history, saved as one row.
    fn alerts(&self) -> Option<Stored>;

    fn put_alerts(&self, stored: &Stored);

    /// The owner's dashboards, saved as one row.
    fn dashboards(&self) -> Option<apz_metrics_view::model::Saved>;

    fn put_dashboards(&self, saved: &apz_metrics_view::model::Saved);

    /// Drops what is past retention, table by table, and caps the kept traces by size.
    fn prune(&self, now_ms: u64, retention: &Retention);

    /// Bytes per stream table (its children with it), for "Metrics cost" and the stores table.
    fn sizes(&self) -> Vec<(String, u64)>;

    /// Writes what is waiting.
    fn flush(&self);
}
