//! `agentplaza metrics`: what the plaza measured. The groups of a metric with the largest values over a window, one
//! metric's series over time (a table or CSV), the overview's tiles printed again as they change, and the alert rules
//! with the subjects firing now. Times are UTC.

use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_metrics::catalog::{self, Unit};
use apz_metrics_view::format;
use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

const GROUPINGS: [&str; 5] = ["agent", "node", "provider", "model", "role"];
const AGGREGATIONS: [&str; 12] = [
    "sum", "avg", "min", "max", "last", "rate", "count", "p10", "p50", "p90", "p95", "p99",
];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The groups of a metric with the largest values over a window (a total for counters, a mean otherwise).
    Top {
        /// The metric, by its catalog id.
        #[arg(long, default_value = "llm.spend_usd")]
        metric: String,
        /// The scope: global, node:n1, provider:anthropic, model:anthropic/opus and so on.
        #[arg(long, default_value = "global")]
        scope: String,
        /// The window ending now: a number and s, m, h or d (e.g. 15m, 24h, 7d).
        #[arg(long, default_value = "24h")]
        window: String,
        /// The grouping: agent or node (the metric's entities), provider, model, or role.
        #[arg(long, default_value = "agent", value_parser = GROUPINGS)]
        by: String,
        /// How many groups to show (1 to 200).
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: u32,
    },
    /// One metric over a window as series: a row per step and a column per group.
    Query {
        /// The metric, by its catalog id.
        #[arg(long)]
        metric: String,
        /// The scope (see `top`).
        #[arg(long, default_value = "global")]
        scope: String,
        /// The window ending now: a number and s, m, h or d (e.g. 15m, 24h, 7d).
        #[arg(long, default_value = "24h")]
        window: String,
        /// The step between points: a number and s, m, h or d (e.g. 1h, 15m).
        #[arg(long, default_value = "1h")]
        step: String,
        /// How each step becomes one value (default: sum for spend, token, call and error metrics, else avg).
        #[arg(long, value_parser = AGGREGATIONS)]
        agg: Option<String>,
        /// The grouping (see `top`).
        #[arg(long, default_value = "agent", value_parser = GROUPINGS)]
        by: String,
        /// Print CSV (epoch milliseconds and raw values) instead of a table.
        #[arg(long)]
        csv: bool,
    },
    /// The overview tiles, printed again every interval until Ctrl-C.
    Watch {
        /// The scope the overview is for.
        #[arg(long, default_value = "global")]
        scope: String,
        /// Seconds between prints (1 to 3600).
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..=3600))]
        interval: u64,
    },
    /// The alert rules, and the subjects firing now.
    Alerts,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Top {
            metric,
            scope,
            window,
            by,
            limit,
        } => {
            let body = top_body(&metric, &scope, &window, now_ms(), &by, limit)?;
            let v = ctx.client.post("/v1/metrics/query", &body)?;
            ctx.out(&v, |v| {
                let unit = unit_of(&metric);
                let rows: Vec<Vec<String>> = array(v.get("groups").unwrap_or(&Value::Null))
                    .iter()
                    .map(|g| {
                        vec![
                            ctx::s(g, "name").to_string(),
                            value_text(group_value(g), unit),
                        ]
                    })
                    .collect();
                println!("{metric}, {window} ({})", default_agg(&metric));
                ctx::table(&[by.as_str(), "value"], &rows);
            });
            Ok(())
        }
        Cmd::Query {
            metric,
            scope,
            window,
            step,
            agg,
            by,
            csv,
        } => {
            let span = window_range(&window, now_ms())?;
            let step_ms = parse_duration_ms(&step)?;
            let agg = agg.unwrap_or_else(|| default_agg(&metric).to_string());
            let body = read_body(&metric, &scope, span, step_ms, &agg, group_of(&by)?, None);
            let v = ctx.client.post("/v1/metrics/query", &body)?;
            ctx.out(&v, |v| {
                print_series(v, span.0, step_ms, unit_of(&metric), csv)
            });
            Ok(())
        }
        Cmd::Watch { scope, interval } => watch(ctx, &scope, interval),
        Cmd::Alerts => {
            let v = ctx.client.get("/v1/metrics/alerts")?;
            ctx.out(&v, |v| print_alerts(v, now_ms()));
            Ok(())
        }
    }
}

