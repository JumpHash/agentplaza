//! The capacity tab: month-end spend against the owner's budget, the daily caps and provider rate limits, the
//! headroom of each node, and the volume that fills first.

use apz_metrics::Point;
use apz_metrics::catalog::Unit;
use apz_metrics::forecast;
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, status_of};
use crate::format::{format, time_label};
use crate::model::*;
use crate::source::{GroupBy, Read, Source};

const DAY_MS: u64 = 86_400_000;
const HOUR_MS: u64 = 3_600_000;
const SIX_HOURS_MS: u64 = 21_600_000;
/// The memory of one VM, for the count of VMs that still fit.
const VM_MEM_MIB: f64 = 2048.0;
/// How many days of volume history the disk projection reads.
const DISK_HISTORY_DAYS: u64 = 7;
/// How many days ahead the disk projection extends.
const DISK_PROJECTION_DAYS: u64 = 4;
/// A disk that fills within this many days is critical.
const DISK_CRIT_DAYS: f64 = 7.0;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let month = Month::build(ctx, src);
    let nodes = load_nodes(ctx, src);
    let limits = rate_bars(src);
    let limits_body = if limits.is_empty() {
        empty("No rate limits reported.")
    } else {
        Body::Bars {
            rows: limits,
            note: String::new(),
        }
    };
    vec![
        panel(
            "tiles",
            "",
            "This month, and when things run out",
            12,
            Body::Tiles {
                tiles: spend_and_capacity_tiles(&month, &nodes),
            },
        ),
        panel(
            "forecast",
            "Spend forecast",
            "Spend by day against the monthly budget",
            8,
            forecast_chart(&month),
        ),
        panel(
            "budgets",
            "Budgets",
            "Spend today against each cap",
            4,
            budget_bars(src),
        ),
        panel(
            "headroom",
            "Node headroom",
            "Free CPU, memory and disk, and when each disk fills",
            8,
            headroom_table(&nodes, ctx.now_ms),
        ),
        panel(
            "rate-limits",
            "Provider rate limits",
            "Share of each provider's window still left",
            4,
            limits_body,
        ),
        disk_panel(&nodes, ctx.now_ms, ctx.q.tz),
    ]
}

/// The month the tab is about: its days, today's place in it, and the spend read so far.
struct Month {
    tz: i32,
    now: u64,
    /// The local day number of the first day of the month.
    start: i64,
    /// Days in the month.
    days: usize,
    /// The index of today in the month, counting from zero.
    today: usize,
    /// Whole days left in the month after today.
    days_left: f64,
    /// Spend per day from the first day of the month to now; the last value is today, so far.
    values: Vec<Option<f64>>,
    /// Whether any spend was read this month.
    has_month: bool,
    /// The spend of the month so far.
    to_date: f64,
    /// Daily spend over the basis window: the full days before today.
    basis: Vec<Point>,
    /// The length of the basis window in days.
    basis_days: i64,
    /// The mean burn over the basis window and the month-end projection built from it.
    spend: forecast::MonthSpend,
    budget: Option<f64>,
}

impl Month {
    fn build(ctx: &Ctx, src: &dyn Source) -> Month {
        let tz = ctx.q.tz;
        let now = ctx.now_ms;
        let today = local_day(now, tz);
        let (year, month, _) = civil_from_days(today);
        let start = days_from_civil(year, month, 1);
        let days = days_in_month(year, month) as usize;
        let today_index = (today - start) as usize;
        let days_left = (days - today_index - 1) as f64;

        let basis_days = i64::from(src.basis_days().max(1));
        let basis_start = day_start(today - basis_days, tz);
        let basis_values = first_values(
            src,
            &spend_read(&ctx.q.scope, basis_start, day_start(today, tz)),
        );
        let basis = points(&basis_values, basis_start, DAY_MS);

        let values = first_values(src, &spend_read(&ctx.q.scope, day_start(start, tz), now));
        let to_date: f64 = values.iter().flatten().sum();
        let spend = forecast::month_spend(&basis, to_date, days_left);
        Month {
            tz,
            now,
            start,
            days,
            today: today_index,
            days_left,
            has_month: values.iter().any(Option::is_some),
            values,
            to_date,
            basis,
            basis_days,
            spend,
            budget: src.monthly_budget(),
        }
    }
}

