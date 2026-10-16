//! The context tab: how full the agents' windows are, what fills them, what compaction frees, and which agents sit
//! nearest their limit. An agent scope reads that agent; any other scope reads the agent nearest its limit for the
//! composition chart and the window gauge reads the median of all of them.

use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, series_of, status_of};
use crate::format::format;
use crate::model::*;
use crate::source::{Entity, GroupBy, Read, Source};

const NO_READS: &str = "No context reads in this window.";
const NO_AGENT: &str = "No agent in this scope has context reads yet.";

/// An agent of the view, with what the context panels read of it.
struct AgentRow {
    /// The agent's scope id, `agent:<cid>`.
    scope: String,
    cid: u64,
    name: String,
    model: String,
    /// The model's context window in tokens, when known.
    window: Option<f64>,
    /// The latest call's input as a share of the window, in percent.
    fill: Option<f64>,
}

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let agents = agent_rows(ctx, src);
    let focus = focus_row(ctx, &agents);
    vec![
        window_panel(ctx, &agents, focus),
        fills_panel(ctx, src, focus),
        carried_panel(ctx, src),
        composition_panel(ctx, src, focus),
        largest_panel(ctx, src),
        nearest_panel(ctx, src, &agents),
        by_provider_panel(ctx, src),
    ]
}

/// The agents of the view: the scope itself for an agent scope, else the agents the scope holds.
fn agent_rows(ctx: &Ctx, src: &dyn Source) -> Vec<AgentRow> {
    let entities = if ctx.kind == "agent" {
        vec![Entity {
            id: ctx.q.scope.clone(),
            label: String::new(),
            state: String::new(),
        }]
    } else {
        src.entities(&ctx.q.scope, "agent")
    };
    entities
        .into_iter()
        .filter_map(|e| {
            let cid = cid_of(&e.id)?;
            let info = src.agent(cid).unwrap_or_default();
            let window = info
                .window
                .map(|w| w as f64)
                .or_else(|| src.scalar("ctx.window", &e.id, ctx.from_ms, ctx.to_ms, AggFn::Last));
            let name = if !info.name.is_empty() {
                info.name.clone()
            } else if !e.label.is_empty() {
                e.label.clone()
            } else {
                cid.to_string()
            };
            Some(AgentRow {
                fill: src.scalar("ctx.fill_pct", &e.id, ctx.from_ms, ctx.to_ms, AggFn::Last),
                scope: e.id,
                cid,
                name,
                model: info.model,
                window,
            })
        })
        .collect()
}

/// The agent the composition reads: the scope's own agent, else the one with the highest latest fill.
fn focus_row<'a>(ctx: &Ctx, agents: &'a [AgentRow]) -> Option<&'a AgentRow> {
    if ctx.kind == "agent" {
        return agents.first();
    }
    agents.iter().filter(|a| a.fill.is_some()).max_by(|a, b| {
        a.fill
            .unwrap_or(f64::MIN)
            .total_cmp(&b.fill.unwrap_or(f64::MIN))
    })
}

/// The agent id of an `agent:<cid>` scope.
fn cid_of(scope: &str) -> Option<u64> {
    scope.strip_prefix("agent:")?.parse().ok()
}

/// The window gauge: the agent's fill, or the median fill of the scope's agents with their p90 and the maximum.
fn window_panel(ctx: &Ctx, agents: &[AgentRow], focus: Option<&AgentRow>) -> Panel {
    if ctx.kind == "agent" {
        let body = match focus.and_then(|a| a.fill.map(|fill| (a, fill))) {
            None => empty(NO_READS),
            Some((a, fill)) => {
                let frac = fill / 100.0;
                let sub = match a.window {
                    Some(w) => format!(
                        "{} of {} tokens",
                        format(Unit::Tokens, frac * w),
                        format(Unit::Tokens, w)
                    ),
                    None => String::new(),
                };
                gauge(frac, format(Unit::Percent, fill), sub)
            }
        };
        return panel(
            "window",
            "Window fill",
            "input of the latest model call as a share of its window",
            4,
            body,
        );
    }
    let mut fills: Vec<(f64, &AgentRow)> = agents
        .iter()
        .filter_map(|a| a.fill.map(|f| (f, a)))
        .collect();
    fills.sort_by(|a, b| a.0.total_cmp(&b.0));
    let body = match fills.last() {
        None => empty(NO_READS),
        Some(&(max, max_row)) => {
            let values: Vec<f64> = fills.iter().map(|(f, _)| *f).collect();
            let median = percentile(&values, 0.5);
            let p90 = percentile(&values, 0.9);
            gauge(
                median / 100.0,
                format(Unit::Percent, median),
                format!(
                    "p90 {} · max {} ({})",
                    format(Unit::Percent, p90),
                    format(Unit::Percent, max),
                    max_row.name
                ),
            )
        }
    };
    panel(
        "window",
        "Median window fill",
        "latest call of each agent in scope",
        4,
        body,
    )
}

