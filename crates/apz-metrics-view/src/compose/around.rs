//! "What changed around then": the drawer a click on a chart point opens.

use std::collections::BTreeMap;

use apz_metrics::query::AggFn;
use apz_metrics::trace::Keep;
use apz_metrics::{EntityKind, SeriesKey, movers};

use super::{Ctx, scope_info};
use crate::format::time_label;
use crate::model::{Around, EventRow, MoverRow, Status, TraceCount, ViewQuery};
use crate::source::{Group, GroupBy, Read, Source};

/// One minute in milliseconds.
const MINUTE_MS: u64 = 60_000;
/// Ten minutes: the half-width of the window around the moment.
const HALF_MS: u64 = 10 * MINUTE_MS;
/// The window around the moment and the window before it are both this wide.
const WINDOW_MS: u64 = 2 * HALF_MS;
/// Series whose values are below this on both sides are noise for the movers.
const MOVER_FLOOR: f64 = 0.01;
/// The sparkline of a mover: 24 points over the 40 minutes from 30 minutes before the moment to 10 after.
const SPARK_POINTS: u64 = 24;
const SPARK_SPAN_MS: u64 = 2 * HALF_MS + WINDOW_MS;
/// The zoom of the drawer's chart: 30 minutes either side of the moment.
const ZOOM_HALF_MS: u64 = 30 * MINUTE_MS;

/// The metrics the movers are read from: metric, aggregate, the tab a click opens, and what the row calls it.
const WATCH: &[(&str, AggFn, &str, &str)] = &[
    ("llm.ttft_ms", AggFn::P95, "latency", "first token p95"),
    ("llm.spend_usd", AggFn::Sum, "tokens", "spend rate"),
    ("host.cpu_pct", AggFn::Avg, "compute", "CPU"),
    ("life.wake_ms", AggFn::P95, "lifecycle", "wake p95"),
    ("work.queued", AggFn::Last, "agents", "queued tasks"),
    ("llm.errors", AggFn::Sum, "latency", "model errors"),
    ("ctx.fill_pct", AggFn::Max, "context", "context fill"),
];

/// What happened within ± 10 minutes of `t_ms` in the query's scope: events, the metrics that moved most against
/// the ten minutes before, kept traces, and the ± 30 minute zoom as fractions of the query's range.
pub fn around(q: &ViewQuery, t_ms: u64, src: &dyn Source) -> Around {
    let from = t_ms.saturating_sub(HALF_MS);
    let to = t_ms.saturating_add(HALF_MS);
    let before_from = t_ms.saturating_sub(HALF_MS + WINDOW_MS);
    let before_to = from;
    let spark_from = before_from;
    let spark_to = to;

    let events = src
        .events(&q.scope, from, to, &[], 20)
        .into_iter()
        .map(|e| {
            let status = event_status(&e.kind);
            EventRow {
                t_ms: e.t_ms,
                time: time_label(e.t_ms, MINUTE_MS, q.tz),
                kind: e.kind,
                status,
                subject: e.entity,
                scope: None,
                detail: e.detail,
            }
        })
        .collect();

    // Each watched metric, read per group in both windows; a group's scope is kept for the row.
    let mut before: Vec<(SeriesKey, f64)> = Vec::new();
    let mut after: Vec<(SeriesKey, f64)> = Vec::new();
    let mut owners: BTreeMap<SeriesKey, (usize, Option<String>)> = BTreeMap::new();
    for (i, &(metric, agg, _, _)) in WATCH.iter().enumerate() {
        for g in read_window(&q.scope, src, metric, agg, before_from, before_to) {
            if let Some(v) = g.last() {
                let key = SeriesKey::new(metric, EntityKind::Global, &g.name);
                owners.entry(key.clone()).or_insert((i, g.scope.clone()));
                before.push((key, v));
            }
        }
        for g in read_window(&q.scope, src, metric, agg, from, to) {
            if let Some(v) = g.last() {
                let key = SeriesKey::new(metric, EntityKind::Global, &g.name);
                owners.entry(key.clone()).or_insert((i, g.scope.clone()));
                after.push((key, v));
            }
        }
    }
    let movers = movers::top(&before, &after, 6, MOVER_FLOOR)
        .into_iter()
        .filter_map(|m| {
            let (i, scope) = owners.get(&m.key)?.clone();
            let (metric, agg, tab, label) = WATCH[i];
            let group = m.key.entity.clone();
            Some(MoverRow {
                name: format!("{label} · {group}"),
                change: change_text(m.before, m.after, matches!(agg, AggFn::Last)),
                status: if m.change.abs() >= 0.5 {
                    Status::Crit
                } else {
                    Status::Warn
                },
                spark: spark_of(&q.scope, src, metric, agg, &group, spark_from, spark_to),
                tab: tab.into(),
                scope,
            })
        })
        .collect();

    let kept = src.traces(&q.scope, from, to, 200);
    let traces = TraceCount {
        total: kept.len() as u32,
        errors: kept.iter().filter(|t| !t.ok).count() as u32,
        slow: kept.iter().filter(|t| t.kept == Some(Keep::Slow)).count() as u32,
    };

    // The zoom is measured against the query's own range, not a zoomed one, so it is a fraction of that range.
    let mut plain = q.clone();
    plain.zoom = None;
    let window = Ctx::new(&plain, src);
    let span = window.to_ms.saturating_sub(window.from_ms).max(1) as f64;
    let at = |ms: u64| ((ms as f64 - window.from_ms as f64) / span).clamp(0.0, 1.0);
    let zoom = (
        at(t_ms.saturating_sub(ZOOM_HALF_MS)),
        at(t_ms.saturating_add(ZOOM_HALF_MS)),
    );

    Around {
        label: time_label(t_ms, MINUTE_MS, q.tz),
        sub: format!(
            "± 10 minutes · {} · events, the metrics that moved most, and kept traces",
            scope_info(&q.scope, src).label
        ),
        events,
        movers,
        traces,
        zoom,
    }
}