/// One node's capacity now and its volumes over the last week.
struct Node {
    /// The node's scope id, such as `node:n1`.
    scope: String,
    /// The node's id, such as `n1`.
    id: String,
    label: String,
    cpu_pct: Option<f64>,
    mem_free_mib: Option<f64>,
    /// The node's CPU count, when its report carries one.
    cores: Option<f64>,
    volumes: Vec<Volume>,
}

/// One volume's fill over the last week, sampled every six hours.
struct Volume {
    name: String,
    /// The time of the first sample.
    from_ms: u64,
    /// One sample per six hours, with gaps as `None`.
    values: Vec<Option<f64>>,
    /// When the fitted fill reaches 100 %, if it does within ten years.
    full_at: Option<u64>,
}

fn load_nodes(ctx: &Ctx, src: &dyn Source) -> Vec<Node> {
    let now = ctx.now_ms;
    let current_from = now.saturating_sub(15 * 60_000);
    let disk_from = now.saturating_sub(DISK_HISTORY_DAYS * DAY_MS);
    src.scopes()
        .into_iter()
        .filter(|s| s.kind == "node")
        .map(|s| {
            let latest = |metric: &str| src.scalar(metric, &s.id, current_from, now, AggFn::Last);
            let mem_total = latest("host.mem_total_mib");
            let mem_used = latest("host.mem_used_mib");
            let read = Read {
                metric: "volume.used_pct".into(),
                scope: s.id.clone(),
                from_ms: disk_from,
                to_ms: now,
                step_ms: SIX_HOURS_MS,
                agg: AggFn::Last,
                group: GroupBy::Entity,
                top: None,
            };
            let volumes = src
                .series(&read)
                .into_iter()
                .map(|g| {
                    let history = points(&g.values, disk_from, SIX_HOURS_MS);
                    Volume {
                        name: g.name,
                        from_ms: disk_from,
                        full_at: forecast::crossing(&history, 100.0),
                        values: g.values,
                    }
                })
                .collect();
            Node {
                id: s.id.strip_prefix("node:").unwrap_or(&s.id).to_string(),
                scope: s.id.clone(),
                label: s.label.clone(),
                cpu_pct: latest("host.cpu_pct"),
                mem_free_mib: mem_total
                    .zip(mem_used)
                    .map(|(total, used)| (total - used).max(0.0)),
                cores: src.report(&s.id).get("cpus").and_then(|v| v.as_f64()),
                volumes,
            }
        })
        .collect()
}