/// Prints the overview's tiles every `interval` seconds until the process is stopped (Ctrl-C). A failed read is
/// reported and the next one is tried.
fn watch(ctx: &Ctx, scope: &str, interval: u64) -> Result<(), String> {
    let path = format!(
        "/v1/metrics/view?tab=overview&scope={}&range=15m",
        ctx::seg(scope)
    );
    loop {
        match ctx.client.get(&path) {
            Ok(v) => ctx.out(&v, print_tiles),
            Err(message) => eprintln!("{message}"),
        }
        println!();
        thread::sleep(Duration::from_secs(interval));
    }
}

/// The body of the `top` read: one step spanning the window, so each group's value is its total (or mean) over it,
/// and only the `limit` largest groups.
pub fn top_body(
    metric: &str,
    scope: &str,
    window: &str,
    now: u64,
    by: &str,
    limit: u32,
) -> Result<Value, String> {
    let (from, to) = window_range(window, now)?;
    Ok(read_body(
        metric,
        scope,
        (from, to),
        (to - from).max(1),
        default_agg(metric),
        group_of(by)?,
        Some(limit),
    ))
}

/// The body of `POST /v1/metrics/query`: one read of a metric over `span` (from, to), in steps of `step_ms`.
fn read_body(
    metric: &str,
    scope: &str,
    span: (u64, u64),
    step_ms: u64,
    agg: &str,
    group: &str,
    top: Option<u32>,
) -> Value {
    json!({
        "metric": metric,
        "scope": scope,
        "from_ms": span.0,
        "to_ms": span.1,
        "step_ms": step_ms,
        "agg": agg,
        "group": group,
        "top": top,
    })
}

/// A duration in milliseconds, from a count and a unit (`s`, `m`, `h` or `d`), as in `15m` or `7d`.
pub fn parse_duration_ms(text: &str) -> Result<u64, String> {
    let refusal =
        || format!("a duration is a number and s, m, h or d (like 15m or 7d), not {text:?}");
    let (at, unit) = text.char_indices().last().ok_or_else(refusal)?;
    let per_unit: u64 = match unit {
        's' => 1_000,
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        _ => return Err(refusal()),
    };
    let count: u64 = match text[..at].parse() {
        Ok(count) if count > 0 => count,
        _ => return Err(refusal()),
    };
    count.checked_mul(per_unit).ok_or_else(refusal)
}

/// The window ending at `now`, as `(from, to)` in epoch milliseconds. `from` is 0 when the window reaches back past
/// the epoch.
pub fn window_range(window: &str, now: u64) -> Result<(u64, u64), String> {
    let span = parse_duration_ms(window)?;
    Ok((now.saturating_sub(span), now))
}

/// The aggregation a metric is read with: a sum for spend, token, call and error totals, else a mean.
pub fn default_agg(metric: &str) -> &'static str {
    let total = ["spend", "tokens", "calls", "errors"]
        .iter()
        .any(|word| metric.contains(word));
    if total { "sum" } else { "avg" }
}

/// The hub's group for a `--by` choice: agents and nodes are the metric's entities, the others a label or an
/// agent's property.
pub fn group_of(by: &str) -> Result<&'static str, String> {
    match by {
        "agent" | "node" => Ok("entity"),
        "provider" => Ok("label:provider"),
        "model" => Ok("label:model"),
        "role" => Ok("prop:role"),
        other => Err(format!(
            "by must be agent, node, provider, model or role, not {other:?}"
        )),
    }
}

/// The unit a metric reads in: its catalog unit, or a plain count for an id the catalog does not name.
fn unit_of(metric: &str) -> Unit {
    catalog::find(metric).map_or(Unit::Count, |def| def.unit)
}

/// The current time in epoch milliseconds.
fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    since.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// A text element of a JSON tuple (empty when missing).
fn tuple_text(tuple: &Value, index: usize) -> &str {
    tuple.get(index).and_then(Value::as_str).unwrap_or("")
}

