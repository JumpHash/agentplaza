//! The latency tab: time to first token and output speed by provider, the duration heatmap, the tools, the error
//! rate by provider, and the turn duration by role.

use apz_metrics::HistSnapshot;
use apz_metrics::catalog::Unit;
use apz_metrics::hist::bucket_value;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, series_of};
use crate::format::format;
use crate::model::{Body, Cell, Chart, Column, Heatmap, Panel, Row, Series, Status, Table, Tone};
use crate::source::{Group, GroupBy, Read, Source};

/// Rows of the duration heatmap.
const HEAT_ROWS: usize = 24;
/// The lowest and the highest duration of the heatmap, in ms.
const HEAT_LO: f64 = 50.0;
const HEAT_HI: f64 = 60_000.0;
/// Most columns of the heatmap; more steps are merged so that each column covers several steps.
const HEAT_COLS: usize = 120;
/// A tool whose error rate is above this many percent is critical.
const TOOL_ERROR_CRIT: f64 = 2.0;

const NO_CALLS: &str = "No model calls in this window.";
const NO_TURNS: &str = "No agent turns in this window.";
const NO_TOOLS: &str = "No tool calls in this window.";

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    vec![
        ttft_by_provider(ctx, src),
        speed_by_provider(ctx, src),
        duration_heatmap(ctx, src),
        tools(ctx, src),
        errors_by_provider(ctx, src),
        turns_by_role(ctx, src),
    ]
}

/// The p95 and p50 time to the first token, per provider. The p95 is solid and the p50 dashed.
fn ttft_by_provider(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = ctx.read("llm.ttft_ms", AggFn::P95, GroupBy::Label("provider".into()));
    let mut chart = ctx.chart(src, &read, Unit::Ms, false, 220);
    let p50 = src.series(&Read {
        agg: AggFn::P50,
        ..read
    });
    let mut series = Vec::new();
    for (i, s) in chart.series.iter().enumerate() {
        series.push(Series {
            name: format!("{} p95", s.name),
            tone: Tone::chart(2 * i),
            ..s.clone()
        });
        if let Some(group) = p50.iter().find(|g| g.name == s.name) {
            let mut line = series_of(group, Tone::chart(2 * i + 1));
            line.name = format!("{} p50", group.name);
            line.dash = true;
            series.push(line);
        }
    }
    chart.series = series;
    let mut out = chart_panel(
        "ttft-by-provider",
        "Time to first token by provider",
        "p95 and p50 time to the first token, by provider",
        8,
        "llm.ttft_ms",
        chart,
        NO_CALLS,
    );
    out.live = true;
    out
}

/// The median output speed per provider.
fn speed_by_provider(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = ctx.read("llm.out_tps", AggFn::P50, GroupBy::Label("provider".into()));
    let chart = ctx.chart(src, &read, Unit::TokensPerSec, false, 180);
    chart_panel(
        "speed",
        "Output speed",
        "median tokens per second, by provider",
        4,
        "llm.out_tps",
        chart,
        NO_CALLS,
    )
}

/// How many calls took each duration, per step, on a log scale from `HEAT_LO` to `HEAT_HI`.
fn duration_heatmap(ctx: &Ctx, src: &dyn Source) -> Panel {
    let steps = src.hist_steps(
        "llm.duration_ms",
        &ctx.q.scope,
        ctx.from_ms,
        ctx.to_ms,
        ctx.step_ms,
    );
    let body = if steps.iter().all(HistSnapshot::is_empty) {
        empty(NO_CALLS)
    } else {
        // Merge consecutive steps so that there are at most `HEAT_COLS` columns.
        let per_column = steps.len().div_ceil(HEAT_COLS).max(1);
        let counts: Vec<Vec<f64>> = steps.chunks(per_column).map(heat_rows).collect();
        Body::Heatmap(Heatmap {
            start_ms: ctx.from_ms,
            step_ms: ctx.step_ms * per_column as u64,
            unit: Unit::Ms,
            lo: HEAT_LO,
            hi: HEAT_HI,
            counts,
            height: 260,
        })
    };
    metric_panel(
        "duration-heatmap",
        "Model call duration, distribution over time",
        "calls per duration band, step by step",
        12,
        "llm.duration_ms",
        body,
    )
}

/// The counts of one heatmap column: each bucket of each snapshot is added to the row that holds the bucket's
/// representative value.
fn heat_rows(snaps: &[HistSnapshot]) -> Vec<f64> {
    let mut rows = vec![0.0; HEAT_ROWS];
    for snap in snaps {
        for &(bucket, count) in &snap.buckets {
            rows[heat_row(bucket_value(bucket))] += count as f64;
        }
    }
    rows
}