fn spend_and_capacity_tiles(m: &Month, nodes: &[Node]) -> Vec<Tile> {
    let spend = &m.spend;
    let projected_status = match m.budget {
        Some(b) if b > 0.0 => status_of(spend.projected / b, 0.9, 1.0),
        _ => Status::Mute,
    };
    let budget_note = match m.budget {
        Some(b) if b > 0.0 => format!(
            "budget {} · {:.0}%",
            format(Unit::Usd, b),
            spend.projected / b * 100.0
        ),
        _ => "no monthly budget set".to_string(),
    };
    let burn = if m.basis.is_empty() {
        "—".to_string()
    } else {
        format!("{}/day", format(Unit::Usd, spend.burn_per_day))
    };

    let fit_mem: Vec<f64> = nodes
        .iter()
        .filter_map(|n| n.mem_free_mib)
        .map(|m| (m / VM_MEM_MIB).floor())
        .collect();
    let fit_cpu: Vec<f64> = nodes
        .iter()
        .filter_map(|n| match (n.cores, n.cpu_pct) {
            (Some(cores), Some(pct)) => Some(((100.0 - pct).max(0.0) / 100.0 * cores).floor()),
            _ => None,
        })
        .collect();
    let vms_value = if fit_mem.is_empty() {
        "—".to_string()
    } else {
        format(Unit::Count, fit_mem.iter().sum::<f64>())
    };
    let vms_note = if fit_cpu.is_empty() {
        "by memory".to_string()
    } else {
        format!(
            "by memory; {} by CPU",
            format(Unit::Count, fit_cpu.iter().sum::<f64>())
        )
    };

    let disk = match first_full(nodes) {
        Some((node, _, full_at)) => {
            let tone = if days_until(full_at, m.now) < DISK_CRIT_DAYS {
                Tone::Crit
            } else {
                Tone::Ok
            };
            stat(
                &format!("{} disk full in", node.label),
                until(full_at, m.now),
                format!("full on {}", time_label(full_at, DAY_MS, m.tz)),
                tone,
                Some("storage"),
            )
        }
        None => stat(
            "Disk full in",
            "—".to_string(),
            "no volume is filling".to_string(),
            Tone::C1,
            Some("storage"),
        ),
    };

    vec![
        stat(
            "Month to date",
            money_or_dash(m.has_month, m.to_date),
            format!("day {} of {}", m.today + 1, m.days),
            Tone::C1,
            Some("tokens"),
        ),
        stat(
            "Projected month end",
            money_or_dash(m.has_month || !m.basis.is_empty(), spend.projected),
            budget_note,
            tone_of(projected_status),
            Some("tokens"),
        ),
        stat(
            "Burn rate",
            burn,
            format!("mean of the last {} days", m.basis_days),
            Tone::C1,
            Some("tokens"),
        ),
        stat(
            "VMs that still fit",
            vms_value,
            vms_note,
            Tone::C1,
            Some("compute"),
        ),
        disk,
    ]
}

/// A stat tile with no comparison and no sparkline.
fn stat(label: &str, value: String, note: String, tone: Tone, tab: Option<&str>) -> Tile {
    Tile {
        label: label.into(),
        value,
        change_pct: None,
        up_bad: false,
        note,
        spark: Vec::new(),
        tone,
        tab: tab.map(String::from),
    }
}

fn forecast_chart(m: &Month) -> Body {
    if !m.has_month {
        return empty("No model calls this month yet.");
    }
    let spend = &m.spend;
    let mtd = m.to_date;
    let mut actual = vec![None; m.days];
    let mut cumulative = 0.0;
    for (i, slot) in actual.iter_mut().enumerate().take(m.today + 1) {
        cumulative += m.values.get(i).copied().flatten().unwrap_or(0.0);
        *slot = Some(cumulative);
    }
    let mut projected = vec![None; m.days];
    let mut lo = vec![0.0; m.days];
    let mut hi = vec![0.0; m.days];
    for i in 0..m.days {
        if i < m.today {
            let value = actual[i].unwrap_or(0.0);
            lo[i] = value;
            hi[i] = value;
        } else {
            let steps = (i - m.today) as f64;
            let frac = if m.days_left > 0.0 {
                steps / m.days_left
            } else {
                0.0
            };
            projected[i] = Some(mtd + spend.burn_per_day * steps);
            lo[i] = mtd + (spend.lo - mtd) * frac;
            hi[i] = mtd + (spend.hi - mtd) * frac;
        }
    }
    let labels: Vec<String> = (0..m.days)
        .map(|i| time_label(day_start(m.start + i as i64, m.tz), DAY_MS, m.tz))
        .collect();
    Body::Chart(Chart {
        unit: Unit::Usd,
        x: XAxis::Labels { labels },
        series: vec![
            Series {
                name: "actual".into(),
                tone: Tone::C1,
                dash: false,
                values: actual,
                gap_reason: None,
                scope: None,
            },
            Series {
                name: "projected".into(),
                tone: Tone::C2,
                dash: true,
                values: projected,
                gap_reason: None,
                scope: None,
            },
        ],
        stacked: false,
        area: false,
        y_max: None,
        // In the projection's tone, so the band reads as its spread (and shows on the dark sheet too).
        band: Some(Band {
            name: "p10–p90".into(),
            tone: Tone::C2,
            lo,
            hi,
        }),
        ghosts: Vec::new(),
        markers: Vec::new(),
        limit: m.budget.map(|b| Limit {
            value: b,
            label: format!("budget {}", format(Unit::Usd, b)),
        }),
        height: 220,
        annotate: false,
        point_tab: None,
    })
}

