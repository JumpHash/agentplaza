//! The tokens tab: what the model calls cost and consume, where the tokens go, and what routing could save.

use apz_metrics::catalog::Unit;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, series_of};
use crate::compare::change_pct;
use crate::format::format;
use crate::model::*;
use crate::source::{CallRow, Group, GroupBy, Read, Source};

/// Output tokens per call above which a model counts as output-heavy, so a cheaper sibling is worth suggesting.
const HIGH_OUTPUT_PER_CALL: f64 = 800.0;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    vec![
        tiles(ctx, src),
        tokens_by_group(ctx, src),
        spend_share(ctx, src),
        cache(ctx, src),
        unit_cost(ctx, src),
        model_mix(ctx, src),
        savings(ctx, src),
        recent_calls(ctx, src),
        prices(ctx, src),
    ]
}

/// The headline numbers: spend, tokens, cache hit, cost per turn and calls.
fn tiles(ctx: &Ctx, src: &dyn Source) -> Panel {
    let tab = "tokens";
    let tiles = vec![
        ctx.tile(
            src,
            "Spend",
            "llm.spend_usd",
            AggFn::Sum,
            Unit::Usd,
            true,
            Tone::C1,
            tab,
        ),
        ctx.tile(
            src,
            "Input tokens",
            "llm.tokens_in",
            AggFn::Sum,
            Unit::Tokens,
            false,
            Tone::C2,
            tab,
        ),
        ctx.tile(
            src,
            "Output tokens",
            "llm.tokens_out",
            AggFn::Sum,
            Unit::Tokens,
            false,
            Tone::C3,
            tab,
        ),
        pair_tile(
            ctx,
            src,
            &Pair {
                label: "Cache hit",
                first: "llm.tokens_cache_read",
                second: "llm.tokens_in",
                combine: cache_hit,
                unit: Unit::Percent,
                up_bad: false,
                tone: Tone::C4,
            },
        ),
        pair_tile(
            ctx,
            src,
            &Pair {
                label: "Cost per turn",
                first: "llm.spend_usd",
                second: "llm.calls",
                combine: cost_per_call,
                unit: Unit::Usd,
                up_bad: true,
                tone: Tone::C5,
            },
        ),
        ctx.tile(
            src,
            "Calls",
            "llm.calls",
            AggFn::Sum,
            Unit::Count,
            false,
            Tone::C6,
            tab,
        ),
    ];
    panel("tiles", "", "", 12, Body::Tiles { tiles })
}

/// A tile whose value combines two metrics, such as cache reads over all input or spend per call.
struct Pair {
    label: &'static str,
    first: &'static str,
    second: &'static str,
    combine: fn(f64, f64) -> Option<f64>,
    unit: Unit,
    up_bad: bool,
    tone: Tone,
}

/// A tile of a combined value: the value and its change come from the two metrics' totals over the window and the
/// comparison window, and the sparkline combines the two sparklines point by point.
fn pair_tile(ctx: &Ctx, src: &dyn Source, pair: &Pair) -> Tile {
    let value_in = |scope: &str, from_ms: u64, to_ms: u64| {
        let first = src.scalar(pair.first, scope, from_ms, to_ms, AggFn::Sum)?;
        let second = src.scalar(pair.second, scope, from_ms, to_ms, AggFn::Sum)?;
        (pair.combine)(first, second)
    };
    let now = value_in(&ctx.q.scope, ctx.from_ms, ctx.to_ms);
    let before = if ctx.comparing() {
        value_in(&ctx.cmp_scope, ctx.cmp_from_ms, ctx.cmp_to_ms)
    } else {
        None
    };
    let first_spark = ctx.spark(src, pair.first, AggFn::Sum);
    let second_spark = ctx.spark(src, pair.second, AggFn::Sum);
    let spark = first_spark
        .iter()
        .zip(&second_spark)
        .map(|(a, b)| (pair.combine)(*a, *b).unwrap_or(0.0))
        .collect();
    Tile {
        label: pair.label.into(),
        value: now
            .map(|v| format(pair.unit, v))
            .unwrap_or_else(|| "—".into()),
        change_pct: match (now, before) {
            (Some(a), Some(b)) => change_pct(a, b),
            _ => None,
        },
        up_bad: pair.up_bad,
        note: String::new(),
        spark,
        tone: pair.tone,
        tab: Some("tokens".into()),
    }
}

