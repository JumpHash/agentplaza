//! Data out: the OpenMetrics text (latest aggregates, catalog labels only), CSV of a composed view, and the opt-in
//! OTLP/HTTP export of kept traces (JSON encoding, only after the owner confirmed the collector).

use apz_metrics::trace::Trace;
use apz_metrics_view::View;

use super::Metrics;

/// The OpenMetrics text of every series' latest 1-minute window.
pub fn openmetrics(m: &Metrics) -> String {
    let _ = m;
    todo!()
}

/// The chart series of a view as CSV: `panel,series,time,value` (time as RFC 3339 UTC), quoting as needed.
pub fn csv(view: &View) -> String {
    let _ = view;
    todo!()
}

/// Sends a kept trace to the owner's OTLP collector when the export is on and confirmed. Runs on its own thread;
/// failures are counted in `ext.errors` for the collector and never retried.
pub fn otlp(m: &Metrics, trace: &Trace) {
    let _ = (m, trace);
    todo!()
}