/// A state's name, lowercased, whether the JSON is the bare name or a tagged object.
fn state_name(state: Option<&Value>) -> String {
    match state {
        Some(Value::String(name)) => name.to_lowercase(),
        Some(Value::Object(map)) => map
            .keys()
            .next()
            .map_or(String::new(), |k| k.to_lowercase()),
        _ => String::new(),
    }
}

/// A group's latest value (its last point that is not a gap).
fn group_value(group: &Value) -> Option<f64> {
    points(group).into_iter().rev().flatten().next()
}

/// A group's points, one per step (`None` for a gap).
fn points(group: &Value) -> Vec<Option<f64>> {
    array(group.get("values").unwrap_or(&Value::Null))
        .iter()
        .map(Value::as_f64)
        .collect()
}

/// A value as text in the metric's unit, or "—" for a gap.
fn value_text(value: Option<f64>, unit: Unit) -> String {
    value.map_or_else(|| "—".to_string(), |v| format::format(unit, v))
}

/// Prints grouped series: a table with a row per step (the time in UTC) and a column per group, or CSV.
fn print_series(v: &Value, from_ms: u64, step_ms: u64, unit: Unit, csv: bool) {
    let groups = array(v.get("groups").unwrap_or(&Value::Null));
    let steps = groups.iter().map(|g| points(g).len()).max().unwrap_or(0);
    let names: Vec<&str> = groups.iter().map(|g| ctx::s(g, "name")).collect();
    let time_at = |i: usize| from_ms + i as u64 * step_ms;
    if csv {
        let header: Vec<String> = names.iter().map(|name| csv_cell(name)).collect();
        println!("t_ms,{}", header.join(","));
        for i in 0..steps {
            let cells: Vec<String> = std::iter::once(time_at(i).to_string())
                .chain(groups.iter().map(|g| {
                    points(g)
                        .get(i)
                        .copied()
                        .flatten()
                        .map_or(String::new(), |x| x.to_string())
                }))
                .collect();
            println!("{}", cells.join(","));
        }
        return;
    }
    let mut header = vec!["time"];
    header.extend(names.iter().copied());
    let rows: Vec<Vec<String>> = (0..steps)
        .map(|i| {
            std::iter::once(format::time_label(time_at(i), step_ms, 0))
                .chain(
                    groups
                        .iter()
                        .map(|g| value_text(points(g).get(i).copied().flatten(), unit)),
                )
                .collect()
        })
        .collect();
    ctx::table(&header, &rows);
}

