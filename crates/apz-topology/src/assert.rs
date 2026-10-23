//! The assertions of a topology checked over what its run observed (PLAN 22.7): each step is a verdict with its detail
//! and the evidence it cites (log lines by their place in the run, spans by their trace). Pure: the hub gathers the
//! evidence and attaches the failing verdicts to the change.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::observe::{LogLine, SpanRec, is_error};
use crate::pii;
use crate::spec::Assertion;

/// The metric that counts a service's requests (increases per window); every request is counted by it.
pub const REQUESTS: &str = "http.server.requests";
/// The most evidence lines one verdict cites.
const CITES: usize = 5;

/// What a run observed: its start (for a warm-up), its log lines and spans in the order they arrived, and each metric's
/// windows as (window start in ms, value) where a counter's value is its increase.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evidence {
    pub started_ms: u64,
    pub logs: Vec<LogLine>,
    pub spans: Vec<SpanRec>,
    pub series: BTreeMap<String, Vec<(u64, f64)>>,
}

/// One step's result: whether it passed, what it found, and the evidence it cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub id: String,
    pub kind: String,
    pub passed: bool,
    pub detail: String,
    pub cites: Vec<String>,
}

/// Every assertion of a topology, checked over the evidence, in the order the topology lists them.
pub fn check(assertions: &[Assertion], evidence: &Evidence) -> Vec<Verdict> {
    assertions
        .iter()
        .map(|assertion| {
            let (passed, detail, cites) = evaluate(assertion, evidence);
            Verdict {
                id: assertion.id(),
                kind: assertion.kind().into(),
                passed,
                detail,
                cites,
            }
        })
        .collect()
}

/// The requests a run counted: the sum of the request counter's increases.
pub fn requests(evidence: &Evidence) -> f64 {
    evidence
        .series
        .get(REQUESTS)
        .map_or(0.0, |windows| windows.iter().map(|(_, v)| v).sum())
}

fn evaluate(assertion: &Assertion, ev: &Evidence) -> (bool, String, Vec<String>) {
    match assertion {
        Assertion::NoErrors { allow, .. } => no_errors(allow, ev),
        Assertion::EveryRequestTraced { .. } => every_request_traced(ev),
        Assertion::P95Under { route, ms, .. } => p95_under(route, *ms, ev),
        Assertion::HitRateAbove {
            hits,
            misses,
            floor_percent,
            after_secs,
            ..
        } => hit_rate_above(hits, misses, *floor_percent, *after_secs, ev),
        Assertion::NoPersonalData { .. } => no_personal_data(ev),
        Assertion::LogVolumePerRequest {
            max_lines_per_request,
            ..
        } => log_volume(*max_lines_per_request, ev),
        Assertion::AuditEvents { expected, .. } => audit_events(expected, ev),
    }
}

/// A log line as a citation: its place in the run, its service, its text (cut to a line).
fn cite(index: usize, line: &LogLine) -> String {
    let body: String = line.body.chars().take(200).collect();
    format!("log #{index} {} {}: {body}", line.service, line.severity)
}

fn no_errors(allow: &[String], ev: &Evidence) -> (bool, String, Vec<String>) {
    let errors: Vec<(usize, &LogLine)> = ev
        .logs
        .iter()
        .enumerate()
        .filter(|(_, line)| is_error(&line.severity))
        .filter(|(_, line)| !allow.iter().any(|a| line.body.contains(a.as_str())))
        .collect();
    let detail = if errors.is_empty() {
        format!(
            "no ERROR line outside the allowlist in {} log lines",
            ev.logs.len()
        )
    } else {
        format!(
            "{} ERROR line(s) outside the allowlist in {} log lines",
            errors.len(),
            ev.logs.len()
        )
    };
    let cites = errors
        .iter()
        .take(CITES)
        .map(|(i, l)| cite(*i, l))
        .collect();
    (errors.is_empty(), detail, cites)
}

fn every_request_traced(ev: &Evidence) -> (bool, String, Vec<String>) {
    let requests = requests(ev);
    let servers: Vec<&SpanRec> = ev.spans.iter().filter(|s| s.kind == "server").collect();
    let untraced: Vec<&SpanRec> = ev.spans.iter().filter(|s| s.trace_id.is_empty()).collect();
    let cites: Vec<String> = untraced
        .iter()
        .take(CITES)
        .map(|s| format!("span {} of {} has no trace id", s.name, s.service))
        .collect();
    if requests <= 0.0 {
        return (
            false,
            "no request was counted, so none can be shown traced".into(),
            cites,
        );
    }
    let passed = untraced.is_empty() && servers.len() as f64 >= requests;
    let detail = format!(
        "{requests:.0} requests counted, {} server spans, {} spans without a trace id",
        servers.len(),
        untraced.len()
    );
    (passed, detail, cites)
}

