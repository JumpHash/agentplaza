//! The agents tab: the work agents did (tasks, success, turns, cost), where their time went by state, the queue
//! they drain, the approvals they waited on, and the laws they tripped.

use apz_metrics::Event;
use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, series_of};
use crate::format::format;
use crate::model::*;
use crate::source::{Entity, Group, GroupBy, Read, Source};

/// The metric of the time each agent spends in each state.
const AGENT_STATE: &str = "agent.state_ms";

/// The agent states in stacking order: key, name, tone.
const STATES: [(&str, &str, Tone); 6] = [
    ("streaming", "model streaming", Tone::C1),
    ("first_token", "waiting first token", Tone::C2),
    ("tools", "running tools", Tone::C3),
    ("approval", "waiting approval", Tone::C4),
    ("idle", "idle", Tone::Ink3),
    ("asleep", "asleep", Tone::Rule),
];

/// How many of the states, from the front, are busy time (streaming, first token, tools).
const BUSY: usize = 3;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let state = src.series(&state_read(ctx));
    vec![
        tiles_panel(ctx, src, &state),
        time_panel(ctx, &state),
        queue_panel(ctx, src),
        agents_panel(ctx, src),
        queue_wait_panel(ctx, src),
        approvals_panel(ctx, src),
        infractions_panel(ctx, src),
    ]
}

/// The read of the agent time by state over the view's window.
fn state_read(ctx: &Ctx) -> Read {
    ctx.read(AGENT_STATE, AggFn::Sum, GroupBy::Label("state".into()))
}

/// The time of each state over the window, in [`STATES`] order (states missing from the groups count as zero).
fn state_totals(groups: &[Group]) -> Vec<f64> {
    STATES
        .iter()
        .map(|(key, _, _)| {
            groups
                .iter()
                .filter(|g| g.name == *key)
                .map(Group::total)
                .sum::<f64>()
        })
        .collect()
}

/// The busy share of the agent time in the groups, in percent; `None` when no agent spent any time.
fn busy_share(groups: &[Group]) -> Option<f64> {
    let totals = state_totals(groups);
    let all: f64 = totals.iter().sum();
    (all > 0.0).then(|| totals[..BUSY].iter().sum::<f64>() / all * 100.0)
}

/// The share of each step's total that each state takes, in percent, for the states the groups hold, in
/// [`STATES`] order. Returns (state index, shares per step).
fn state_shares(groups: &[Group]) -> Vec<(usize, Vec<Option<f64>>)> {
    let present: Vec<(usize, &Group)> = STATES
        .iter()
        .enumerate()
        .filter_map(|(k, s)| groups.iter().find(|g| g.name == s.0).map(|g| (k, g)))
        .collect();
    let columns: Vec<Vec<Option<f64>>> = present.iter().map(|(_, g)| g.values.clone()).collect();
    present
        .iter()
        .zip(step_shares(&columns))
        .map(|((k, _), values)| (*k, values))
        .collect()
}

/// The busy share of each step, in percent (a step with no agent time counts as zero).
fn busy_steps(groups: &[Group]) -> Vec<f64> {
    let shares = state_shares(groups);
    let steps = shares.first().map_or(0, |(_, values)| values.len());
    (0..steps)
        .map(|i| {
            shares
                .iter()
                .filter(|(k, _)| *k < BUSY)
                .filter_map(|(_, values)| values.get(i).copied().flatten())
                .sum::<f64>()
        })
        .collect()
}

/// Turns per-state times into per-step shares: each state's value over its step's total, times 100. A step with
/// no time at all stays `None` for every state. Each inner vector is one state, one value per step.
fn step_shares(columns: &[Vec<Option<f64>>]) -> Vec<Vec<Option<f64>>> {
    let steps = columns.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![vec![None; steps]; columns.len()];
    for i in 0..steps {
        let total: f64 = columns.iter().filter_map(|c| value_at(c, i)).sum();
        if total <= 0.0 {
            continue;
        }
        for (k, column) in columns.iter().enumerate() {
            out[k][i] = value_at(column, i).map(|v| v / total * 100.0);
        }
    }
    out
}