/// The heatmap row of a duration in ms. Row `r` covers `[lo·(hi/lo)^(r/24), lo·(hi/lo)^((r+1)/24))`, so the rows
/// split the range into equal steps on a log scale; durations outside the range go to the first or last row.
fn heat_row(ms: f64) -> usize {
    let fraction = (ms / HEAT_LO).ln() / (HEAT_HI / HEAT_LO).ln();
    (fraction * HEAT_ROWS as f64)
        .floor()
        .clamp(0.0, (HEAT_ROWS - 1) as f64) as usize
}

/// The tools that made calls in the window, busiest first: calls, p50 and p95 duration, error rate and a trend.
fn tools(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut rows: Vec<(f64, Row)> = src
        .entities(&ctx.q.scope, "tool")
        .into_iter()
        .filter_map(|tool| {
            let calls = src.scalar("tool.calls", &tool.id, ctx.from_ms, ctx.to_ms, AggFn::Sum)?;
            if calls <= 0.0 {
                return None;
            }
            let errors = src
                .scalar("tool.errors", &tool.id, ctx.from_ms, ctx.to_ms, AggFn::Sum)
                .unwrap_or(0.0);
            let p50 = src.scalar(
                "tool.duration_ms",
                &tool.id,
                ctx.from_ms,
                ctx.to_ms,
                AggFn::P50,
            );
            let p95 = src.scalar(
                "tool.duration_ms",
                &tool.id,
                ctx.from_ms,
                ctx.to_ms,
                AggFn::P95,
            );
            let rate = errors / calls * 100.0;
            let row = Row {
                cells: vec![
                    Cell::Mono {
                        text: tool.label.clone(),
                    },
                    Cell::Num {
                        text: format(Unit::Count, calls),
                    },
                    Cell::Num {
                        text: or_dash(Unit::Ms, p50),
                    },
                    Cell::Num {
                        text: or_dash(Unit::Ms, p95),
                    },
                    Cell::Status {
                        text: format(Unit::Percent, rate),
                        status: if rate > TOOL_ERROR_CRIT {
                            Status::Crit
                        } else {
                            Status::Ok
                        },
                    },
                    Cell::Spark {
                        values: tool_spark(ctx, src, &tool.id, "tool.calls"),
                        tone: Tone::chart(0),
                    },
                ],
                scope: None,
                tab: None,
                trace: None,
                selected: false,
            };
            Some((calls, row))
        })
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let body = if rows.is_empty() {
        empty(NO_TOOLS)
    } else {
        Body::Table(Table {
            columns: vec![
                column("Tool", false),
                column("Calls", true),
                column("p50", true),
                column("p95", true),
                column("Errors", true),
                column("Trend", false),
            ],
            rows: rows.into_iter().map(|(_, row)| row).collect(),
            empty: NO_TOOLS.into(),
        })
    };
    metric_panel(
        "tools",
        "Tools and MCP",
        "calls, latency and error rate per tool",
        6,
        "tool.duration_ms",
        body,
    )
}

/// The error rate per provider: (errors + retries) over calls, in percent.
fn errors_by_provider(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = |metric: &str| ctx.read(metric, AggFn::Sum, GroupBy::Label("provider".into()));
    let series = error_series(
        src,
        &read("llm.calls"),
        &read("llm.errors"),
        &read("llm.retries"),
    );
    let mut ghosts = Vec::new();
    if ctx.comparing() {
        let cmp = |metric: &str| ctx.cmp_read(&read(metric));
        let before = error_series(
            src,
            &cmp("llm.calls"),
            &cmp("llm.errors"),
            &cmp("llm.retries"),
        );
        for s in before.into_iter().take(2) {
            ghosts.push(Series {
                name: format!("{} · {}", s.name, ctx.cmp_label),
                dash: true,
                ..s
            });
        }
    }
    let chart = Chart {
        unit: Unit::Percent,
        x: ctx.x(),
        series,
        stacked: false,
        area: false,
        y_max: None,
        band: None,
        ghosts,
        markers: Vec::new(),
        limit: None,
        height: 180,
        annotate: true,
        point_tab: None,
    };
    chart_panel(
        "errors",
        "Errors and retries by provider",
        "(errors + retries) / calls, by provider",
        6,
        "llm.errors",
        chart,
        NO_CALLS,
    )
}

/// The p95 turn duration per role.
fn turns_by_role(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = ctx.read(
        "turn.duration_ms",
        AggFn::P95,
        GroupBy::AgentProp("role".into()),
    );
    let chart = ctx.chart(src, &read, Unit::Ms, false, 220);
    chart_panel(
        "turns",
        "Turn duration",
        "p95 turn duration, by role",
        12,
        "turn.duration_ms",
        chart,
        NO_TURNS,
    )
}