fn budget_bars(src: &dyn Source) -> Body {
    let budgets = src.budgets();
    if budgets.is_empty() {
        return empty("No daily caps set.");
    }
    let rows = budgets
        .iter()
        .map(|b| {
            let frac = if b.cap_usd > 0.0 {
                b.today_usd / b.cap_usd
            } else {
                0.0
            };
            BarRow {
                label: b.label.clone(),
                value: format!(
                    "{} of {} today",
                    format(Unit::Usd, b.today_usd),
                    format(Unit::Usd, b.cap_usd)
                ),
                frac: frac.clamp(0.0, 1.0),
                status: status_of(frac, 0.75, 0.9),
                note: String::new(),
            }
        })
        .collect();
    Body::Bars {
        rows,
        note: "Caps are enforced by laws; this view only reports them.".into(),
    }
}

fn rate_bars(src: &dyn Source) -> Vec<BarRow> {
    let mut rows = Vec::new();
    for (provider, requests, tokens) in src.rate_limits() {
        for (what, left) in [("requests", requests), ("tokens", tokens)] {
            if let Some(left) = left {
                rows.push(BarRow {
                    label: format!("{provider} · {what}"),
                    value: format!("{left:.0}% left"),
                    frac: (left / 100.0).clamp(0.0, 1.0),
                    status: status_of(1.0 - left / 100.0, 0.75, 0.9),
                    note: String::new(),
                });
            }
        }
    }
    rows
}

fn headroom_table(nodes: &[Node], now: u64) -> Body {
    if nodes.is_empty() {
        return empty("No nodes have reported yet.");
    }
    let column = |head: &str, num: bool| Column {
        head: head.into(),
        num,
    };
    Body::Table(Table {
        columns: vec![
            column("Node", false),
            column("CPU free", true),
            column("Memory free", true),
            column("Fits (2 GiB VMs)", true),
            column("Disk free", true),
            column("Disk full in", true),
        ],
        rows: nodes.iter().map(|n| headroom_row(n, now)).collect(),
        empty: "No nodes have reported yet.".into(),
        more: None,
    })
}

fn headroom_row(node: &Node, now: u64) -> Row {
    let full = match node_full_at(node) {
        Some(at) => Cell::Status {
            text: until(at, now),
            status: if days_until(at, now) < DISK_CRIT_DAYS {
                Status::Crit
            } else {
                Status::Ok
            },
        },
        None => Cell::Status {
            text: "—".to_string(),
            status: Status::Mute,
        },
    };
    Row {
        cells: vec![
            Cell::Named {
                name: node.label.clone(),
                id: node.id.clone(),
            },
            Cell::Num {
                text: text_or_dash(
                    node.cpu_pct
                        .map(|pct| format(Unit::Percent, (100.0 - pct).max(0.0))),
                ),
            },
            Cell::Num {
                text: text_or_dash(node.mem_free_mib.map(|mib| format(Unit::Mib, mib))),
            },
            Cell::Num {
                text: text_or_dash(
                    node.mem_free_mib
                        .map(|mib| format(Unit::Count, (mib / VM_MEM_MIB).floor())),
                ),
            },
            Cell::Num {
                text: text_or_dash(
                    worst_used(node).map(|used| format(Unit::Percent, (100.0 - used).max(0.0))),
                ),
            },
            full,
        ],
        scope: Some(node.scope.clone()),
        tab: None,
        trace: None,
        selected: false,
    }
}