/// The value of a column at a step, `None` for a gap or past its end.
fn value_at(column: &[Option<f64>], i: usize) -> Option<f64> {
    column.get(i).copied().flatten()
}

/// The success rate of a scope over a window: finished tasks over finished and failed ones.
fn success_rate(src: &dyn Source, scope: &str, from_ms: u64, to_ms: u64) -> Option<f64> {
    let done = src.scalar("work.done", scope, from_ms, to_ms, AggFn::Sum)?;
    let failed = src
        .scalar("work.failed", scope, from_ms, to_ms, AggFn::Sum)
        .unwrap_or(0.0);
    let all = done + failed;
    (all > 0.0).then(|| done / all)
}

/// The success rate per step of the sparkline (40 points, as the other tiles), in percent.
fn success_spark(ctx: &Ctx, src: &dyn Source) -> Vec<f64> {
    let done = ctx.spark(src, "work.done", AggFn::Sum);
    let failed = ctx.spark(src, "work.failed", AggFn::Sum);
    done.iter()
        .zip(failed.iter())
        .map(|(d, f)| {
            if d + f > 0.0 {
                d / (d + f) * 100.0
            } else {
                0.0
            }
        })
        .collect()
}

/// A value as a table or tile cell, "—" when there is none.
fn cell_text(value: Option<f64>, unit: Unit) -> String {
    value.map_or_else(|| "—".to_string(), |v| format(unit, v))
}

/// A table column.
fn column(head: &str, num: bool) -> Column {
    Column {
        head: head.into(),
        num,
    }
}

/// The agent id in an entity or scope id ("agent:4417", or just "4417").
fn agent_cid(id: &str) -> Option<u64> {
    id.rsplit(':').next()?.parse().ok()
}

/// A chart body, or an empty state when it has no series.
fn chart_or(chart: Chart, text: &str) -> Body {
    if chart.series.is_empty() {
        empty(text)
    } else {
        Body::Chart(chart)
    }
}

fn tiles_panel(ctx: &Ctx, src: &dyn Source, state: &[Group]) -> Panel {
    let tiles = vec![
        ctx.tile(
            src,
            "Tasks finished",
            "work.done",
            AggFn::Sum,
            Unit::Count,
            false,
            Tone::C1,
            "agents",
        ),
        success_tile(ctx, src),
        ctx.tile(
            src,
            "Turns per task",
            "work.turns",
            AggFn::P50,
            Unit::Count,
            true,
            Tone::C3,
            "agents",
        ),
        ctx.tile(
            src,
            "Approval wait p50",
            "approval.wait_ms",
            AggFn::P50,
            Unit::Ms,
            true,
            Tone::C4,
            "agents",
        ),
        law_tile(ctx, src),
        busy_tile(ctx, src, state),
    ];
    let body = if tiles.iter().all(|t| t.value == "—") {
        empty("No agent work in this window.")
    } else {
        Body::Tiles { tiles }
    };
    panel("tiles", "", "This window, against the comparison", 12, body)
}

fn success_tile(ctx: &Ctx, src: &dyn Source) -> Tile {
    let now = success_rate(src, &ctx.q.scope, ctx.from_ms, ctx.to_ms).map(|r| r * 100.0);
    let before = if ctx.comparing() {
        success_rate(src, &ctx.cmp_scope, ctx.cmp_from_ms, ctx.cmp_to_ms).map(|r| r * 100.0)
    } else {
        None
    };
    Tile {
        label: "Success rate".into(),
        value: cell_text(now, Unit::Percent),
        change_pct: match (now, before) {
            (Some(a), Some(b)) => crate::compare::change_pct(a, b),
            _ => None,
        },
        up_bad: false,
        note: String::new(),
        spark: success_spark(ctx, src),
        tone: Tone::C2,
        tab: Some("agents".into()),
    }
}

