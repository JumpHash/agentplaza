//! The performance gate end to end over the wire (PLAN 22.8): the built-in generator runs a short profile against a
//! local server whose latency the test sets, for the previous release and again for the candidate, and the comparison
//! fails a seeded 20 % slowdown and passes an unchanged latency.

use apz_topology::compare::{self, Limit, Metric};
use apz_topology::load::{self, Summary, Tool};
use apz_topology::slowserve::SlowServer;

/// The metrics of a summary, as the comparison reads them.
fn metrics_of(summary: &Summary) -> Vec<Metric> {
    load::metric_points(summary)
        .iter()
        .map(|(name, value)| Metric {
            name: (*name).into(),
            value: *value,
        })
        .collect()
}

/// A short steady profile against `server`, its summary.
fn short_run(server: &SlowServer, scratch: &std::path::Path) -> Summary {
    let profile = load::parse("steady-20rps", Some(3_000)).unwrap();
    load::run_with(&Tool::Builtin, &server.url(), &profile, scratch)
        .expect("the built-in generator runs")
}

#[test]
fn a_seeded_twenty_percent_slowdown_fails_against_the_previous_release() {
    let scratch = tempfile::tempdir().unwrap();
    let server = SlowServer::start(100).unwrap();

    // The previous release, measured at its own latency.
    let baseline = short_run(&server, scratch.path());
    assert_eq!(baseline.tool, "builtin");
    assert!(baseline.requests >= 50, "{baseline:?}");
    assert_eq!(baseline.errors, 0, "{baseline:?}");
    assert!(
        baseline.p50_ms >= 100.0,
        "the stand-in answers after its delay: {baseline:?}"
    );

    // The candidate, seeded 20 % slower.
    server.set_delay(120);
    let candidate = short_run(&server, scratch.path());
    let comparisons = compare::compare(&metrics_of(&baseline), &metrics_of(&candidate), &[]);
    let failed: Vec<&str> = compare::failures(&comparisons)
        .iter()
        .map(|c| c.metric.as_str())
        .collect();
    assert!(failed.contains(&"load.latency.p50_ms"), "{comparisons:?}");
    assert!(failed.contains(&"load.latency.p95_ms"), "{comparisons:?}");
    let p50 = comparisons
        .iter()
        .find(|c| c.metric == "load.latency.p50_ms")
        .unwrap();
    assert!(p50.change > 15.0, "the slowdown is about 20 %: {p50:?}");
}

#[test]
fn an_unchanged_latency_passes_against_the_previous_release() {
    let scratch = tempfile::tempdir().unwrap();
    let server = SlowServer::start(100).unwrap();
    let baseline = short_run(&server, scratch.path());
    let candidate = short_run(&server, scratch.path());
    let comparisons = compare::compare(&metrics_of(&baseline), &metrics_of(&candidate), &[]);
    let failed: Vec<&str> = compare::failures(&comparisons)
        .iter()
        .map(|c| c.metric.as_str())
        .collect();
    assert!(
        failed.is_empty(),
        "an unchanged latency is not a regression: {comparisons:?}"
    );
}

#[test]
fn a_project_limit_can_let_the_slowdown_through() {
    let scratch = tempfile::tempdir().unwrap();
    let server = SlowServer::start(100).unwrap();
    let baseline = short_run(&server, scratch.path());
    server.set_delay(120);
    let candidate = short_run(&server, scratch.path());
    let generous = [Limit {
        metric: "load.latency.*".into(),
        max: 60.0,
    }];
    let comparisons = compare::compare(&metrics_of(&baseline), &metrics_of(&candidate), &generous);
    let failed = compare::failures(&comparisons);
    assert!(
        failed.iter().all(|c| !c.metric.starts_with("load.latency")),
        "a 60 % limit lets a 20 % slowdown pass: {failed:?}"
    );
}
