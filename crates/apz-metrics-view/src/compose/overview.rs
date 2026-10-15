//! The overview tab: the plaza at a glance. A health strip, the key numbers, spend by group, the changes that
//! matter, the first-token and CPU charts, and, on the global scope, what the metrics cost and the top agents.

use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, status_of};
use crate::compare::{Compare, change_pct};
use crate::format::format;
use crate::model::*;
use crate::source::{Group, GroupBy, Source};
use crate::tabs::applies;

/// Rises smaller than this many percent are not flagged.
const FLAG_FROM_PCT: f64 = 10.0;

/// A group read over the window: its mean level, and how far it moved against the comparison window.
struct Rise {
    name: String,
    scope: Option<String>,
    now: f64,
    change: Option<f64>,
}

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let mut panels = vec![
        health(ctx, src),
        tiles(ctx, src),
        spend_by_group(ctx, src),
        flagged(ctx, src),
        ttft(ctx, src),
        cpu(ctx, src),
    ];
    if ctx.kind == "global" {
        panels.push(cost(src));
        panels.push(top_agents(ctx, src));
    }
    panels
}

/// The health strip: one item per area whose tab applies to the scope, each with a status from its data.
fn health(ctx: &Ctx, src: &dyn Source) -> Panel {
    let items: Vec<HealthItem> = [
        providers(ctx, src),
        nodes(ctx, src),
        vms(ctx, src),
        agents(ctx, src),
        spend(ctx, src),
        hub(ctx, src),
    ]
    .into_iter()
    .filter(|item| applies(&item.tab, &ctx.kind, &ctx.scope.id))
    .collect();
    panel(
        "health",
        "Health",
        &format!("{} · worst status per area", ctx.scope.label),
        12,
        Body::Health { items },
    )
}

/// The model with the largest rise in p95 first-token time, or the slowest one when there is no comparison.
fn providers(ctx: &Ctx, src: &dyn Source) -> HealthItem {
    let models = rises_of(
        ctx,
        src,
        &ctx.read("llm.ttft_ms", AggFn::P95, GroupBy::Label("model".into())),
    );
    let worst = models
        .iter()
        .filter_map(|r| r.change.map(|c| (r, c)))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((r, change)) = worst {
        let status = status_of(change / 100.0, 0.2, 0.5);
        let word = if status == Status::Ok {
            "steady"
        } else {
            "slow"
        };
        let text = format!("{} {word} · p95 first token {}", r.name, signed_pct(change));
        return health_item("Providers", status, text, "latency");
    }
    match models.iter().max_by(|a, b| a.now.total_cmp(&b.now)) {
        Some(r) => health_item(
            "Providers",
            Status::Ok,
            format!("{} · p95 first token {}", r.name, format(Unit::Ms, r.now)),
            "latency",
        ),
        None => health_item(
            "Providers",
            Status::Mute,
            "No model calls in this window.".into(),
            "latency",
        ),
    }
}