/// The law infractions tile, with a note of the infractions by law.
fn law_tile(ctx: &Ctx, src: &dyn Source) -> Tile {
    let mut tile = ctx.tile(
        src,
        "Law infractions",
        "law.infractions",
        AggFn::Sum,
        Unit::Count,
        true,
        Tone::C5,
        "agents",
    );
    let mut by_law: Vec<(String, f64)> = src
        .series(&ctx.read("law.infractions", AggFn::Sum, GroupBy::Label("law".into())))
        .iter()
        .map(|g| (g.name.clone(), g.total()))
        .filter(|(_, n)| *n > 0.0)
        .collect();
    by_law.sort_by(|a, b| b.1.total_cmp(&a.1));
    tile.note = by_law
        .iter()
        .map(|(law, n)| format!("{law} {n:.0}"))
        .collect::<Vec<_>>()
        .join(" · ");
    tile
}

fn busy_tile(ctx: &Ctx, src: &dyn Source, state: &[Group]) -> Tile {
    let now = busy_share(state);
    let before = if ctx.comparing() {
        busy_share(&src.series(&ctx.cmp_read(&state_read(ctx))))
    } else {
        None
    };
    Tile {
        label: "Busy share".into(),
        value: cell_text(now, Unit::Percent),
        change_pct: match (now, before) {
            (Some(a), Some(b)) => crate::compare::change_pct(a, b),
            _ => None,
        },
        up_bad: false,
        note: String::new(),
        spark: busy_steps(state),
        tone: Tone::C2,
        tab: Some("agents".into()),
    }
}

fn time_panel(ctx: &Ctx, state: &[Group]) -> Panel {
    let shares = state_shares(state);
    let body = if shares.is_empty() {
        empty("No agent time in this window.")
    } else {
        let series = shares
            .into_iter()
            .map(|(k, values)| Series {
                name: STATES[k].1.into(),
                tone: STATES[k].2,
                dash: false,
                values,
                gap_reason: None,
                scope: None,
            })
            .collect();
        Body::Chart(Chart {
            unit: Unit::Percent,
            x: ctx.x(),
            series,
            stacked: true,
            area: false,
            y_max: Some(100.0),
            band: None,
            ghosts: Vec::new(),
            markers: Vec::new(),
            limit: None,
            height: 220,
            annotate: true,
            point_tab: None,
        })
    };
    panel(
        "time",
        "Where agent time goes",
        "Share of each step's agent time, by state",
        8,
        body,
    )
}

fn queue_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let chart = ctx.chart(
        src,
        &ctx.read("work.queued", AggFn::Last, GroupBy::All),
        Unit::Count,
        false,
        150,
    );
    panel(
        "queue",
        "Work queue",
        "tasks waiting for an agent",
        4,
        chart_or(chart, "No tasks waited for an agent in this window."),
    )
}

fn queue_wait_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let chart = ctx.chart(
        src,
        &ctx.read("work.wait_ms", AggFn::P95, GroupBy::All),
        Unit::Ms,
        false,
        150,
    );
    panel(
        "queue-wait",
        "Task wait",
        "p95 time a task waited for an agent",
        4,
        chart_or(chart, "No task waited for an agent in this window."),
    )
}

/// Approvals waiting now and their p50 wait. The two series have different units (a count and milliseconds),
/// and a chart has one unit, so the axis reads as a count.
fn approvals_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let reads = [
        ("approval.pending", AggFn::Last, "Waiting now"),
        ("approval.wait_ms", AggFn::P50, "Wait p50"),
    ];
    let series: Vec<Series> = reads
        .iter()
        .enumerate()
        .filter_map(|(i, (metric, agg, name))| {
            src.series(&ctx.read(metric, *agg, GroupBy::All))
                .into_iter()
                .next()
                .map(|g| Series {
                    name: (*name).into(),
                    ..series_of(&g, Tone::chart(i))
                })
        })
        .collect();
    let body = if series.is_empty() {
        empty("No approvals in this window.")
    } else {
        Body::Chart(Chart {
            unit: Unit::Count,
            x: ctx.x(),
            series,
            stacked: false,
            area: false,
            y_max: None,
            band: None,
            ghosts: Vec::new(),
            markers: Vec::new(),
            limit: None,
            height: 150,
            annotate: true,
            point_tab: None,
        })
    };
    panel(
        "approvals",
        "Approvals",
        "waiting now, and how long they waited",
        4,
        body,
    )
}