/// The nearest-rank 95th percentile of durations (None when there are none).
pub fn p95(durations: &mut [u64]) -> Option<u64> {
    if durations.is_empty() {
        return None;
    }
    durations.sort_unstable();
    let rank = (0.95 * durations.len() as f64).ceil() as usize;
    durations.get(rank.max(1) - 1).copied()
}

fn p95_under(route: &str, ms: u64, ev: &Evidence) -> (bool, String, Vec<String>) {
    let spans: Vec<&SpanRec> = ev
        .spans
        .iter()
        .filter(|s| s.kind == "server" && (route == "*" || s.name == route))
        .collect();
    let mut durations: Vec<u64> = spans.iter().map(|s| s.duration_ms()).collect();
    let Some(p) = p95(&mut durations) else {
        return (
            false,
            format!("no server span of {route} was traced"),
            Vec::new(),
        );
    };
    let mut slow: Vec<&&SpanRec> = spans.iter().filter(|s| s.duration_ms() >= ms).collect();
    slow.sort_by_key(|s| std::cmp::Reverse(s.duration_ms()));
    let cites = slow
        .iter()
        .take(CITES)
        .map(|s| {
            format!(
                "span {} trace {} took {} ms",
                s.name,
                s.trace_id,
                s.duration_ms()
            )
        })
        .collect();
    (
        p < ms,
        format!(
            "p95 of {route} is {p} ms over {} spans (under {ms} ms)",
            spans.len()
        ),
        cites,
    )
}

fn hit_rate_above(
    hits: &str,
    misses: &str,
    floor: u32,
    after_secs: u64,
    ev: &Evidence,
) -> (bool, String, Vec<String>) {
    let from = ev
        .started_ms
        .saturating_add(after_secs.saturating_mul(1000));
    let total_of = |metric: &str| -> f64 {
        ev.series.get(metric).map_or(0.0, |windows| {
            windows
                .iter()
                .filter(|(t, _)| *t >= from)
                .map(|(_, v)| v)
                .sum()
        })
    };
    let (h, m) = (total_of(hits), total_of(misses));
    if h + m <= 0.0 {
        return (
            false,
            format!("no cache lookups after {after_secs} s of warm-up"),
            Vec::new(),
        );
    }
    let rate = h / (h + m) * 100.0;
    (
        rate > f64::from(floor),
        format!("hit rate {rate:.1} % ({h:.0} hits, {m:.0} misses) against a floor of {floor} %"),
        Vec::new(),
    )
}

fn no_personal_data(ev: &Evidence) -> (bool, String, Vec<String>) {
    let mut cites = Vec::new();
    let mut found = 0usize;
    for (index, line) in ev.logs.iter().enumerate() {
        let kinds = pii::kinds_in(&line.body);
        if !kinds.is_empty() {
            found += 1;
            if cites.len() < CITES {
                cites.push(format!("{} in {}", kinds.join(", "), cite(index, line)));
            }
        }
    }
    let detail = if found == 0 {
        format!("no personal data pattern in {} log lines", ev.logs.len())
    } else {
        format!("{found} log line(s) hold personal data")
    };
    (found == 0, detail, cites)
}

fn log_volume(max: u64, ev: &Evidence) -> (bool, String, Vec<String>) {
    let requests = requests(ev);
    let lines = ev.logs.len() as f64;
    if requests <= 0.0 {
        return (
            false,
            "no request was counted, so the log volume per request is unknown".into(),
            Vec::new(),
        );
    }
    let per = lines / requests;
    (
        per <= max as f64,
        format!(
            "{per:.1} log lines per request ({lines:.0} lines, {requests:.0} requests), at most {max}"
        ),
        Vec::new(),
    )
}