/// One gauge of a fraction, coloured at 75 % and 90 %.
fn gauge(frac: f64, label: String, sub: String) -> Body {
    let frac = frac.clamp(0.0, 1.0);
    Body::Gauges {
        gauges: vec![GaugeItem {
            frac,
            label,
            sub,
            status: status_of(frac, 0.75, 0.9),
        }],
    }
}

/// The nearest-rank percentile of sorted values, `q` in 0..=1. Panics on an empty slice.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// The read of the context parts of an agent over the window.
fn parts_read(ctx: &Ctx, agent_scope: &str) -> Read {
    Read {
        metric: "ctx.tokens".into(),
        scope: agent_scope.into(),
        from_ms: ctx.from_ms,
        to_ms: ctx.to_ms,
        step_ms: ctx.step_ms,
        agg: AggFn::Last,
        group: GroupBy::Label("part".into()),
        top: None,
    }
}

/// The name of a context part as people read it.
fn part_name(part: &str) -> String {
    match part {
        "system" => "system prompt",
        "tools" => "tool definitions",
        "history" => "history",
        "tool_results" => "tool results",
        "attachments" => "attachments",
        "summary" => "compaction summary",
        other => other,
    }
    .into()
}

/// The split of the focus agent's window into its parts, from the latest call.
fn fills_panel(ctx: &Ctx, src: &dyn Source, focus: Option<&AgentRow>) -> Panel {
    let body = match focus {
        None => empty(NO_AGENT),
        Some(a) => {
            let items: Vec<TreeItem> = src
                .series(&parts_read(ctx, &a.scope))
                .iter()
                .enumerate()
                .filter_map(|(i, g)| {
                    g.last().map(|value| TreeItem {
                        name: part_name(&g.name),
                        value,
                        tone: Tone::chart(i),
                    })
                })
                .collect();
            if items.is_empty() {
                empty("No context split for this agent yet.")
            } else {
                Body::Treemap {
                    unit: Unit::Tokens,
                    items,
                }
            }
        }
    };
    panel(
        "fills",
        "What fills the window",
        "latest call · estimated split, scaled to provider token counts",
        4,
        body,
    )
}

/// The share of spend and of context that only re-sends earlier turns, the compactions and the cache hit.
fn carried_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let scope = &ctx.q.scope;
    let sum = |metric: &str| src.scalar(metric, scope, ctx.from_ms, ctx.to_ms, AggFn::Sum);
    let tokens_in = sum("llm.tokens_in");
    let carried = sum("ctx.carried_tokens");
    let cache_read = sum("llm.tokens_cache_read");
    let compactions = sum("ctx.compactions");
    let freed = sum("ctx.freed_tokens");
    let share = match (carried, tokens_in) {
        (Some(c), Some(i)) => pct(c, i),
        _ => None,
    };
    let cache_hit = match (cache_read, tokens_in) {
        (Some(r), Some(i)) => pct(r, r + i),
        _ => None,
    };
    let value =
        |v: Option<f64>, unit: Unit| v.map(|v| format(unit, v)).unwrap_or_else(|| "—".into());
    let stats = vec![
        StatLine {
            label: "share of input spend".into(),
            value: value(share, Unit::Percent),
            note: "input that re-sends earlier turns".into(),
        },
        StatLine {
            label: "compactions in range".into(),
            value: value(compactions, Unit::Count),
            note: freed
                .map(|f| format!("freed {} tokens", format(Unit::Tokens, f)))
                .unwrap_or_default(),
        },
        StatLine {
            label: "cache hit on carried context".into(),
            value: value(cache_hit, Unit::Percent),
            note: String::new(),
        },
    ];
    panel(
        "carried",
        "Carried context",
        "tokens re-sent from earlier turns · compactions in this window",
        4,
        Body::Stats { stats },
    )
}