/// The disk projection: the volume that fills first, its last week and four days of the linear trend, with the
/// crossing marked. Falls back to the fullest volume when none fills on its trend.
fn disk_panel(nodes: &[Node], now: u64, tz: i32) -> Panel {
    let picked = first_full(nodes)
        .map(|(node, vol, _)| (node, vol))
        .or_else(|| fullest(nodes));
    let found = picked.and_then(|(node, vol)| {
        disk_chart(vol, now, tz).map(|body| {
            let title = format!("{}, projected", vol.name);
            let sub = format!(
                "{} · {}, projected {} days ahead",
                node.label, vol.name, DISK_PROJECTION_DAYS
            );
            (title, sub, body)
        })
    });
    let (title, sub, body) = match found {
        Some(found) => found,
        None => (
            "Disk projection".to_string(),
            "Volumes filling first".to_string(),
            empty("No volume has reported its use in the last 7 days."),
        ),
    };
    panel("disk-projection", &title, &sub, 12, body)
}

fn disk_chart(vol: &Volume, now: u64, tz: i32) -> Option<Body> {
    let last_index = vol.values.iter().rposition(Option::is_some)?;
    let history = points(&vol.values, vol.from_ms, SIX_HOURS_MS);
    let last_at = history.last()?.0;
    let projection = forecast::linear(
        &history,
        last_at + DISK_PROJECTION_DAYS * DAY_MS,
        SIX_HOURS_MS,
    );
    let total = vol.values.len() + projection.points.len();
    let used: Vec<Option<f64>> = vol
        .values
        .iter()
        .copied()
        .chain(std::iter::repeat(None))
        .take(total)
        .collect();
    let mut projected = vec![None; total];
    projected[last_index] = vol.values[last_index];
    for (k, (_, value)) in projection.points.iter().enumerate() {
        projected[last_index + k + 1] = Some(*value);
    }
    let labels: Vec<String> = (0..total)
        .map(|i| time_label(vol.from_ms + i as u64 * SIX_HOURS_MS, HOUR_MS, tz))
        .collect();
    let markers = vol
        .full_at
        .into_iter()
        .filter_map(|full_at| {
            let steps = (full_at.saturating_sub(last_at) + SIX_HOURS_MS / 2) / SIX_HOURS_MS;
            let index = last_index + steps as usize;
            (index < total).then(|| Marker {
                index,
                label: if full_at > now {
                    format!("full ~ in {}", until(full_at, now))
                } else {
                    "full".to_string()
                },
            })
        })
        .collect();
    Some(Body::Chart(Chart {
        unit: Unit::Percent,
        x: XAxis::Labels { labels },
        series: vec![
            Series {
                name: "used".into(),
                tone: Tone::C1,
                dash: false,
                values: used,
                gap_reason: None,
                scope: None,
            },
            Series {
                name: "projected".into(),
                tone: Tone::C2,
                dash: true,
                values: projected,
                gap_reason: None,
                scope: None,
            },
        ],
        stacked: false,
        area: false,
        y_max: Some(100.0),
        band: None,
        ghosts: Vec::new(),
        markers,
        limit: Some(Limit {
            value: 100.0,
            label: "full".into(),
        }),
        height: 200,
        annotate: true,
        point_tab: None,
    }))
}

/// The volume whose fitted fill reaches 100 % first, with its node and the time it does.
fn first_full(nodes: &[Node]) -> Option<(&Node, &Volume, u64)> {
    nodes
        .iter()
        .flat_map(|node| node.volumes.iter().map(move |vol| (node, vol)))
        .filter_map(|(node, vol)| vol.full_at.map(|at| (node, vol, at)))
        .min_by_key(|&(_, _, at)| at)
}

/// The volume with the highest latest reading, when none fills on its trend.
fn fullest(nodes: &[Node]) -> Option<(&Node, &Volume)> {
    nodes
        .iter()
        .flat_map(|node| node.volumes.iter().map(move |vol| (node, vol)))
        .max_by(|a, b| {
            last_value(&a.1.values)
                .unwrap_or(0.0)
                .total_cmp(&last_value(&b.1.values).unwrap_or(0.0))
        })
}

fn node_full_at(node: &Node) -> Option<u64> {
    node.volumes.iter().filter_map(|v| v.full_at).min()
}

fn worst_used(node: &Node) -> Option<f64> {
    node.volumes
        .iter()
        .filter_map(|v| last_value(&v.values))
        .reduce(f64::max)
}