/// Cache reads as a percentage of all input (cache reads and fresh input together).
fn cache_hit(reads: f64, fresh: f64) -> Option<f64> {
    let all = reads + fresh;
    (all > 0.0).then(|| reads / all * 100.0)
}

/// Spend per model call.
fn cost_per_call(spend: f64, calls: f64) -> Option<f64> {
    (calls > 0.0).then(|| spend / calls)
}

/// Input tokens by group, with output stacked on top.
fn tokens_by_group(ctx: &Ctx, src: &dyn Source) -> Panel {
    let choice = if ctx.q.group.is_empty() {
        "provider"
    } else {
        ctx.q.group.as_str()
    };
    let input = ctx.read("llm.tokens_in", AggFn::Sum, ctx.group.clone());
    let output = ctx.read("llm.tokens_out", AggFn::Sum, GroupBy::All);
    let chart = stacked(
        ctx,
        src,
        &[(input, None), (output, Some("output"))],
        Unit::Tokens,
        220,
    );
    panel(
        "tokens-by-group",
        &format!("Tokens by {choice}"),
        "input by group, output stacked on top",
        8,
        chart_or_empty(chart, "No model calls in this window."),
    )
}

/// Spend by group as a treemap of the window's totals.
fn spend_share(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = ctx.read("llm.spend_usd", AggFn::Sum, ctx.group.clone());
    let items: Vec<TreeItem> = src
        .series(&read)
        .iter()
        .enumerate()
        .filter(|(_, g)| g.total() > 0.0)
        .map(|(i, g)| TreeItem {
            name: g.name.clone(),
            value: g.total(),
            tone: Tone::chart(i),
        })
        .collect();
    let body = if items.is_empty() {
        empty("No model spend in this window.")
    } else {
        Body::Treemap {
            unit: Unit::Usd,
            items,
        }
    };
    panel(
        "spend-share",
        "Spend share",
        "spend per group over the window",
        4,
        body,
    )
}

/// Cache reads, cache writes and fresh input, stacked.
fn cache(ctx: &Ctx, src: &dyn Source) -> Panel {
    let reads = [
        (
            ctx.read("llm.tokens_cache_read", AggFn::Sum, GroupBy::All),
            Some("cache read"),
        ),
        (
            ctx.read("llm.tokens_cache_write", AggFn::Sum, GroupBy::All),
            Some("cache write"),
        ),
        (
            ctx.read("llm.tokens_in", AggFn::Sum, GroupBy::All),
            Some("fresh input"),
        ),
    ];
    let chart = stacked(ctx, src, &reads, Unit::Tokens, 200);
    panel(
        "cache",
        "Input: cached vs fresh",
        "cache reads cost about a tenth of fresh input",
        6,
        chart_or_empty(chart, "No input tokens in this window."),
    )
}

/// What each role costs per turn and per task, with its token use and a spend trend.
fn unit_cost(ctx: &Ctx, src: &dyn Source) -> Panel {
    let columns = vec![
        Column {
            head: "Role".into(),
            num: false,
        },
        Column {
            head: "Per turn".into(),
            num: true,
        },
        Column {
            head: "Per task".into(),
            num: true,
        },
        Column {
            head: "Tokens / turn".into(),
            num: true,
        },
        Column {
            head: "Out / in".into(),
            num: true,
        },
        Column {
            head: "Trend".into(),
            num: false,
        },
    ];
    let rows: Vec<Row> = src
        .entities(&ctx.q.scope, "role")
        .into_iter()
        .map(|role| {
            let scope = if role.id.starts_with("role:") {
                role.id.clone()
            } else {
                format!("role:{}", role.id)
            };
            let total =
                |metric: &str, agg: AggFn| src.scalar(metric, &scope, ctx.from_ms, ctx.to_ms, agg);
            let spend = total("llm.spend_usd", AggFn::Sum);
            let calls = total("llm.calls", AggFn::Sum);
            let tokens_in = total("llm.tokens_in", AggFn::Sum);
            let tokens_out = total("llm.tokens_out", AggFn::Sum);
            let tokens = [tokens_in, tokens_out]
                .into_iter()
                .flatten()
                .reduce(|a, b| a + b);
            let task = total("work.cost_usd", AggFn::P50);
            Row {
                cells: vec![
                    Cell::Text {
                        text: role.label.clone(),
                    },
                    num_cell(per(spend, calls), Unit::Usd),
                    num_cell(task, Unit::Usd),
                    num_cell(per(tokens, calls), Unit::Tokens),
                    num_cell(per(tokens_out, tokens_in), Unit::Ratio),
                    Cell::Spark {
                        values: role_trend(ctx, src, &scope),
                        tone: Tone::C1,
                    },
                ],
                scope: Some(scope),
                tab: None,
                trace: None,
                selected: false,
            }
        })
        .collect();
    let body = if rows.is_empty() {
        empty("No roles in this scope yet.")
    } else {
        Body::Table(Table {
            columns,
            rows,
            empty: "No roles in this scope yet.".into(),
            more: None,
        })
    };
    panel(
        "unit-cost",
        "Cost per unit of work",
        "per role · per task is the median work cost",
        6,
        body,
    )
}