/// `part` as a percentage of `whole`, when there is a whole.
fn pct(part: f64, whole: f64) -> Option<f64> {
    (whole > 0.0).then(|| part / whole * 100.0)
}

/// The focus agent's context parts over time, stacked, with its compactions and its window as a limit.
fn composition_panel(ctx: &Ctx, src: &dyn Source, focus: Option<&AgentRow>) -> Panel {
    let Some(a) = focus else {
        return panel(
            "composition",
            "Context composition per model call",
            "",
            12,
            empty(NO_AGENT),
        );
    };
    let groups = src.series(&parts_read(ctx, &a.scope));
    let body = if groups.is_empty() {
        empty(NO_READS)
    } else {
        let series: Vec<Series> = groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let mut s = series_of(g, Tone::chart(i));
                s.name = part_name(&g.name);
                s
            })
            .collect();
        Body::Chart(Chart {
            unit: Unit::Tokens,
            x: ctx.x(),
            series,
            stacked: true,
            area: false,
            y_max: None,
            band: None,
            ghosts: Vec::new(),
            markers: compaction_markers(ctx, src, &a.scope),
            limit: a.window.map(|w| Limit {
                value: w,
                label: format!("window {}", format(Unit::Tokens, w)),
            }),
            height: 230,
            annotate: true,
            point_tab: None,
        })
    };
    panel(
        "composition",
        "Context composition per model call",
        &format!(
            "{} · stacked by part, with compactions · click a call for its trace",
            a.name
        ),
        12,
        body,
    )
}

/// One marker per compaction of the agent in the window, at its step.
fn compaction_markers(ctx: &Ctx, src: &dyn Source, agent_scope: &str) -> Vec<Marker> {
    let step = ctx.step_ms.max(1);
    src.events(agent_scope, ctx.from_ms, ctx.to_ms, &["compaction"], 100)
        .iter()
        .filter(|e| e.t_ms >= ctx.from_ms && e.t_ms < ctx.to_ms)
        .map(|e| ((e.t_ms - ctx.from_ms) / step) as usize)
        .filter(|index| *index < ctx.points)
        .map(|index| Marker {
            index,
            label: "compacted".into(),
        })
        .collect()
}

/// The largest tool results in the window's kept traces, by their tokens.
fn largest_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut found: Vec<(f64, Row)> = Vec::new();
    for trace in src.traces(&ctx.q.scope, ctx.from_ms, ctx.to_ms, 50) {
        for span in &trace.spans {
            let Some(tokens) =
                attr(&span.attrs, "result tokens").and_then(|v| v.parse::<f64>().ok())
            else {
                continue;
            };
            let tool = span
                .name
                .strip_prefix("tool · ")
                .unwrap_or(span.name.as_str())
                .to_string();
            let agent = src
                .agent(trace.cid)
                .map(|a| a.name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "agent".into());
            found.push((
                tokens,
                Row {
                    cells: vec![
                        Cell::Mono { text: tool },
                        Cell::Named {
                            name: agent,
                            id: trace.cid.to_string(),
                        },
                        Cell::Num {
                            text: format(Unit::Tokens, tokens),
                        },
                        number_cell(attr(&span.attrs, "re-sent"), Unit::Tokens),
                        number_cell(attr(&span.attrs, "carried cost"), Unit::Usd),
                    ],
                    scope: None,
                    tab: Some("traces".into()),
                    trace: Some(trace.id.clone()),
                    selected: false,
                },
            ));
        }
    }
    if found.is_empty() {
        return panel(
            "largest-results",
            "Largest tool results",
            "",
            6,
            empty("No large tool results in this window."),
        );
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    found.truncate(8);
    let table = Table {
        columns: vec![
            column("Tool", false),
            column("Agent", false),
            column("Tokens", true),
            column("Re-sent", true),
            column("Carried cost", true),
        ],
        rows: found.into_iter().map(|(_, row)| row).collect(),
        empty: String::new(),
        more: None,
    };
    panel(
        "largest-results",
        "Largest tool results",
        "tool results by size · click a row for its trace",
        6,
        Body::Table(table),
    )
}