/// The volume with the fullest disk now.
fn nodes(ctx: &Ctx, src: &dyn Source) -> HealthItem {
    let worst = src
        .series(&ctx.read("volume.used_pct", AggFn::Last, GroupBy::Entity))
        .into_iter()
        .filter_map(|g| {
            let used = g.last()?;
            Some((g.name, used))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    match worst {
        Some((name, used)) => health_item(
            "Nodes",
            status_of(used / 100.0, 0.8, 0.9),
            format!("{name} volume {} full", format(Unit::Percent, used)),
            "capacity",
        ),
        None => health_item(
            "Nodes",
            Status::Mute,
            "No volumes reporting.".into(),
            "capacity",
        ),
    }
}

/// The count of VMs running and asleep.
fn vms(ctx: &Ctx, src: &dyn Source) -> HealthItem {
    let (running, asleep) = vm_counts(ctx, src);
    if running + asleep == 0 {
        health_item(
            "VMs",
            Status::Mute,
            "No VMs in this scope.".into(),
            "compute",
        )
    } else {
        health_item(
            "VMs",
            Status::Ok,
            format!("{running} running · {asleep} asleep"),
            "compute",
        )
    }
}

/// The agent whose context is fullest at its peak.
fn agents(ctx: &Ctx, src: &dyn Source) -> HealthItem {
    let fullest = src
        .series(&ctx.read("ctx.fill_pct", AggFn::Max, GroupBy::Entity))
        .into_iter()
        .filter_map(|g| {
            let fill = peak(&g)?;
            Some((g.name, fill))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    match fullest {
        Some((name, fill)) => health_item(
            "Agents",
            status_of(fill / 100.0, 0.75, 0.9),
            format!("{name} context at {}", format(Unit::Percent, fill)),
            "context",
        ),
        None => health_item(
            "Agents",
            Status::Mute,
            "No agent context reported.".into(),
            "context",
        ),
    }
}

/// The budget closest to its cap today, among the budgets of the scope (all of them on the global scope).
fn spend(ctx: &Ctx, src: &dyn Source) -> HealthItem {
    let worst = src
        .budgets()
        .into_iter()
        .filter(|b| b.cap_usd > 0.0)
        .filter(|b| ctx.kind == "global" || b.scope == ctx.scope.id)
        .max_by(|a, b| (a.today_usd / a.cap_usd).total_cmp(&(b.today_usd / b.cap_usd)));
    match worst {
        Some(b) => {
            let frac = b.today_usd / b.cap_usd;
            let text = format!(
                "{} at {} of {} today",
                b.label,
                format(Unit::Percent, frac * 100.0),
                format(Unit::Usd, b.cap_usd)
            );
            health_item("Spend", status_of(frac, 0.8, 1.0), text, "capacity")
        }
        None => health_item(
            "Spend",
            Status::Mute,
            "No budget set for this scope.".into(),
            "capacity",
        ),
    }
}

/// The p95 of the hub's request time over the window.
fn hub(ctx: &Ctx, src: &dyn Source) -> HealthItem {
    match src.scalar(
        "hub.http_ms",
        &ctx.q.scope,
        ctx.from_ms,
        ctx.to_ms,
        AggFn::P95,
    ) {
        Some(p95) => health_item(
            "Hub",
            status_of(p95, 300.0, 1000.0),
            format!("p95 response {}", format(Unit::Ms, p95)),
            "hub",
        ),
        None => health_item(
            "Hub",
            Status::Mute,
            "No requests in this window.".into(),
            "hub",
        ),
    }
}

/// The key numbers as stat tiles. The fleet CPU tile carries the VM counts as its note and no change.
fn tiles(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut fleet_cpu = ctx.tile(
        src,
        "Fleet CPU",
        "host.cpu_pct",
        AggFn::Avg,
        Unit::Percent,
        true,
        Tone::chart(5),
        "compute",
    );
    let (running, asleep) = vm_counts(ctx, src);
    fleet_cpu.note = format!("{running} VMs running · {asleep} asleep");
    fleet_cpu.change_pct = None;
    let tiles = vec![
        ctx.tile(
            src,
            "Spend",
            "llm.spend_usd",
            AggFn::Sum,
            Unit::Usd,
            true,
            Tone::chart(0),
            "tokens",
        ),
        ctx.tile(
            src,
            "Tokens in",
            "llm.tokens_in",
            AggFn::Sum,
            Unit::Tokens,
            false,
            Tone::chart(1),
            "tokens",
        ),
        ctx.tile(
            src,
            "First token, p95",
            "llm.ttft_ms",
            AggFn::P95,
            Unit::Ms,
            true,
            Tone::chart(2),
            "latency",
        ),
        ctx.tile(
            src,
            "Context fill",
            "ctx.fill_pct",
            AggFn::Avg,
            Unit::Percent,
            true,
            Tone::chart(3),
            "context",
        ),
        ctx.tile(
            src,
            "Wake, p95",
            "life.wake_ms",
            AggFn::P95,
            Unit::Ms,
            true,
            Tone::chart(4),
            "lifecycle",
        ),
        fleet_cpu,
    ];
    let body = if tiles.iter().all(|t| t.value == "—") {
        empty("No data in this window.")
    } else {
        Body::Tiles { tiles }
    };
    panel("tiles", "Key numbers", &ctx.scope.label, 12, body)
}

/// Spend stacked by the view's group choice.
fn spend_by_group(ctx: &Ctx, src: &dyn Source) -> Panel {
    let choice = if ctx.q.group.is_empty() {
        "provider"
    } else {
        ctx.q.group.as_str()
    };
    let chart = ctx.chart(
        src,
        &ctx.read("llm.spend_usd", AggFn::Sum, ctx.group.clone()),
        Unit::Usd,
        true,
        210,
    );
    let total: f64 = chart
        .series
        .iter()
        .flat_map(|s| s.values.iter().flatten())
        .sum();
    let body = if chart.series.is_empty() {
        empty("No model spend in this window.")
    } else {
        Body::Chart(chart)
    };
    let mut out = panel(
        "spend-by-group",
        &format!("Spend by {choice}"),
        &format!("{} · stacked", ctx.scope.label),
        8,
        body,
    );
    out.right = format!("{} total", format(Unit::Usd, total));
    out
}

/// The changes that matter, at most five: agents whose context is nearly full first, then the largest rises
/// against the comparison window.
fn flagged(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut flags: Vec<Flag> = Vec::new();
    for g in src.series(&ctx.read("ctx.fill_pct", AggFn::Max, GroupBy::Entity)) {
        if let Some(fill) = peak(&g).filter(|v| *v > 90.0) {
            flags.push(Flag {
                status: status_of(fill / 100.0, 0.9, 0.95),
                text: format!("{} context at {}", g.name, format(Unit::Percent, fill)),
                when: ctx.time(ctx.to_ms),
                tab: Some("context".into()),
                scope: g.scope.clone(),
            });
        }
    }
    let ttft = rises_of(
        ctx,
        src,
        &ctx.read("llm.ttft_ms", AggFn::P95, GroupBy::Label("provider".into())),
    );
    let used = rises_of(
        ctx,
        src,
        &ctx.read("volume.used_pct", AggFn::Avg, GroupBy::Entity),
    );
    let wake = scalar_rise(ctx, src, "Plaza", "life.wake_ms", AggFn::P95);
    let retries = scalar_rise(ctx, src, "Model calls", "llm.retries", AggFn::Sum);
    let mut moved: Vec<(f64, Flag)> = Vec::new();
    moved.extend(
        ttft.iter()
            .filter_map(|r| change_flag(ctx, r, "first token p95", "latency")),
    );
    moved.extend(
        used.iter()
            .filter_map(|r| change_flag(ctx, r, "used", "storage")),
    );
    moved.extend(
        wake.iter()
            .filter_map(|r| change_flag(ctx, r, "wake p95", "lifecycle")),
    );
    moved.extend(
        retries
            .iter()
            .filter_map(|r| change_flag(ctx, r, "retries", "latency")),
    );
    moved.sort_by(|a, b| b.0.total_cmp(&a.0));
    flags.extend(moved.into_iter().map(|(_, flag)| flag));
    flags.truncate(5);

    let sub = match &ctx.compare {
        Compare::Week => "same time last week".to_string(),
        Compare::Scope(_) => format!("changes against {}", ctx.cmp_label),
        Compare::Prev => "changes against the previous period".to_string(),
        Compare::None => "no comparison set".to_string(),
    };
    let body = if flags.is_empty() {
        empty(if ctx.comparing() {
            "Nothing rose against the comparison window."
        } else {
            "Pick a comparison to see what changed."
        })
    } else {
        Body::Flags { flags }
    };
    panel("flagged", "Changes", &sub, 4, body)
}

/// The first-token chart: the p50 line over all providers, with the p10 to p95 band behind it.
fn ttft(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut chart = ctx.chart(
        src,
        &ctx.read("llm.ttft_ms", AggFn::P50, GroupBy::All),
        Unit::Ms,
        false,
        180,
    );
    let line = chart
        .series
        .first()
        .map(|s| s.values.clone())
        .unwrap_or_default();
    let low = src.series(&ctx.read("llm.ttft_ms", AggFn::P10, GroupBy::All));
    let high = src.series(&ctx.read("llm.ttft_ms", AggFn::P95, GroupBy::All));
    if let (Some(low), Some(high)) = (low.first(), high.first()) {
        // A step without a band reading falls back to the line, so the band has no holes.
        let pick = |band: &Group, i: usize| {
            band.values
                .get(i)
                .copied()
                .flatten()
                .or(line.get(i).copied().flatten())
                .unwrap_or(0.0)
        };
        chart.band = Some(Band {
            name: "p10–p95".into(),
            tone: Tone::C1,
            lo: (0..line.len()).map(|i| pick(low, i)).collect(),
            hi: (0..line.len()).map(|i| pick(high, i)).collect(),
        });
    }
    let body = if chart.series.is_empty() {
        empty("No model calls in this window.")
    } else {
        Body::Chart(chart)
    };
    let mut out = panel(
        "ttft",
        "Time to first token",
        "p50 line, p10–p95 band · all providers",
        6,
        body,
    );
    out.live = true;
    out
}

/// CPU per node. A node with no readings in the window is drawn as offline.
fn cpu(ctx: &Ctx, src: &dyn Source) -> Panel {
    let mut chart = ctx.chart(
        src,
        &ctx.read("host.cpu_pct", AggFn::Avg, GroupBy::Entity),
        Unit::Percent,
        false,
        180,
    );
    chart.y_max = Some(100.0);
    for series in &mut chart.series {
        if series.values.iter().all(|v| v.is_none()) {
            series.gap_reason = Some(format!("{} offline", series.name));
        }
    }
    let body = if chart.series.is_empty() {
        empty("No CPU readings in this window.")
    } else {
        Body::Chart(chart)
    };
    let mut out = panel(
        "cpu",
        "CPU by node",
        &format!("average per node · {}", ctx.scope.label),
        6,
        body,
    );
    out.live = true;
    out
}

/// What the metrics system itself costs, against its budget. Global scope only.
fn cost(src: &dyn Source) -> Panel {
    let (level, _) = src.level();
    let level = serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let rows: Vec<BarRow> = src
        .self_cost()
        .rows
        .into_iter()
        .map(|(label, value, budget, unit)| {
            let frac = if budget > 0.0 { value / budget } else { 0.0 };
            BarRow {
                label,
                value: format!("{value:.1} {unit}"),
                frac,
                status: status_of(frac, 0.8, 1.0),
                note: format!("of {budget:.1} {unit}"),
            }
        })
        .collect();
    let body = if rows.is_empty() {
        empty("No metrics cost recorded.")
    } else {
        Body::Bars {
            rows,
            note: "Steps down a level by itself after 5 min over budget. 2 s resolution only while a scope is \
                   open live; asleep VMs are not sampled."
                .into(),
        }
    };
    let mut out = panel(
        "cost",
        "Metrics cost",
        "what the metrics system itself costs, against its budget",
        6,
        body,
    );
    out.right = format!("level: {level}");
    out
}

/// The agents with the most spend in the window, each row opening the agent's scope. Global scope only.
fn top_agents(ctx: &Ctx, src: &dyn Source) -> Panel {
    let columns = vec![
        Column {
            head: "Agent".into(),
            num: false,
        },
        Column {
            head: "Role".into(),
            num: false,
        },
        Column {
            head: "Spend".into(),
            num: true,
        },
        Column {
            head: "Trend".into(),
            num: false,
        },
        Column {
            head: "Context".into(),
            num: true,
        },
    ];
    let mut rows = Vec::new();
    let groups = src.series(&ctx.read("llm.spend_usd", AggFn::Sum, GroupBy::Entity));
    for (i, group) in groups.iter().enumerate() {
        let Some(cid) = group
            .scope
            .as_deref()
            .and_then(|s| s.strip_prefix("agent:"))
            .and_then(|c| c.parse::<u64>().ok())
        else {
            continue;
        };
        let Some(agent) = src.agent(cid) else {
            continue;
        };
        let context = match src.scalar(
            "ctx.fill_pct",
            &format!("agent:{cid}"),
            ctx.from_ms,
            ctx.to_ms,
            AggFn::Last,
        ) {
            Some(fill) => Cell::Status {
                text: format(Unit::Percent, fill),
                status: status_of(fill / 100.0, 0.75, 0.9),
            },
            None => Cell::Muted { text: "—".into() },
        };
        rows.push(Row {
            cells: vec![
                Cell::Named {
                    name: agent.name.clone(),
                    id: cid.to_string(),
                },
                Cell::Text {
                    text: agent.role.clone(),
                },
                Cell::Num {
                    text: format(Unit::Usd, group.total()),
                },
                Cell::Spark {
                    values: group.values.iter().map(|v| v.unwrap_or(0.0)).collect(),
                    tone: Tone::chart(i),
                },
                context,
            ],
            scope: Some(format!("agent:{cid}")),
            tab: None,
            trace: None,
            selected: false,
        });
    }
    let text = "No agent spent in this window.";
    let body = if rows.is_empty() {
        empty(text)
    } else {
        Body::Table(Table {
            columns,
            rows,
            empty: text.into(),
        })
    };
    panel(
        "top-agents",
        "Top agents",
        "by spend in the window",
        6,
        body,
    )
}

/// The number of VMs running and asleep in the view's scope.
fn vm_counts(ctx: &Ctx, src: &dyn Source) -> (usize, usize) {
    let vms = src.entities(&ctx.q.scope, "vm");
    let running = vms.iter().filter(|v| v.state == "running").count();
    let asleep = vms.iter().filter(|v| v.state == "asleep").count();
    (running, asleep)
}

/// Each group of a read: its mean over the window, and its change against the same group of the comparison.
fn rises_of(ctx: &Ctx, src: &dyn Source, read: &crate::source::Read) -> Vec<Rise> {
    let before = if ctx.comparing() {
        src.series(&ctx.cmp_read(read))
    } else {
        Vec::new()
    };
    src.series(read)
        .iter()
        .filter_map(|g| {
            let now = mean(g)?;
            let change = before
                .iter()
                .find(|b| b.name == g.name)
                .and_then(mean)
                .and_then(|b| change_pct(now, b));
            Some(Rise {
                name: g.name.clone(),
                scope: g.scope.clone(),
                now,
                change,
            })
        })
        .collect()
}

/// A metric over the window against the comparison window, as one rise.
fn scalar_rise(ctx: &Ctx, src: &dyn Source, name: &str, metric: &str, agg: AggFn) -> Option<Rise> {
    let now = src.scalar(metric, &ctx.q.scope, ctx.from_ms, ctx.to_ms, agg)?;
    let change = if ctx.comparing() {
        src.scalar(metric, &ctx.cmp_scope, ctx.cmp_from_ms, ctx.cmp_to_ms, agg)
            .and_then(|before| change_pct(now, before))
    } else {
        None
    };
    Some(Rise {
        name: name.into(),
        scope: None,
        now,
        change,
    })
}

/// A flag for a rise of at least [`FLAG_FROM_PCT`], with the change to sort the flags by.
fn change_flag(ctx: &Ctx, r: &Rise, what: &str, tab: &str) -> Option<(f64, Flag)> {
    let change = r.change.filter(|c| *c >= FLAG_FROM_PCT)?;
    Some((
        change,
        Flag {
            status: status_of(change / 100.0, 0.2, 0.5),
            text: format!("{}: {what} {}", r.name, signed_pct(change)),
            when: ctx.time(ctx.to_ms),
            tab: Some(tab.into()),
            scope: r.scope.clone(),
        },
    ))
}

/// The mean of a group's readings over the window (gaps skipped).
fn mean(g: &Group) -> Option<f64> {
    let values: Vec<f64> = g.values.iter().flatten().copied().collect();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// The highest reading of a group over the window.
fn peak(g: &Group) -> Option<f64> {
    g.values.iter().flatten().copied().reduce(f64::max)
}

/// A change in percent with its sign: "+41%".
fn signed_pct(change: f64) -> String {
    format!("{change:+.0}%")
}

fn health_item(area: &str, status: Status, text: String, tab: &str) -> HealthItem {
    HealthItem {
        area: area.into(),
        status,
        text,
        tab: tab.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::{Body, View, ViewQuery};

    /// The panel ids of a view, in order.
    fn ids(view: &View) -> Vec<&str> {
        view.panels.iter().map(|p| p.id.as_str()).collect()
    }

    /// The variant of a body, by name.
    fn body_kind(body: &Body) -> &'static str {
        match body {
            Body::Health { .. } => "health",
            Body::Tiles { .. } => "tiles",
            Body::Chart(_) => "chart",
            Body::Table(_) => "table",
            Body::Flags { .. } => "flags",
            Body::Bars { .. } => "bars",
            Body::Empty { .. } => "empty",
            _ => "other",
        }
    }

    #[test]
    fn the_overview_returns_its_panels_in_order_whatever_the_data() {
        let fixture = Fixture::new(1_760_600_000_000);
        let view = compose(&ViewQuery::new("overview", "global", "24h"), &fixture);
        assert_eq!(
            ids(&view),
            [
                "health",
                "tiles",
                "spend-by-group",
                "flagged",
                "ttft",
                "cpu",
                "cost",
                "top-agents"
            ]
        );
        for panel in &view.panels {
            let allowed: &[&str] = match panel.id.as_str() {
                "health" => &["health"],
                "tiles" => &["tiles", "empty"],
                "flagged" => &["flags", "empty"],
                "cost" => &["bars", "empty"],
                "top-agents" => &["table", "empty"],
                _ => &["chart", "empty"],
            };
            let kind = body_kind(&panel.body);
            assert!(
                allowed.contains(&kind),
                "panel {} has body {kind}",
                panel.id
            );
        }
    }

    #[test]
    fn a_node_scope_has_no_global_only_panels() {
        let fixture = Fixture::new(1_760_600_000_000);
        let view = compose(&ViewQuery::new("overview", "node:n1", "24h"), &fixture);
        let names = ids(&view);
        assert!(names.contains(&"health"));
        assert!(!names.contains(&"cost"));
        assert!(!names.contains(&"top-agents"));
    }
}
