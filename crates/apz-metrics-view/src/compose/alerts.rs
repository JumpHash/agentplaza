//! The alerts tab: what is firing, the rules, the service levels, and the history of rule changes.

use apz_metrics::alerts::{Rule, Severity, State};
use apz_metrics::catalog::{self, Unit};
use apz_metrics::query::AggFn;

use super::{Ctx, empty, panel, scope_info};
use crate::format::format;
use crate::model::*;
use crate::source::{AlertsState, Source};

/// The panels of the tab: `firing`, `rules`, `slos` and `history`.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let state = src.alerts();

    let firing = firing_rows(ctx, src, &state);
    let firing_total = firing.len();
    let mut firing_panel = panel(
        "firing",
        "Firing now",
        "rules run on the hub's in-memory aggregates as points arrive · no database queries",
        12,
        Body::Alerts { rows: firing },
    );
    firing_panel.right = format!("{firing_total} firing");

    let rules = rule_rows(ctx, src, &state);
    let rules_body = if rules.is_empty() {
        empty("No rules yet.")
    } else {
        Body::Rules { rules }
    };

    let slos = slo_rows(&state);
    let slos_body = if slos.is_empty() {
        empty("No service levels yet.")
    } else {
        Body::Bars {
            rows: slos,
            note: String::new(),
        }
    };

    vec![
        firing_panel,
        panel("rules", "Rules", "every rule, on or off", 8, rules_body),
        panel(
            "slos",
            "Service levels",
            "share of 5-minute windows this month that met the objective",
            4,
            slos_body,
        ),
        panel(
            "history",
            "History",
            "newest first",
            12,
            Body::Table(history_table(ctx, &state)),
        ),
    ]
}

/// The subjects that are firing or clearing and whose rule is not muted, as alert rows.
fn firing_rows(ctx: &Ctx, src: &dyn Source, state: &AlertsState) -> Vec<AlertRow> {
    state
        .subjects
        .iter()
        .filter(|(_, _, s, _)| matches!(s, State::Firing { .. } | State::Clearing { .. }))
        .filter(|(rule_id, _, _, _)| !is_muted(state, rule_id, ctx.now_ms))
        .filter_map(|(rule_id, subject, st, value)| {
            let rule = state.rules.iter().find(|r| &r.id == rule_id)?;
            let since_ms = match st {
                State::Firing { since_ms } | State::Clearing { since_ms, .. } => *since_ms,
                State::Ok | State::Pending { .. } => return None,
            };
            Some(AlertRow {
                rule: rule.id.clone(),
                name: rule.name.clone(),
                scope: subject.clone(),
                scope_label: scope_info(subject, src).label,
                cond: condition(rule),
                for_text: duration_text(rule.for_ms),
                since: ctx.time(since_ms),
                now: format(metric_unit(&rule.metric), *value),
                severity: severity_status(rule.severity),
                spark: subject_spark(ctx, src, rule, subject),
                tab: family_tab(&rule.metric).into(),
                acked: state
                    .acked
                    .iter()
                    .any(|(r, s)| r == rule_id && s == subject),
            })
        })
        .collect()
}

/// Every rule as a row: its state is "off", "muted", "firing" or "ok".
fn rule_rows(ctx: &Ctx, src: &dyn Source, state: &AlertsState) -> Vec<RuleRow> {
    state
        .rules
        .iter()
        .map(|rule| {
            let firing = state.subjects.iter().any(|(id, _, s, _)| {
                id == &rule.id && matches!(s, State::Firing { .. } | State::Clearing { .. })
            });
            let rule_state = if !rule.on {
                "off"
            } else if is_muted(state, &rule.id, ctx.now_ms) {
                "muted"
            } else if firing {
                "firing"
            } else {
                "ok"
            };
            let last = state
                .history
                .iter()
                .find(|h| h.1 == rule.id && h.3 == "fired")
                .map(|h| ctx.time(h.0))
                .unwrap_or_else(|| "never".into());
            RuleRow {
                id: rule.id.clone(),
                on: rule.on,
                name: rule.name.clone(),
                scope_label: scope_label(&rule.scope, src),
                cond: condition(rule),
                for_text: duration_text(rule.for_ms),
                severity: severity_status(rule.severity),
                state: rule_state.into(),
                last,
                starter: rule.starter,
            }
        })
        .collect()
}