/// The value of a span attribute.
fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A right-aligned number cell from an attribute, or a dash when it is missing or not a number.
fn number_cell(value: Option<&str>, unit: Unit) -> Cell {
    let text = value
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| format(unit, v))
        .unwrap_or_else(|| "—".into());
    Cell::Num { text }
}

fn column(head: &str, num: bool) -> Column {
    Column {
        head: head.into(),
        num,
    }
}

/// The fill of an agent as a status cell: warn from 75 %, crit from 90 %.
fn fill_cell(fill: Option<f64>) -> Cell {
    match fill {
        Some(f) => Cell::Status {
            text: format(Unit::Percent, f),
            status: status_of(f / 100.0, 0.75, 0.9),
        },
        None => Cell::Muted { text: "—".into() },
    }
}

/// The spark of a metric of one scope over the window, 40 points, gaps as 0.
fn spark_of(ctx: &Ctx, src: &dyn Source, scope: &str, metric: &str, agg: AggFn) -> Vec<f64> {
    let read = Read {
        metric: metric.into(),
        scope: scope.into(),
        from_ms: ctx.from_ms,
        to_ms: ctx.to_ms,
        step_ms: ((ctx.to_ms - ctx.from_ms) / 40).max(1_000),
        agg,
        group: GroupBy::All,
        top: None,
    };
    src.series(&read)
        .into_iter()
        .next()
        .map(|g| g.values.iter().map(|v| v.unwrap_or(0.0)).collect())
        .unwrap_or_default()
}

/// The agents nearest their window, fullest first.
fn nearest_panel(ctx: &Ctx, src: &dyn Source, agents: &[AgentRow]) -> Panel {
    if agents.is_empty() {
        return panel(
            "nearest",
            "Agents nearest their window",
            "",
            6,
            empty("No agents in this scope."),
        );
    }
    let hours = ctx.to_ms.saturating_sub(ctx.from_ms) as f64 / 3_600_000.0;
    let mut sorted: Vec<&AgentRow> = agents.iter().collect();
    sorted.sort_by(|a, b| {
        b.fill
            .unwrap_or(f64::MIN)
            .total_cmp(&a.fill.unwrap_or(f64::MIN))
    });
    let rows: Vec<Row> = sorted
        .iter()
        .map(|a| {
            let compactions = src.scalar(
                "ctx.compactions",
                &a.scope,
                ctx.from_ms,
                ctx.to_ms,
                AggFn::Sum,
            );
            let per_hour = compactions
                .filter(|_| hours > 0.0)
                .map(|c| c / hours)
                .map(|v| format(Unit::Count, v))
                .unwrap_or_else(|| "—".into());
            Row {
                cells: vec![
                    Cell::Named {
                        name: a.name.clone(),
                        id: a.cid.to_string(),
                    },
                    Cell::Text {
                        text: if a.model.is_empty() {
                            "—".into()
                        } else {
                            a.model.clone()
                        },
                    },
                    fill_cell(a.fill),
                    Cell::Num { text: per_hour },
                    Cell::Spark {
                        values: spark_of(ctx, src, &a.scope, "ctx.fill_pct", AggFn::Last),
                        tone: Tone::C1,
                    },
                ],
                scope: Some(a.scope.clone()),
                tab: None,
                trace: None,
                selected: false,
            }
        })
        .collect();
    let table = Table {
        columns: vec![
            column("Agent", false),
            column("Model", false),
            column("Fill", false),
            column("Compactions / h", true),
            column("Fill over time", false),
        ],
        rows,
        empty: String::new(),
        more: None,
    };
    panel(
        "nearest",
        "Agents nearest their window",
        "agents by fill, highest first · click an agent for its context",
        6,
        Body::Table(table),
    )
}

/// A stacked bar of the p95 fill, coloured by its status.
fn bar_cell(fill: Option<f64>) -> Cell {
    match fill {
        Some(f) => {
            let f = f.clamp(0.0, 100.0);
            let tone = match status_of(f / 100.0, 0.75, 0.9) {
                Status::Crit => Tone::Crit,
                Status::Warn => Tone::Warn,
                _ => Tone::C1,
            };
            Cell::Bar {
                parts: vec![
                    (f, tone, format(Unit::Percent, f)),
                    (100.0 - f, Tone::Rule, String::new()),
                ],
            }
        }
        None => Cell::Muted { text: "—".into() },
    }
}