fn infractions_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let text = "No law infractions in this window.";
    let events = src.events(&ctx.q.scope, ctx.from_ms, ctx.to_ms, &["law"], 50);
    let body = if events.is_empty() {
        empty(text)
    } else {
        Body::Table(Table {
            columns: vec![
                column("Time", false),
                column("Agent", false),
                column("Law", false),
                column("What happened", false),
            ],
            rows: events.iter().map(|e| infraction_row(ctx, src, e)).collect(),
            empty: text.into(),
            more: None,
        })
    };
    panel(
        "infractions",
        "Law infractions",
        "What the laws stopped, newest first",
        4,
        body,
    )
}

/// One infraction. The event carries no law name of its own, so the law is the part of the detail before its
/// first colon.
fn infraction_row(ctx: &Ctx, src: &dyn Source, e: &Event) -> Row {
    let cid = agent_cid(&e.entity);
    let agent = match cid.and_then(|c| src.agent(c)) {
        Some(info) => Cell::Named {
            name: info.name,
            id: info.cid.to_string(),
        },
        None => Cell::Mono {
            text: e.entity.clone(),
        },
    };
    let law = e
        .detail
        .split_once(':')
        .map_or(e.detail.as_str(), |(law, _)| law.trim());
    Row {
        cells: vec![
            Cell::Mono {
                text: ctx.time(e.t_ms),
            },
            agent,
            Cell::Text { text: law.into() },
            Cell::Text {
                text: e.detail.clone(),
            },
        ],
        scope: cid.map(|c| format!("agent:{c}")),
        tab: None,
        trace: None,
        selected: false,
    }
}

fn agents_panel(ctx: &Ctx, src: &dyn Source) -> Panel {
    let rows: Vec<Row> = src
        .entities(&ctx.q.scope, "agent")
        .iter()
        .filter_map(|e| agent_cid(&e.id).map(|cid| agent_row(ctx, src, e, cid)))
        .collect();
    let body = if rows.is_empty() {
        empty("No agents in this scope.")
    } else {
        Body::Table(Table {
            columns: vec![
                column("Agent", false),
                column("Role", false),
                column("Time", false),
                column("Tasks", true),
                column("Success", true),
                column("Turns / task", true),
                column("Cost / task", true),
                column("Tool errors", true),
                column("Approval wait", true),
                column("Infractions", true),
            ],
            rows,
            empty: String::new(),
            more: None,
        })
    };
    panel(
        "agents",
        "Agents",
        "Each agent's work in this window",
        12,
        body,
    )
}