/// One bar per service level: the share of its error budget left, clamped to 0..=1.
fn slo_rows(state: &AlertsState) -> Vec<BarRow> {
    state
        .slos
        .iter()
        .map(|(slo, status)| {
            let left = status.budget_left;
            let (bar_status, note) = if left < 0.0 {
                (
                    Status::Crit,
                    format!("budget spent, {}% over", (-left * 100.0).round()),
                )
            } else {
                let status = if left < 0.3 { Status::Warn } else { Status::Ok };
                (
                    status,
                    format!("{}% of error budget left", (left * 100.0).round()),
                )
            };
            BarRow {
                label: slo.name.clone(),
                value: format!("{:.1}% · goal {}%", status.met_pct, slo.goal_pct),
                frac: left.clamp(0.0, 1.0),
                status: bar_status,
                note,
            }
        })
        .collect()
}

/// The history of rule changes, newest first, as a table.
fn history_table(ctx: &Ctx, state: &AlertsState) -> Table {
    let rows = state
        .history
        .iter()
        .map(|(at, rule_id, _subject, what, value, lasted)| {
            let rule = state.rules.iter().find(|r| &r.id == rule_id);
            let unit = rule.map(|r| metric_unit(&r.metric)).unwrap_or(Unit::Count);
            Row {
                cells: vec![
                    Cell::Mono {
                        text: ctx.time(*at),
                    },
                    Cell::Text {
                        text: rule
                            .map(|r| r.name.clone())
                            .unwrap_or_else(|| rule_id.clone()),
                    },
                    Cell::Pill {
                        text: what.clone(),
                        status: event_status(what),
                    },
                    Cell::Num {
                        text: format(unit, *value),
                    },
                    Cell::Num {
                        text: lasted.map(duration_text).unwrap_or_else(|| "—".into()),
                    },
                ],
                scope: None,
                tab: None,
                trace: None,
                selected: false,
            }
        })
        .collect();
    Table {
        columns: vec![
            column("Time", false),
            column("Rule", false),
            column("Event", false),
            column("Value", true),
            column("Lasted", true),
        ],
        rows,
        empty: "No alerts have fired or resolved yet.".into(),
        more: None,
    }
}

fn column(head: &str, num: bool) -> Column {
    Column {
        head: head.into(),
        num,
    }
}

/// Whether a rule is muted now: a mute that ends after `now_ms`.
fn is_muted(state: &AlertsState, rule_id: &str, now_ms: u64) -> bool {
    state
        .muted
        .iter()
        .any(|(id, until)| id == rule_id && *until > now_ms)
}

/// The condition as text: "p95(ttft) > 2.0 s".
fn condition(rule: &Rule) -> String {
    format!(
        "{}({}) {} {}",
        agg_label(rule.agg),
        metric_short(&rule.metric),
        rule.cmp.sign(),
        format(metric_unit(&rule.metric), rule.threshold)
    )
}