fn audit_events(expected: &[String], ev: &Evidence) -> (bool, String, Vec<String>) {
    let present = |name: &str| {
        ev.logs.iter().any(|line| {
            line.attrs
                .iter()
                .any(|(key, value)| key == "event.name" && value == name)
        })
    };
    let missing: Vec<&String> = expected.iter().filter(|name| !present(name)).collect();
    let detail = if missing.is_empty() {
        format!("all {} expected audit events are present", expected.len())
    } else {
        format!(
            "missing audit event(s): {}",
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    (missing.is_empty(), detail, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::{Attrs, SpanRec};

    fn log(t_ms: u64, severity: &str, body: &str, attrs: Attrs) -> LogLine {
        LogLine {
            t_ms,
            service: "api-1".into(),
            severity: severity.into(),
            body: body.into(),
            trace_id: String::new(),
            span_id: String::new(),
            attrs,
        }
    }

    fn span(name: &str, kind: &str, trace: &str, ms: u64) -> SpanRec {
        SpanRec {
            trace_id: trace.into(),
            span_id: "s".into(),
            parent_id: String::new(),
            name: name.into(),
            service: "api-1".into(),
            kind: kind.into(),
            start_ms: 1_000,
            end_ms: 1_000 + ms,
            status: "ok".into(),
            attrs: Vec::new(),
        }
    }

    /// A run of `requests` requests: each traced by a server span of `ms` (the latencies given, cycled), with its logs.
    fn run(requests: u64, logs: Vec<LogLine>, latencies: &[u64]) -> Evidence {
        let spans = (0..requests)
            .map(|i| {
                let ms = latencies[i as usize % latencies.len()];
                span(
                    "GET /items",
                    "server",
                    "0af7651916cd43dd8448eb211c80319c",
                    ms,
                )
            })
            .collect();
        let mut series = BTreeMap::new();
        series.insert(REQUESTS.to_string(), vec![(1_000, requests as f64)]);
        Evidence {
            started_ms: 0,
            logs,
            spans,
            series,
        }
    }

    fn one(assertion: Assertion, ev: &Evidence) -> Verdict {
        check(std::slice::from_ref(&assertion), ev).remove(0)
    }

    fn no_errors() -> Assertion {
        Assertion::NoErrors {
            id: None,
            allow: vec!["expected".into()],
        }
    }

    #[test]
    fn a_seeded_error_fails_no_errors_and_cites_its_log_line() {
        let ev = run(
            1,
            vec![
                log(1, "INFO", "started", vec![]),
                log(2, "ERROR", "seeded failure in the cache", vec![]),
            ],
            &[5],
        );
        let verdict = one(no_errors(), &ev);
        assert!(!verdict.passed);
        assert_eq!(verdict.id, "no_errors");
        assert_eq!(
            verdict.cites,
            ["log #1 api-1 ERROR: seeded failure in the cache"]
        );
    }

    #[test]
    fn an_allowlisted_error_passes_no_errors() {
        let ev = run(
            1,
            vec![log(
                2,
                "ERROR",
                "expected: upstream timeout (retried)",
                vec![],
            )],
            &[5],
        );
        assert!(one(no_errors(), &ev).passed);
    }

    #[test]
    fn every_request_traced_needs_a_server_span_per_request_and_a_trace_id() {
        let mut ev = run(3, vec![], &[5]);
        assert!(one(Assertion::EveryRequestTraced { id: None }, &ev).passed);
        ev.spans.pop();
        assert!(!one(Assertion::EveryRequestTraced { id: None }, &ev).passed);
        ev.spans.push(span("GET /items", "server", "", 5));
        let verdict = one(Assertion::EveryRequestTraced { id: None }, &ev);
        assert!(!verdict.passed && verdict.cites.len() == 1);
        let none = Evidence::default();
        assert!(!one(Assertion::EveryRequestTraced { id: None }, &none).passed);
    }

    #[test]
    fn p95_is_the_nearest_rank_of_the_durations() {
        let mut durations: Vec<u64> = (1..=100).collect();
        assert_eq!(p95(&mut durations), Some(95));
        assert_eq!(p95(&mut []), None);
    }

    #[test]
    fn p95_under_passes_below_its_threshold_and_fails_above_it() {
        let quick = run(20, vec![], &[10, 12, 15]);
        let assertion = Assertion::P95Under {
            id: None,
            route: "GET /items".into(),
            ms: 100,
        };
        assert!(one(assertion.clone(), &quick).passed);
        let slow = run(20, vec![], &[10, 12, 150]);
        let verdict = one(assertion, &slow);
        assert!(!verdict.passed);
        assert!(verdict.cites[0].contains("took 150 ms"));
        let other_route = Assertion::P95Under {
            id: None,
            route: "POST /orders".into(),
            ms: 100,
        };
        assert!(!one(other_route, &quick).passed, "no span of that route");
    }

    fn hits_and_misses(hits: f64, misses: f64) -> Evidence {
        let mut series = BTreeMap::new();
        series.insert("cache.hits".into(), vec![(1_000, hits)]);
        series.insert("cache.misses".into(), vec![(1_000, misses)]);
        Evidence {
            started_ms: 0,
            series,
            ..Evidence::default()
        }
    }

    #[test]
    fn hit_rate_above_passes_over_its_floor_and_fails_under_it() {
        let assertion = Assertion::HitRateAbove {
            id: None,
            hits: "cache.hits".into(),
            misses: "cache.misses".into(),
            floor_percent: 80,
            after_secs: 0,
        };
        assert!(one(assertion.clone(), &hits_and_misses(90.0, 10.0)).passed);
        assert!(!one(assertion.clone(), &hits_and_misses(50.0, 50.0)).passed);
        assert!(
            !one(assertion, &hits_and_misses(0.0, 0.0)).passed,
            "no lookups at all"
        );
    }

    #[test]
    fn hit_rate_counts_only_after_the_warm_up() {
        let mut ev = hits_and_misses(0.0, 0.0);
        ev.series
            .insert("cache.misses".into(), vec![(1_000, 50.0), (9_000, 5.0)]);
        ev.series
            .insert("cache.hits".into(), vec![(1_000, 0.0), (9_000, 95.0)]);
        let after = Assertion::HitRateAbove {
            id: None,
            hits: "cache.hits".into(),
            misses: "cache.misses".into(),
            floor_percent: 80,
            after_secs: 5,
        };
        assert!(one(after, &ev).passed, "the cold start is left out");
    }

    #[test]
    fn personal_data_in_a_log_fails_and_a_clean_run_passes() {
        let assertion = Assertion::NoPersonalData { id: None };
        let dirty = run(
            1,
            vec![log(1, "INFO", "login for ana@example.com", vec![])],
            &[5],
        );
        let verdict = one(assertion.clone(), &dirty);
        assert!(!verdict.passed);
        assert!(verdict.cites[0].starts_with("email in log #0"));
        let clean = run(1, vec![log(1, "INFO", "login ok", vec![])], &[5]);
        assert!(one(assertion, &clean).passed);
    }

    #[test]
    fn log_volume_per_request_is_the_lines_over_the_requests() {
        let assertion = Assertion::LogVolumePerRequest {
            id: None,
            max_lines_per_request: 2,
        };
        let quiet = run(
            4,
            vec![log(1, "INFO", "a", vec![]), log(2, "INFO", "b", vec![])],
            &[5],
        );
        assert!(one(assertion.clone(), &quiet).passed);
        let chatty = run(
            1,
            vec![
                log(1, "INFO", "a", vec![]),
                log(2, "INFO", "b", vec![]),
                log(3, "INFO", "c", vec![]),
            ],
            &[5],
        );
        assert!(!one(assertion, &chatty).passed);
    }

    #[test]
    fn expected_audit_events_must_all_be_present() {
        let assertion = Assertion::AuditEvents {
            id: None,
            expected: vec!["login.ok".into(), "role.granted".into()],
        };
        let ev = run(
            1,
            vec![log(
                1,
                "INFO",
                "{}",
                vec![("event.name".into(), "login.ok".into())],
            )],
            &[5],
        );
        let verdict = one(assertion.clone(), &ev);
        assert!(!verdict.passed);
        assert!(verdict.detail.contains("role.granted"));
        let all = run(
            1,
            vec![
                log(
                    1,
                    "INFO",
                    "{}",
                    vec![("event.name".into(), "login.ok".into())],
                ),
                log(
                    2,
                    "INFO",
                    "{}",
                    vec![("event.name".into(), "role.granted".into())],
                ),
            ],
            &[5],
        );
        assert!(one(assertion, &all).passed);
    }

    #[test]
    fn a_topology_checks_its_steps_in_order_with_their_ids() {
        let assertions = vec![
            no_errors(),
            Assertion::EveryRequestTraced {
                id: Some("traced".into()),
            },
        ];
        let ev = run(1, vec![log(1, "ERROR", "boom", vec![])], &[5]);
        let verdicts = check(&assertions, &ev);
        assert_eq!(
            verdicts.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["no_errors", "traced"]
        );
        assert!(!verdicts[0].passed && verdicts[1].passed);
    }
}