/// One agent's row, read over the view's window from the agent's own scope.
fn agent_row(ctx: &Ctx, src: &dyn Source, entity: &Entity, cid: u64) -> Row {
    let scope = entity.id.as_str();
    let (from, to) = (ctx.from_ms, ctx.to_ms);
    let info = src.agent(cid);
    let name = info
        .as_ref()
        .map(|a| a.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| entity.label.clone());
    let role = info.map(|a| a.role).unwrap_or_default();
    let scalar = |metric: &str, agg: AggFn| src.scalar(metric, scope, from, to, agg);

    let time = src.series(&Read {
        metric: AGENT_STATE.into(),
        scope: scope.into(),
        from_ms: from,
        to_ms: to,
        step_ms: ctx.step_ms,
        agg: AggFn::Sum,
        group: GroupBy::Label("state".into()),
        top: None,
    });
    let totals = state_totals(&time);
    let all: f64 = totals.iter().sum();
    let parts = STATES
        .iter()
        .zip(totals)
        .map(|((_, label, tone), t)| {
            let share = if all > 0.0 { t / all * 100.0 } else { 0.0 };
            (share, *tone, (*label).to_string())
        })
        .collect();

    let infractions = scalar("law.infractions", AggFn::Sum);
    let infractions_cell = match infractions {
        Some(n) if n > 0.0 => Cell::Status {
            text: cell_text(infractions, Unit::Count),
            status: Status::Crit,
        },
        _ => Cell::Num {
            text: cell_text(infractions, Unit::Count),
        },
    };

    Row {
        cells: vec![
            Cell::Named {
                name,
                id: cid.to_string(),
            },
            Cell::Text { text: role },
            Cell::Bar { parts },
            Cell::Num {
                text: cell_text(scalar("work.done", AggFn::Sum), Unit::Count),
            },
            Cell::Num {
                text: cell_text(
                    success_rate(src, scope, from, to).map(|r| r * 100.0),
                    Unit::Percent,
                ),
            },
            Cell::Num {
                text: cell_text(scalar("work.turns", AggFn::P50), Unit::Count),
            },
            Cell::Num {
                text: cell_text(scalar("work.cost_usd", AggFn::P50), Unit::Usd),
            },
            Cell::Num {
                text: cell_text(scalar("tool.errors_by_agent", AggFn::Sum), Unit::Count),
            },
            Cell::Num {
                text: cell_text(scalar("approval.wait_ms", AggFn::P50), Unit::Ms),
            },
            infractions_cell,
        ],
        scope: Some(entity.id.clone()),
        tab: None,
        trace: None,
        selected: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::{Body, View, ViewQuery};

    fn body_of<'a>(view: &'a View, id: &str) -> &'a Body {
        &view
            .panels
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("no panel {id}"))
            .body
    }

    #[test]
    fn the_agents_tab_returns_every_panel_in_order_even_without_data() {
        let fixture = Fixture::new(1_760_600_000_000);
        let view = compose(&ViewQuery::new("agents", "global", "24h"), &fixture);
        let ids: Vec<&str> = view.panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "tiles",
                "time",
                "queue",
                "agents",
                "queue-wait",
                "approvals",
                "infractions"
            ]
        );
        assert!(matches!(
            body_of(&view, "tiles"),
            Body::Tiles { .. } | Body::Empty { .. }
        ));
        assert!(matches!(
            body_of(&view, "time"),
            Body::Chart(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body_of(&view, "queue"),
            Body::Chart(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body_of(&view, "agents"),
            Body::Table(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body_of(&view, "queue-wait"),
            Body::Chart(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body_of(&view, "approvals"),
            Body::Chart(_) | Body::Empty { .. }
        ));
        assert!(matches!(
            body_of(&view, "infractions"),
            Body::Table(_) | Body::Empty { .. }
        ));
    }

    #[test]
    fn the_time_shares_of_three_states_add_to_one_hundred_per_step() {
        let columns = vec![
            vec![Some(10.0), Some(0.0)],
            vec![Some(30.0), None],
            vec![Some(60.0), Some(0.0)],
        ];
        let shares = step_shares(&columns);
        let first: f64 = shares.iter().filter_map(|s| s[0]).sum();
        assert!((first - 100.0).abs() < 1e-9);
        assert_eq!(shares[0][0], Some(10.0));
        assert_eq!(shares[2][0], Some(60.0));
    }

    #[test]
    fn a_step_with_no_agent_time_stays_none_for_every_state() {
        let columns = vec![vec![Some(0.0), Some(5.0)], vec![Some(0.0), Some(5.0)]];
        let shares = step_shares(&columns);
        assert_eq!(shares[0][0], None);
        assert_eq!(shares[1][0], None);
        assert_eq!(shares[0][1], Some(50.0));
    }
}