/// A CSV field, quoted when it holds a comma, a quote or a line break.
fn csv_cell(text: &str) -> String {
    if text.contains([',', '"', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// Prints the overview's tiles as `label: value (change)`; the change is a signed percentage, else the tile's note.
fn print_tiles(v: &Value) {
    for panel in array(v.get("panels").unwrap_or(&Value::Null)) {
        let Some(body) = panel
            .get("body")
            .filter(|body| ctx::s(body, "type") == "tiles")
        else {
            continue;
        };
        for tile in array(body.get("tiles").unwrap_or(&Value::Null)) {
            println!(
                "{}: {}{}",
                ctx::s(tile, "label"),
                ctx::s(tile, "value"),
                change_text(tile)
            );
        }
    }
}

/// A tile's change: a signed percentage, else its note, else nothing.
fn change_text(tile: &Value) -> String {
    if let Some(pct) = tile.get("change_pct").and_then(Value::as_f64) {
        format!(" ({pct:+.1}%)")
    } else {
        match ctx::s(tile, "note") {
            "" => String::new(),
            note => format!(" ({note})"),
        }
    }
}

/// A rule's state: off when switched off, else firing while a subject fires, muted while a mute lasts, else ok.
fn rule_state(on: bool, firing: bool, muted: bool) -> &'static str {
    match (on, firing, muted) {
        (false, _, _) => "off",
        (true, true, _) => "firing",
        (true, false, true) => "muted",
        _ => "ok",
    }
}

/// Prints the subjects firing now, then the rules with their condition and state.
fn print_alerts(v: &Value, now: u64) {
    let rules = array(v.get("rules").unwrap_or(&Value::Null));
    let subjects = array(v.get("subjects").unwrap_or(&Value::Null));
    let muted = array(v.get("muted").unwrap_or(&Value::Null));

    let firing: Vec<Vec<String>> = subjects
        .iter()
        .filter(|s| state_name(s.get(2)) == "firing")
        .map(|s| {
            let unit = rules
                .iter()
                .find(|r| ctx::s(r, "id") == tuple_text(s, 0))
                .map_or(Unit::Count, |r| unit_of(ctx::s(r, "metric")));
            vec![
                tuple_text(s, 0).to_string(),
                tuple_text(s, 1).to_string(),
                value_text(s.get(3).and_then(Value::as_f64), unit),
            ]
        })
        .collect();
    println!("firing");
    ctx::table(&["rule", "subject", "value"], &firing);

    let rows: Vec<Vec<String>> = rules
        .iter()
        .map(|rule| {
            let id = ctx::s(rule, "id");
            let on = rule.get("on").and_then(Value::as_bool).unwrap_or(false);
            let metric = ctx::s(rule, "metric");
            let sign = match ctx::s(rule, "cmp") {
                "gt" => ">",
                "lt" => "<",
                other => other,
            };
            let threshold = rule.get("threshold").and_then(Value::as_f64).unwrap_or(0.0);
            let condition = format!(
                "{metric} {} {sign} {}",
                ctx::s(rule, "agg"),
                format::format(unit_of(metric), threshold)
            );
            let is_firing = subjects
                .iter()
                .any(|s| tuple_text(s, 0) == id && state_name(s.get(2)) == "firing");
            let is_muted = muted.iter().any(|m| {
                tuple_text(m, 0) == id && m.get(1).and_then(Value::as_u64).unwrap_or(0) > now
            });
            vec![
                id.to_string(),
                if on { "yes" } else { "no" }.to_string(),
                ctx::s(rule, "name").to_string(),
                condition,
                rule_state(on, is_firing, is_muted).to_string(),
            ]
        })
        .collect();
    println!();
    println!("rules");
    ctx::table(&["id", "on", "name", "condition", "state"], &rows);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_a_count_of_seconds_minutes_hours_or_days() {
        assert_eq!(parse_duration_ms("30s"), Ok(30_000));
        assert_eq!(parse_duration_ms("15m"), Ok(900_000));
        assert_eq!(parse_duration_ms("1h"), Ok(3_600_000));
        assert_eq!(parse_duration_ms("7d"), Ok(604_800_000));
        assert!(parse_duration_ms("").is_err());
        assert!(parse_duration_ms("h").is_err());
        assert!(parse_duration_ms("0h").is_err());
        assert!(parse_duration_ms("3w").is_err());
    }

    #[test]
    fn a_window_ends_now_and_never_starts_before_the_epoch() {
        let now = 100_000_000_000;
        assert_eq!(window_range("24h", now), Ok((now - 86_400_000, now)));
        assert_eq!(window_range("15m", 1_000), Ok((0, 1_000)));
    }

    #[test]
    fn top_asks_for_one_step_over_the_window_grouped_as_asked() {
        let now = 100_000_000_000;
        let body = top_body("llm.spend_usd", "global", "24h", now, "provider", 5).unwrap();
        assert_eq!(body["metric"], "llm.spend_usd");
        assert_eq!(body["scope"], "global");
        assert_eq!(body["from_ms"], now - 86_400_000);
        assert_eq!(body["to_ms"], now);
        assert_eq!(body["step_ms"], 86_400_000);
        assert_eq!(body["agg"], "sum");
        assert_eq!(body["group"], "label:provider");
        assert_eq!(body["top"], 5);
    }

    #[test]
    fn a_spend_metric_is_summed_and_a_latency_metric_is_averaged() {
        assert_eq!(default_agg("llm.spend_usd"), "sum");
        assert_eq!(default_agg("llm.tokens_in"), "sum");
        assert_eq!(default_agg("llm.ttft_ms"), "avg");
    }
}