/// The error rate of each group, per step: (errors + retries) over calls, in percent. A step with no calls, or a
/// gap in the calls, is a gap.
fn error_series(src: &dyn Source, calls: &Read, errors: &Read, retries: &Read) -> Vec<Series> {
    let call_groups = src.series(calls);
    let error_groups = src.series(errors);
    let retry_groups = src.series(retries);
    call_groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let values = group
                .values
                .iter()
                .enumerate()
                .map(|(step, total)| {
                    let total = (*total)?;
                    if total <= 0.0 {
                        return None;
                    }
                    let failed = value_at(&error_groups, &group.name, step)
                        + value_at(&retry_groups, &group.name, step);
                    Some(failed / total * 100.0)
                })
                .collect();
            Series {
                values,
                ..series_of(group, Tone::chart(i))
            }
        })
        .collect()
}

/// The value of a named group at a step: zero when the group is missing or has a gap there.
fn value_at(groups: &[Group], name: &str, step: usize) -> f64 {
    groups
        .iter()
        .find(|g| g.name == name)
        .and_then(|g| g.values.get(step).copied().flatten())
        .unwrap_or(0.0)
}

/// The trend of a tool's metric over the window: 40 points, gaps as 0.
fn tool_spark(ctx: &Ctx, src: &dyn Source, scope: &str, metric: &str) -> Vec<f64> {
    let step = ((ctx.to_ms - ctx.from_ms) / 40).max(1_000);
    let read = Read {
        metric: metric.into(),
        scope: scope.into(),
        from_ms: ctx.from_ms,
        to_ms: ctx.to_ms,
        step_ms: step,
        agg: AggFn::Sum,
        group: GroupBy::All,
        top: None,
    };
    src.series(&read)
        .into_iter()
        .next()
        .map(|g| g.values.iter().map(|v| v.unwrap_or(0.0)).collect())
        .unwrap_or_default()
}

/// A formatted value, or a dash when there is none.
fn or_dash(unit: Unit, value: Option<f64>) -> String {
    value.map(|v| format(unit, v)).unwrap_or_else(|| "—".into())
}

fn column(head: &str, num: bool) -> Column {
    Column {
        head: head.into(),
        num,
    }
}

/// A panel with its catalog metric set.
fn metric_panel(id: &str, title: &str, sub: &str, span: u8, metric: &str, body: Body) -> Panel {
    let mut out = panel(id, title, sub, span, body);
    out.metric = Some(metric.into());
    out
}

/// A panel of a chart, or the empty text when the chart has no series.
fn chart_panel(
    id: &str,
    title: &str,
    sub: &str,
    span: u8,
    metric: &str,
    chart: Chart,
    no_data: &str,
) -> Panel {
    let body = if chart.series.is_empty() {
        empty(no_data)
    } else {
        Body::Chart(chart)
    };
    metric_panel(id, title, sub, span, metric, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::ViewQuery;

    /// The name of a body's variant.
    fn variant(body: &Body) -> &'static str {
        match body {
            Body::Chart(_) => "chart",
            Body::Heatmap(_) => "heatmap",
            Body::Table(_) => "table",
            Body::Empty { .. } => "empty",
            _ => "other",
        }
    }

    #[test]
    fn the_latency_tab_composes_its_six_panels_in_order() {
        let fixture = Fixture::new(1_760_600_000_000);
        let view = compose(&ViewQuery::new("latency", "global", "24h"), &fixture);
        assert_eq!(view.tab, "latency");
        let expected = [
            ("ttft-by-provider", "chart"),
            ("speed", "chart"),
            ("duration-heatmap", "heatmap"),
            ("tools", "table"),
            ("errors", "chart"),
            ("turns", "chart"),
        ];
        let got: Vec<(&str, &str)> = view
            .panels
            .iter()
            .map(|p| (p.id.as_str(), variant(&p.body)))
            .collect();
        assert_eq!(got.len(), expected.len());
        for ((id, body), (want_id, want)) in got.iter().zip(expected) {
            assert_eq!(*id, want_id);
            // With no data in the fixture a panel is Empty, and never another body.
            assert!(
                *body == want || *body == "empty",
                "{id} is {body}, want {want} or empty"
            );
        }
    }

    #[test]
    fn a_snapshot_of_100_ms_and_10_s_lands_in_two_rows_with_counts_kept() {
        let mut snap = HistSnapshot::default();
        snap.record(100.0);
        snap.record(10_000.0);
        let rows = heat_rows(&[snap]);
        assert_eq!(rows.len(), HEAT_ROWS);
        assert_ne!(heat_row(100.0), heat_row(10_000.0));
        assert_eq!(rows[heat_row(100.0)], 1.0);
        assert_eq!(rows[heat_row(10_000.0)], 1.0);
        assert_eq!(rows.iter().sum::<f64>(), 2.0);
    }
}