/// One value per group of a metric over a window: the window is one step, so each group has one reading.
fn read_window(
    scope: &str,
    src: &dyn Source,
    metric: &str,
    agg: AggFn,
    from_ms: u64,
    to_ms: u64,
) -> Vec<Group> {
    src.series(&Read {
        metric: metric.into(),
        scope: scope.into(),
        from_ms,
        to_ms,
        step_ms: WINDOW_MS,
        agg,
        group: GroupBy::Entity,
        top: None,
    })
}

/// The sparkline of one group of a metric: 24 points over the window, gaps as 0.
fn spark_of(
    scope: &str,
    src: &dyn Source,
    metric: &str,
    agg: AggFn,
    group: &str,
    from_ms: u64,
    to_ms: u64,
) -> Vec<f64> {
    src.series(&Read {
        metric: metric.into(),
        scope: scope.into(),
        from_ms,
        to_ms,
        step_ms: SPARK_SPAN_MS / SPARK_POINTS,
        agg,
        group: GroupBy::Entity,
        top: None,
    })
    .into_iter()
    .find(|g| g.name == group)
    .map(|g| {
        g.values
            .iter()
            .take(SPARK_POINTS as usize)
            .map(|v| v.unwrap_or(0.0))
            .collect()
    })
    .unwrap_or_default()
}

/// The status of an event by its kind: errors and alerts are critical, compactions informational, scaling,
/// moves and laws warnings, and everything else muted.
fn event_status(kind: &str) -> Status {
    if kind.contains("error") || kind.contains("alert") {
        Status::Crit
    } else if kind.contains("compact") {
        Status::Info
    } else if kind.contains("scale") || kind.contains("move") || kind.contains("law") {
        Status::Warn
    } else {
        Status::Mute
    }
}

/// How a mover's change reads in the drawer: "3 → 9" for small counts, "2.1×" when the value at least doubled,
/// otherwise a signed percent such as "+63%" or "−18%".
fn change_text(before: f64, after: f64, count: bool) -> String {
    if count && before < 20.0 && after < 20.0 {
        return format!("{} → {}", before.round() as i64, after.round() as i64);
    }
    if before.abs() < 1e-9 {
        return "from 0".into();
    }
    let ratio = after / before;
    if ratio >= 2.0 {
        return format!("{ratio:.1}×");
    }
    let pct = ((after - before) / before.abs() * 100.0).round() as i64;
    if pct >= 0 {
        format!("+{pct}%")
    } else {
        format!("−{}%", -pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::Fixture;

    const NOW: u64 = 1_760_600_000_000;

    #[test]
    fn around_gives_a_label_and_a_zoom_inside_the_range() {
        let src = Fixture::new(NOW);
        let q = ViewQuery::new("overview", "global", "1h");
        let a = around(&q, NOW - 5 * MINUTE_MS, &src);
        assert!(!a.label.is_empty());
        assert!(0.0 <= a.zoom.0 && a.zoom.0 < a.zoom.1 && a.zoom.1 <= 1.0);
    }

    #[test]
    fn a_change_reads_as_a_percent_a_ratio_or_a_count_pair() {
        assert_eq!(change_text(100.0, 163.0, false), "+63%");
        assert_eq!(change_text(100.0, 82.0, false), "−18%");
        assert_eq!(change_text(10.0, 21.0, false), "2.1×");
        assert_eq!(change_text(3.0, 9.0, true), "3 → 9");
    }
}