fn last_value(values: &[Option<f64>]) -> Option<f64> {
    values.iter().rev().flatten().next().copied()
}

fn first_values(src: &dyn Source, read: &Read) -> Vec<Option<f64>> {
    src.series(read)
        .into_iter()
        .next()
        .map(|g| g.values)
        .unwrap_or_default()
}

/// The daily spend read of a scope over a window, one value per day.
fn spend_read(scope: &str, from_ms: u64, to_ms: u64) -> Read {
    Read {
        metric: "llm.spend_usd".into(),
        scope: scope.into(),
        from_ms,
        to_ms,
        step_ms: DAY_MS,
        agg: AggFn::Sum,
        group: GroupBy::All,
        top: None,
    }
}

/// The samples that have a value, as (time, value) points.
fn points(values: &[Option<f64>], from_ms: u64, step_ms: u64) -> Vec<Point> {
    values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|v| (from_ms + i as u64 * step_ms, v)))
        .collect()
}

fn money_or_dash(shown: bool, usd: f64) -> String {
    if shown {
        format(Unit::Usd, usd)
    } else {
        "—".to_string()
    }
}

fn text_or_dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "—".to_string())
}

fn tone_of(status: Status) -> Tone {
    match status {
        Status::Ok => Tone::Ok,
        Status::Warn => Tone::Warn,
        Status::Crit => Tone::Crit,
        _ => Tone::C1,
    }
}

fn days_until(at_ms: u64, now_ms: u64) -> f64 {
    (at_ms as f64 - now_ms as f64) / DAY_MS as f64
}

/// How long until a time, as "3.4 days", or "now" once it has passed.
fn until(at_ms: u64, now_ms: u64) -> String {
    if at_ms <= now_ms {
        "now".to_string()
    } else {
        format!("{:.1} days", days_until(at_ms, now_ms))
    }
}

/// The local day number (days since 1970-01-01 in the console's zone) of a time.
fn local_day(t_ms: u64, tz: i32) -> i64 {
    (t_ms as i64 + i64::from(tz) * 60_000).div_euclid(DAY_MS as i64)
}

/// The UTC time of local midnight at the start of a local day number.
fn day_start(day: i64, tz: i32) -> u64 {
    (day * DAY_MS as i64 - i64::from(tz) * 60_000).max(0) as u64
}

/// The local day number of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil (year, month, day) of a local day number.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The number of days in a month, found from the day numbers of its first day and the next month's.
fn days_in_month(year: i64, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (days_from_civil(next_year, next_month, 1) - days_from_civil(year, month, 1)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::Fixture;
    use crate::model::ViewQuery;

    fn kind(body: &Body) -> &'static str {
        match body {
            Body::Tiles { .. } => "tiles",
            Body::Chart(_) => "chart",
            Body::Bars { .. } => "bars",
            Body::Table(_) => "table",
            Body::Empty { .. } => "empty",
            _ => "other",
        }
    }

    #[test]
    fn the_capacity_tab_composes_its_six_panels_in_order() {
        let src = Fixture::new(1_760_600_000_000);
        let q = ViewQuery::new("capacity", "global", "24h");
        let view = crate::compose::compose(&q, &src);
        let expected = [
            ("tiles", "tiles"),
            ("forecast", "chart"),
            ("budgets", "bars"),
            ("headroom", "table"),
            ("rate-limits", "bars"),
            ("disk-projection", "chart"),
        ];
        let ids: Vec<&str> = view.panels.iter().map(|p| p.id.as_str()).collect();
        let want: Vec<&str> = expected.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, want);
        for (panel, (_, body_kind)) in view.panels.iter().zip(expected) {
            let got = kind(&panel.body);
            assert!(
                got == body_kind || got == "empty",
                "panel {} is {got}, expected {body_kind} or empty",
                panel.id
            );
        }
    }

    #[test]
    fn a_month_has_31_days_in_october_and_29_in_february_2028() {
        assert_eq!(days_in_month(2026, 10), 31);
        assert_eq!(days_in_month(2028, 2), 29);
    }
}