/// An aggregate as its id: "p95", "avg", "sum".
fn agg_label(agg: AggFn) -> String {
    serde_json::to_value(agg)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// A metric's short name for conditions: "ttft" for "llm.ttft_ms".
fn metric_short(metric: &str) -> &str {
    let name = metric.rsplit('.').next().unwrap_or(metric);
    ["_ms", "_usd", "_pct"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
}

/// A metric's unit from the catalog (a count when the metric is unknown).
fn metric_unit(metric: &str) -> Unit {
    catalog::find(metric).map(|m| m.unit).unwrap_or(Unit::Count)
}

/// The tab an alert on a metric opens, by the family the metric id starts with.
fn family_tab(metric: &str) -> &'static str {
    if metric.contains("spend") || metric.contains("cost") {
        return "capacity";
    }
    match metric.split('.').next().unwrap_or("") {
        "llm" => "latency",
        "volume" => "storage",
        "ctx" => "context",
        "life" => "lifecycle",
        "link" | "hub" => "hub",
        "host" | "vm" | "svc" => "compute",
        "work" | "agent" | "approval" | "law" => "agents",
        _ => "overview",
    }
}

/// A duration as the console words it: "10 min", "2 h", "45 s".
fn duration_text(ms: u64) -> String {
    if ms >= 3_600_000 {
        format!("{} h", ms / 3_600_000)
    } else if ms >= 60_000 {
        format!("{} min", ms / 60_000)
    } else {
        format!("{} s", ms / 1_000)
    }
}

/// A rule's scope as words: "Every node" for an `every:node` scope, otherwise the scope's label.
fn scope_label(scope: &str, src: &dyn Source) -> String {
    match scope.strip_prefix("every:") {
        Some(kind) => format!("Every {kind}"),
        None => scope_info(scope, src).label,
    }
}

fn severity_status(severity: Severity) -> Status {
    match severity {
        Severity::Critical => Status::Crit,
        Severity::Warning => Status::Warn,
    }
}

/// The pill colour of a history event: fired is critical, resolved is ok, anything else muted.
fn event_status(what: &str) -> Status {
    match what {
        "fired" => Status::Crit,
        "resolved" => Status::Ok,
        _ => Status::Mute,
    }
}

/// The sparkline of a rule's metric in the subject's scope rather than the view's.
fn subject_spark(ctx: &Ctx, src: &dyn Source, rule: &Rule, subject: &str) -> Vec<f64> {
    let mut scoped = ctx.clone();
    scoped.q.scope = subject.into();
    scoped.spark(src, &rule.metric, rule.agg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::compose;
    use crate::fixture::Fixture;
    use apz_metrics::alerts::{Cmp, Notify};

    const NOW: u64 = 1_760_600_000_000;

    fn ttft_rule() -> Rule {
        Rule {
            id: "ttft.p95".into(),
            name: "First token slow".into(),
            metric: "llm.ttft_ms".into(),
            agg: AggFn::P95,
            scope: "global".into(),
            cmp: Cmp::Gt,
            threshold: 2_000.0,
            for_ms: 600_000,
            severity: Severity::Warning,
            on: true,
            notify: Notify::default(),
            starter: true,
        }
    }

    #[test]
    fn a_firing_subject_becomes_an_alert_row_and_a_muted_rule_shows_muted() {
        let fixture = Fixture::new(NOW);
        let ctx = Ctx::new(&ViewQuery::new("alerts", "global", "24h"), &fixture);
        let mut state = AlertsState {
            rules: vec![ttft_rule()],
            subjects: vec![(
                "ttft.p95".into(),
                "node:n1".into(),
                State::Firing {
                    since_ms: NOW - 600_000,
                },
                2_600.0,
            )],
            muted: Vec::new(),
            acked: Vec::new(),
            history: Vec::new(),
            slos: Vec::new(),
        };

        let rows = firing_rows(&ctx, &fixture, &state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cond, "p95(ttft) > 2.0 s");
        assert_eq!(rows[0].for_text, "10 min");

        state.muted.push(("ttft.p95".into(), NOW + 3_600_000));
        assert!(firing_rows(&ctx, &fixture, &state).is_empty());
        assert_eq!(rule_rows(&ctx, &fixture, &state)[0].state, "muted");
    }

    #[test]
    fn compose_returns_the_firing_rules_slos_and_history_panels() {
        let fixture = Fixture::new(NOW);
        let view = compose(&ViewQuery::new("alerts", "global", "24h"), &fixture);
        let ids: Vec<&str> = view.panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["firing", "rules", "slos", "history"]);
    }
}
