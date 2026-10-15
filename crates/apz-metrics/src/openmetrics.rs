//! The OpenMetrics text exposition for an outside scraper (the opt-in `/metrics` endpoint). Aggregates only; the
//! labels are the entity (`kind`, `entity`) and the metric's catalog labels — any other label on a key is dropped,
//! so no request text can leak through a label value. Label values are escaped (`\\`, `"`, newline).
//!
//! Names: the metric id with `.` replaced by `_`, prefixed `apz_`. Counters get `_total`; histograms are written
//! as summaries with `quantile="0.5"|"0.95"|"0.99"` plus `_sum` and `_count`; gauges as gauges. Ends with `# EOF`.

use crate::SeriesKey;
use crate::agg::Agg;

/// Renders the latest aggregate of each series (counters as their running totals).
pub fn render(series: &[(SeriesKey, Agg)]) -> String {
    let _ = series;
    todo!("group by metric; # TYPE and # HELP (catalog title) lines; unknown metrics are skipped")
}