/// Spend per step of a role's scope over the window, the trend in its row.
fn role_trend(ctx: &Ctx, src: &dyn Source, scope: &str) -> Vec<f64> {
    let read = Read {
        metric: "llm.spend_usd".into(),
        scope: scope.into(),
        from_ms: ctx.from_ms,
        to_ms: ctx.to_ms,
        step_ms: ((ctx.to_ms - ctx.from_ms) / 40).max(1_000),
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

/// The share of calls per model, step by step. It never draws comparison ghosts, since shares do not compare.
fn model_mix(ctx: &Ctx, src: &dyn Source) -> Panel {
    let read = ctx.read("llm.calls", AggFn::Sum, GroupBy::Label("model".into()));
    let groups = src.series(&read);
    let body = if groups.is_empty() {
        empty("No model calls in this window.")
    } else {
        let steps = groups.iter().map(|g| g.values.len()).max().unwrap_or(0);
        let totals: Vec<f64> = (0..steps)
            .map(|i| {
                groups
                    .iter()
                    .filter_map(|g| g.values.get(i).copied().flatten())
                    .sum()
            })
            .collect();
        let series = groups
            .iter()
            .enumerate()
            .map(|(i, g)| Series {
                name: g.name.clone(),
                tone: Tone::chart(i),
                dash: false,
                values: g
                    .values
                    .iter()
                    .enumerate()
                    .map(|(j, v)| match *v {
                        Some(v) if totals[j] > 0.0 => Some(v / totals[j] * 100.0),
                        _ => None,
                    })
                    .collect(),
                gap_reason: None,
                scope: g.scope.clone(),
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
            height: 200,
            annotate: true,
            point_tab: None,
        })
    };
    panel(
        "model-mix",
        "Model mix",
        "share of calls per model, over time · shows routing and fallbacks",
        6,
        body,
    )
}

/// A model's totals over the window, for the savings estimates.
struct ModelShare {
    name: String,
    provider: String,
    calls: f64,
    spend: f64,
    output: f64,
}

impl ModelShare {
    fn cost_per_call(&self) -> f64 {
        self.spend / self.calls
    }

    fn output_per_call(&self) -> f64 {
        self.output / self.calls
    }
}

/// Estimated savings: an output-heavy model whose cheaper sibling from the same provider could take its calls, and
/// the share of calls that were retries. Both are estimates, not bills.
fn savings(ctx: &Ctx, src: &dyn Source) -> Panel {
    let by_model =
        |metric: &str| src.series(&ctx.read(metric, AggFn::Sum, GroupBy::Label("model".into())));
    let calls_by_model = by_model("llm.calls");
    let spend_by_model = by_model("llm.spend_usd");
    let output_by_model = by_model("llm.tokens_out");
    let models: Vec<ModelShare> = calls_by_model
        .iter()
        .filter(|c| c.total() > 0.0)
        .map(|c| ModelShare {
            name: c.name.clone(),
            provider: c.name.split('/').next().unwrap_or("").to_string(),
            calls: c.total(),
            spend: total_named(&spend_by_model, &c.name),
            output: total_named(&output_by_model, &c.name),
        })
        .collect();

    let mut flags = Vec::new();
    for model in &models {
        if model.output_per_call() < HIGH_OUTPUT_PER_CALL {
            continue;
        }
        let sibling = models
            .iter()
            .filter(|s| s.provider == model.provider && s.cost_per_call() < model.cost_per_call())
            .min_by(|a, b| a.cost_per_call().total_cmp(&b.cost_per_call()));
        if let Some(sibling) = sibling {
            let saved = model.spend * (1.0 - sibling.cost_per_call() / model.cost_per_call());
            flags.push(Flag {
                status: Status::Info,
                text: format!(
                    "estimated: {} costs {} a call and {} costs {}; routing its calls there would save about {}",
                    model.name,
                    format(Unit::Usd, model.cost_per_call()),
                    sibling.name,
                    format(Unit::Usd, sibling.cost_per_call()),
                    format(Unit::Usd, saved),
                ),
                when: String::new(),
                tab: None,
                scope: None,
            });
        }
    }

    let total = |metric: &str| {
        src.scalar(metric, &ctx.q.scope, ctx.from_ms, ctx.to_ms, AggFn::Sum)
            .unwrap_or(0.0)
    };
    let retries = total("llm.retries");
    let all_calls = total("llm.calls");
    let all_spend = total("llm.spend_usd");
    if retries > 0.0 && all_calls > 0.0 {
        let share = retries / all_calls;
        flags.push(Flag {
            status: Status::Info,
            text: format!(
                "estimated: {} of calls were retries, about {} spent on them",
                format(Unit::Percent, share * 100.0),
                format(Unit::Usd, all_spend * share),
            ),
            when: String::new(),
            tab: None,
            scope: None,
        });
    }

    let body = if flags.is_empty() {
        empty("No savings to estimate in this window.")
    } else {
        Body::Flags { flags }
    };
    panel(
        "savings",
        "Savings available",
        "estimated, not a bill",
        6,
        body,
    )
}

/// The total of the group with this name (zero when there is none).
fn total_named(groups: &[Group], name: &str) -> f64 {
    groups
        .iter()
        .find(|g| g.name == name)
        .map(Group::total)
        .unwrap_or(0.0)
}

/// Rows of the call log on the panel; "Load older" pages by [`CALLS_PAGE`].
const CALLS_FIRST: usize = 12;

/// Rows per older page of the call log.
pub const CALLS_PAGE: usize = 50;

/// The latest model calls of the scope, each linked to its kept trace when it has one.
fn recent_calls(ctx: &Ctx, src: &dyn Source) -> Panel {
    let table = calls_table(ctx, src, ctx.to_ms, CALLS_FIRST);
    let body = if table.rows.is_empty() {
        empty("No model calls in this window.")
    } else {
        Body::Table(table)
    };
    panel("calls", "Recent model calls", "", 12, body)
}

/// The page of the call log before `before_ms` (`GET /v1/metrics/calls/page`), for the query's scope and time zone.
pub fn calls_page(q: &ViewQuery, src: &dyn Source, before_ms: u64) -> Table {
    calls_table(&Ctx::new(q, src), src, before_ms, CALLS_PAGE)
}

/// Up to `limit` calls before `before_ms`, with the route of the next page when the page is full.
fn calls_table(ctx: &Ctx, src: &dyn Source, before_ms: u64, limit: usize) -> Table {
    let columns = [
        "Time",
        "Agent",
        "Model",
        "In",
        "Cached %",
        "Out",
        "First token",
        "Total",
        "Cost",
        "Status",
    ]
    .iter()
    .enumerate()
    .map(|(i, head)| Column {
        head: (*head).into(),
        num: (3..=8).contains(&i),
    })
    .collect();
    let calls = src.calls(&ctx.q.scope, before_ms, limit);
    let more = match calls.last() {
        Some(oldest) if calls.len() == limit => Some(format!(
            "/v1/metrics/calls/page?scope={}&before={}&tz={}{}",
            query_text(&ctx.q.scope),
            oldest.at_ms,
            ctx.q.tz,
            if ctx.q.fixture { "&fixture=1" } else { "" }
        )),
        _ => None,
    };
    let rows: Vec<Row> = calls
        .into_iter()
        .map(|call| {
            let CallRow {
                at_ms,
                agent,
                model,
                tokens_in,
                cache_read,
                tokens_out,
                ttft_ms,
                duration_ms,
                cost_usd,
                status,
                trace,
                ..
            } = call;
            let cached = (tokens_in > 0).then(|| cache_read as f64 / tokens_in as f64 * 100.0);
            Row {
                cells: vec![
                    Cell::Mono {
                        text: ctx.time(at_ms),
                    },
                    Cell::Text { text: agent },
                    Cell::Mono { text: model },
                    num_cell(Some(tokens_in as f64), Unit::Tokens),
                    num_cell(cached, Unit::Percent),
                    num_cell(Some(tokens_out as f64), Unit::Tokens),
                    num_cell(ttft_ms, Unit::Ms),
                    num_cell(duration_ms, Unit::Ms),
                    num_cell(Some(cost_usd), Unit::Usd),
                    Cell::Status {
                        text: status.to_string(),
                        status: if status >= 400 {
                            Status::Crit
                        } else {
                            Status::Ok
                        },
                    },
                ],
                scope: None,
                tab: trace.as_ref().map(|_| "traces".to_string()),
                trace,
                selected: false,
            }
        })
        .collect();
    Table {
        columns,
        rows,
        empty: "No model calls in this window.".into(),
        more,
    }
}

/// A scope id as a query value: letters, digits and `-_.~:/` as they are, everything else percent-encoded.
fn query_text(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The rates behind the spend estimates, for the scope's models and those the settings name.
fn prices(ctx: &Ctx, src: &dyn Source) -> Panel {
    let used: Vec<String> = src
        .entities(&ctx.q.scope, "model")
        .into_iter()
        .filter_map(|e| e.id.strip_prefix("model:").map(str::to_string))
        .collect();
    let prices = src.prices(&used);
    let columns = [
        ("Model", false),
        ("Used by", false),
        ("Source", false),
        ("Input", true),
        ("Output", true),
        ("Cache read", true),
        ("Cache write", true),
        ("Status", false),
    ]
    .iter()
    .map(|(head, num)| Column {
        head: (*head).into(),
        num: *num,
    })
    .collect();
    let rate = |v: Option<f64>| num_cell(v, Unit::Usd);
    let rows: Vec<Row> = prices
        .rows
        .into_iter()
        .map(|p| {
            let priced = p.input.is_some();
            Row {
                cells: vec![
                    Cell::Mono { text: p.model },
                    if p.used_by.is_empty() {
                        Cell::Muted { text: "—".into() }
                    } else {
                        Cell::Text { text: p.used_by }
                    },
                    if p.source.is_empty() {
                        Cell::Muted { text: "—".into() }
                    } else {
                        Cell::Text { text: p.source }
                    },
                    rate(p.input),
                    rate(p.output),
                    rate(p.cache_read),
                    rate(p.cache_write),
                    if priced {
                        Cell::Pill {
                            text: "priced".into(),
                            status: Status::Ok,
                        }
                    } else {
                        Cell::Pill {
                            text: "unpriced: set one under Models".into(),
                            status: Status::Warn,
                        }
                    },
                ],
                scope: None,
                tab: None,
                trace: None,
                selected: false,
            }
        })
        .collect();
    let body = if rows.is_empty() {
        empty("No models in use. Models appear here once one is called or set as a default.")
    } else {
        Body::Table(Table {
            columns,
            rows,
            empty: String::new(),
            more: None,
        })
    };
    let mut p = panel(
        "prices",
        "Prices",
        "USD per million tokens; the estimates use these rates, the reported cost is what the provider charged",
        12,
        body,
    );
    p.right = match (prices.fetched_at_ms, prices.sources.is_empty()) {
        (0, _) => "not fetched yet".into(),
        (at, true) => format!("no list answered · {}", ctx.time(at)),
        (at, false) => format!("from {} · {}", prices.sources.join(" and "), ctx.time(at)),
    };
    p.actions = vec![PanelAction {
        label: "Refresh prices".into(),
        post: "/v1/prices/refresh".into(),
        done: "Prices refreshed".into(),
    }];
    p
}

/// A stacked chart of reads. Each read adds its groups as series, or one series named by its label when it has one.
/// Comparing adds one dashed total of all the reads over the comparison window.
fn stacked(
    ctx: &Ctx,
    src: &dyn Source,
    reads: &[(Read, Option<&str>)],
    unit: Unit,
    height: u16,
) -> Chart {
    let mut series = Vec::new();
    for (read, label) in reads {
        for group in src.series(read) {
            let mut s = series_of(&group, Tone::chart(series.len()));
            if let Some(label) = label {
                s.name = label.to_string();
            }
            series.push(s);
        }
    }
    let mut ghosts = Vec::new();
    if ctx.comparing() && !series.is_empty() {
        let mut total: Vec<Option<f64>> = vec![None; ctx.points];
        for (read, _) in reads {
            for group in src.series(&ctx.cmp_read(read)) {
                for (i, v) in group.values.iter().enumerate().take(ctx.points) {
                    if let Some(v) = v {
                        total[i] = Some(total[i].unwrap_or(0.0) + v);
                    }
                }
            }
        }
        ghosts.push(Series {
            name: format!("total · {}", ctx.cmp_label),
            tone: Tone::Ink3,
            dash: true,
            values: total,
            gap_reason: None,
            scope: None,
        });
    }
    Chart {
        unit,
        x: ctx.x(),
        series,
        stacked: true,
        area: false,
        y_max: None,
        band: None,
        ghosts,
        markers: Vec::new(),
        limit: None,
        height,
        annotate: true,
        point_tab: None,
    }
}

/// The chart as a body, or the empty state when it has no series.
fn chart_or_empty(chart: Chart, text: &str) -> Body {
    if chart.series.is_empty() {
        empty(text)
    } else {
        Body::Chart(chart)
    }
}

/// A formatted number cell, or a dash when there is no value.
fn num_cell(value: Option<f64>, unit: Unit) -> Cell {
    match value {
        Some(v) => Cell::Num {
            text: format(unit, v),
        },
        None => Cell::Muted { text: "—".into() },
    }
}

/// `a / b` when both are present and `b` is positive.
fn per(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) if b > 0.0 => Some(a / b),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use crate::model::{Body, View, ViewQuery};

    fn compose_tokens(compare: &str) -> View {
        let mut q = ViewQuery::new("tokens", "global", "24h");
        q.compare = compare.into();
        compose(&q, &Fixture::new(1_760_600_000_000))
    }

    /// The name of a body's variant.
    fn variant(body: &Body) -> &'static str {
        match body {
            Body::Tiles { .. } => "tiles",
            Body::Chart(_) => "chart",
            Body::Treemap { .. } => "treemap",
            Body::Table(_) => "table",
            Body::Flags { .. } => "flags",
            Body::Empty { .. } => "empty",
            _ => "other",
        }
    }

    #[test]
    fn the_tokens_tab_lists_its_nine_panels_in_order_with_the_expected_bodies() {
        let view = compose_tokens("prev");
        let expected: [(&str, &[&str]); 9] = [
            ("tiles", &["tiles"]),
            ("tokens-by-group", &["chart", "empty"]),
            ("spend-share", &["treemap", "empty"]),
            ("cache", &["chart", "empty"]),
            ("unit-cost", &["table", "empty"]),
            ("model-mix", &["chart", "empty"]),
            ("savings", &["flags", "empty"]),
            ("calls", &["table", "empty"]),
            ("prices", &["table", "empty"]),
        ];
        let ids: Vec<&str> = view.panels.iter().map(|p| p.id.as_str()).collect();
        let expected_ids: Vec<&str> = expected.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, expected_ids);
        for (panel, (id, variants)) in view.panels.iter().zip(expected) {
            let got = variant(&panel.body);
            assert!(variants.contains(&got), "panel {id} has body {got}");
        }
    }

    #[test]
    fn the_call_log_pages_older_calls_and_the_prices_panel_can_refresh() {
        let view = compose_tokens("none");
        let calls = view.panels.iter().find(|p| p.id == "calls").unwrap();
        let Body::Table(table) = &calls.body else {
            panic!("the call log is a table");
        };
        let more = table.more.as_deref().expect("a full first page offers older calls");
        assert!(more.starts_with("/v1/metrics/calls/page?scope=global&before="));
        let prices = view.panels.iter().find(|p| p.id == "prices").unwrap();
        assert_eq!(prices.actions[0].post, "/v1/prices/refresh");
        assert_eq!(super::query_text("model:a/b c"), "model:a/b%20c");
    }

    #[test]
    fn the_model_mix_chart_has_no_ghosts_even_when_comparing() {
        for compare in ["none", "prev"] {
            let view = compose_tokens(compare);
            let panel = view
                .panels
                .iter()
                .find(|p| p.id == "model-mix")
                .expect("the model-mix panel is composed");
            if let Body::Chart(chart) = &panel.body {
                assert!(chart.ghosts.is_empty(), "ghosts with compare {compare}");
            }
        }
    }
}