/// The context size of each model: p50 and p95 window fill in tokens, the window, and the cache hit on its input.
fn by_provider_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut rows: Vec<(f64, Row)> = Vec::new();
    for model in src.entities(&ctx.q.scope, "model") {
        let read =
            |metric: &str, agg: AggFn| src.scalar(metric, &model.id, ctx.from_ms, ctx.to_ms, agg);
        let p50 = read("ctx.fill_pct", AggFn::P50);
        let p95 = read("ctx.fill_pct", AggFn::P95);
        let window = read("ctx.window", AggFn::Last);
        let cache_read = read("llm.tokens_cache_read", AggFn::Sum);
        let tokens_in = read("llm.tokens_in", AggFn::Sum);
        let cache_hit = match (cache_read, tokens_in) {
            (Some(r), Some(i)) => pct(r, r + i),
            _ => None,
        };
        let tokens = |fill: Option<f64>| match (fill, window) {
            (Some(f), Some(w)) => format(Unit::Tokens, f / 100.0 * w),
            _ => "—".into(),
        };
        rows.push((
            p95.unwrap_or(f64::MIN),
            Row {
                cells: vec![
                    Cell::Text {
                        text: model.label.clone(),
                    },
                    Cell::Num { text: tokens(p50) },
                    Cell::Num { text: tokens(p95) },
                    Cell::Num {
                        text: window
                            .map(|w| format(Unit::Tokens, w))
                            .unwrap_or_else(|| "—".into()),
                    },
                    bar_cell(p95),
                    Cell::Num {
                        text: cache_hit
                            .map(|v| format(Unit::Percent, v))
                            .unwrap_or_else(|| "—".into()),
                    },
                ],
                scope: Some(model.id.clone()),
                tab: None,
                trace: None,
                selected: false,
            },
        ));
    }
    if rows.is_empty() {
        return panel(
            "by-provider",
            "Context size by provider",
            "",
            12,
            empty("No model calls in this window."),
        );
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let table = Table {
        columns: vec![
            column("Provider / model", false),
            column("p50", true),
            column("p95", true),
            column("Window", true),
            column("p95 fill", false),
            column("Cache hit", true),
        ],
        rows: rows.into_iter().map(|(_, row)| row).collect(),
        empty: String::new(),
        more: None,
    };
    panel(
        "by-provider",
        "Context size by provider",
        "window fill across the agents on each model · cache hit on its input",
        12,
        Body::Table(table),
    )
}

#[cfg(test)]
mod tests {
    use crate::compose::Ctx;
    use crate::fixture::Fixture;
    use crate::model::{Body, Panel, ViewQuery};

    use super::panels;

    fn compose(scope: &str) -> Vec<Panel> {
        let src = Fixture::new(1_760_600_000_000);
        let q = ViewQuery::new("context", scope, "24h");
        let ctx = Ctx::new(&q, &src);
        panels(&ctx, &src)
    }

    /// The seven panels in order. Each body is its declared kind, or Empty when the source has no data.
    fn check_shape(panels: &[Panel], window_title: &str) {
        let ids: Vec<&str> = panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "window",
                "fills",
                "carried",
                "composition",
                "largest-results",
                "nearest",
                "by-provider"
            ]
        );
        assert_eq!(panels[0].title, window_title);
        assert!(matches!(
            panels[0].body,
            Body::Gauges { .. } | Body::Empty { .. }
        ));
        assert!(matches!(
            panels[1].body,
            Body::Treemap { .. } | Body::Empty { .. }
        ));
        assert!(matches!(panels[2].body, Body::Stats { .. }));
        assert!(matches!(
            panels[3].body,
            Body::Chart(_) | Body::Empty { .. }
        ));
        for panel in &panels[4..] {
            assert!(matches!(panel.body, Body::Table(_) | Body::Empty { .. }));
        }
    }

    #[test]
    fn the_global_scope_composes_its_context_panels_with_a_median_window() {
        let panels = compose("global");
        check_shape(&panels, "Median window fill");
    }

    #[test]
    fn an_agent_scope_composes_its_context_panels_with_its_own_window() {
        let panels = compose("agent:4417");
        check_shape(&panels, "Window fill");
    }
}
