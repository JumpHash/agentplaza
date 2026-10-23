//! A lab run's performance against its release's baseline (PLAN 22.8, 22.11): the load and bench values the run recorded
//! in the metrics store (its collector's OTLP gauges, `apz-load`'s summary among them), the values of the last qualified
//! stable release's run, and the project's limits (`performance_limits` in its settings). Each comparison that fails is
//! a finding of kind `performance` on the run's change, its proof the numbers.
//!
//! Routes:
//!   GET /v1/test-lab/runs/{id}/performance   the run's comparisons with its baseline (`baseline` null when it has none)

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_topology::compare::{self, Comparison, Limit, Metric};
use apz_topology::load::METRICS;
use serde_json::{Value as Json, json};

use crate::lab_observe;
use crate::metrics::Metrics;
use crate::project_settings::PerfLimit;
use crate::releases::entity::{Release, ReleaseState};
use crate::review_graph::{By, Finding, Reviews, Verdict as FindingVerdict};
use crate::state::App;
use crate::test_lab::{LabRun, Store};

/// The finding kind a failing comparison is filed as: the change tree's performance node reads it (`prf`).
pub const KIND: &str = "performance";

/// The project's limits as the comparison takes them.
pub fn limits_of(rows: &[PerfLimit]) -> Vec<Limit> {
    rows.iter()
        .map(|row| Limit {
            metric: row.metric.clone(),
            max: f64::from(row.max_tenths) / 10.0,
        })
        .collect()
}

/// The limits and the releases a run's comparison reads: its project's.
pub fn context(app: &App, project: &str) -> (Vec<Release>, Vec<Limit>) {
    let limits = limits_of(&app.project_settings.get(project).performance_limits);
    (app.releases.list(project), limits)
}

/// The metric names a comparison reads: the load's, and each exact name the project's limits name.
pub fn names(limits: &[Limit]) -> Vec<String> {
    let mut names: Vec<String> = METRICS.iter().map(|m| (*m).to_string()).collect();
    names.extend(
        limits
            .iter()
            .filter(|l| !l.metric.ends_with('*'))
            .map(|l| l.metric.clone()),
    );
    names.sort();
    names.dedup();
    names
}

/// The values a run recorded for `names`: each series' mean over its windows, for the names it has values for.
pub fn values(metrics: &Metrics, run: &str, names: &[String]) -> Vec<Metric> {
    names
        .iter()
        .filter_map(|name| {
            let (sum, count) = lab_observe::windows(metrics, run, name, 0, i64::MAX as u64)
                .into_iter()
                .flat_map(|(_, rows)| rows.into_iter())
                .fold((0.0_f64, 0_u64), |(sum, count), (_, agg)| {
                    (sum + agg.sum, count + agg.count)
                });
            (count > 0).then(|| Metric {
                name: name.clone(),
                value: sum / count as f64,
            })
        })
        .collect()
}

/// The baseline a run is compared with: the last qualified stable release (other than the run's own), and the newest
/// done run of that release that recorded values.
#[derive(Debug, Clone, PartialEq)]
pub struct Baseline {
    pub release: String,
    pub run: String,
    pub metrics: Vec<Metric>,
}

/// The comparison of a run with its baseline: the baseline, and each metric's comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub baseline: Baseline,
    pub comparisons: Vec<Comparison>,
}

fn qualified(release: &Release) -> bool {
    release.channel == "stable"
        && matches!(
            release.state,
            ReleaseState::Qualified | ReleaseState::Published | ReleaseState::Promoted
        )
}

/// The baseline of `run`, from the project's `releases` (newest first), or None when the run qualifies no release or no
/// earlier stable release has recorded values for `names`.
pub fn baseline_of(
    lab: &Store,
    metrics: &Metrics,
    releases: &[Release],
    run: &LabRun,
    names: &[String],
) -> Option<Baseline> {
    if run.release.is_empty() {
        return None;
    }
    let runs = lab.runs();
    releases
        .iter()
        .filter(|r| r.project == run.project && r.id != run.release && qualified(r))
        .find_map(|release| {
            let mut earlier: Vec<&LabRun> = runs
                .iter()
                .filter(|r| r.release == release.id && r.state == "done" && r.id != run.id)
                .collect();
            earlier.sort_by_key(|r| std::cmp::Reverse(r.enqueued_ms));
            earlier.into_iter().find_map(|r| {
                let values = values(metrics, &r.id, names);
                (!values.is_empty()).then(|| Baseline {
                    release: release.id.clone(),
                    run: r.id.clone(),
                    metrics: values,
                })
            })
        })
}

/// The comparison of `run` with its baseline under `limits`; None when there is no baseline.
pub fn compare_run(
    lab: &Store,
    metrics: &Metrics,
    releases: &[Release],
    run: &LabRun,
    limits: &[Limit],
) -> Option<Outcome> {
    let names = names(limits);
    let baseline = baseline_of(lab, metrics, releases, run, &names)?;
    let candidate = values(metrics, &run.id, &names);
    let comparisons = compare::compare(&baseline.metrics, &candidate, limits);
    Some(Outcome {
        baseline,
        comparisons,
    })
}

fn number(value: f64) -> String {
    format!("{value:.2}")
}

/// One comparison as people read it: `load.latency.p50_ms 100.00 -> 120.00 (+20.0 percent, limit 15 percent)`.
pub fn describe(c: &Comparison) -> String {
    format!(
        "{} {} -> {} ({:+.1} {}, limit {} {})",
        c.metric,
        number(c.baseline),
        number(c.candidate),
        c.change,
        c.unit,
        c.limit,
        c.unit
    )
}

/// The finding a failing comparison becomes on the run's change (PLAN 22.8): a confirmed `performance` finding whose
/// proof is the numbers. Empty when the run names no change; the change's existing review run must be there.
pub fn file_finding(reviews: &Reviews, run: &LabRun, outcome: &Outcome, c: &Comparison) -> String {
    if run.change.is_empty() {
        return String::new();
    }
    let by = By {
        seat: "test-lab".into(),
        model: "comparison".into(),
        session: run.id.clone(),
    };
    let finding = Finding {
        symbol: if run.topology.is_empty() {
            String::new()
        } else {
            format!("{}/{}.toml", apz_topology::spec::DIR, run.topology)
        },
        lineage: None,
        text: format!(
            "{} regressed against {}: {}",
            c.metric,
            outcome.baseline.release,
            describe(c)
        ),
        severity: "major".into(),
        node: String::new(),
        round: 0,
        id: String::new(),
        scenario: format!(
            "lab run {} compared with release {} (run {}) under the project's performance limits",
            run.id, outcome.baseline.release, outcome.baseline.run
        ),
        kind: KIND.into(),
        verdict: FindingVerdict::Confirmed,
        proof: format!(
            "lab run {} against {} (run {}):\n{}",
            run.id,
            outcome.baseline.release,
            outcome.baseline.run,
            describe(c)
        ),
        reason: String::new(),
        by: Some(by.clone()),
        verified_by: Some(by),
        contest: String::new(),
        ruling: String::new(),
        ruling_reason: String::new(),
    };
    reviews
        .attach_finding(&run.change, finding)
        .map_or_else(String::new, |(review, id)| format!("{review}/{id}"))
}

fn view(run: &LabRun, limits: &[Limit], outcome: Option<&Outcome>) -> Json {
    let comparisons = outcome
        .map(|o| o.comparisons.as_slice())
        .unwrap_or_default();
    json!({
        "run": run.id,
        "release": run.release,
        "baseline": outcome.map(|o| json!({"release": o.baseline.release, "run": o.baseline.run})),
        "limits": limits.iter().map(|l| json!({"metric": l.metric, "max": l.max})).collect::<Vec<_>>(),
        "comparisons": comparisons,
        "failed": compare::failures(comparisons).len(),
    })
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/test-lab/runs/{id}/performance", performance_route)
        .doc(RouteDoc::new("A lab run's performance against the last qualified stable release's run: each metric's values, its change and whether it passed the project's limits (baseline null when there is none)").errors(&[404]))
}

fn performance_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let Some(run) = app
        .test_lab
        .runs()
        .into_iter()
        .find(|r| r.id == p.get("id"))
    else {
        return Response::error(404, "no such run");
    };
    let (releases, limits) = context(app, &run.project);
    let outcome = compare_run(&app.test_lab, &app.metrics, &releases, &run, &limits);
    Response::json(200, &view(&run, &limits, outcome.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_are_the_loads_and_each_exact_limit() {
        let limits = [
            Limit {
                metric: "bench.parse.ns".into(),
                max: 5.0,
            },
            Limit {
                metric: "bench.*".into(),
                max: 5.0,
            },
        ];
        let names = names(&limits);
        assert!(names.contains(&"load.latency.p50_ms".to_string()));
        assert!(names.contains(&"bench.parse.ns".to_string()));
        assert!(!names.iter().any(|n| n.ends_with('*')));
        assert_eq!(names.len(), 7);
    }

    #[test]
    fn a_comparison_reads_as_numbers() {
        let c = Comparison {
            metric: "load.latency.p50_ms".into(),
            baseline: 100.0,
            candidate: 120.0,
            change: 20.0,
            limit: 15.0,
            passed: false,
            unit: "percent".into(),
        };
        assert_eq!(
            describe(&c),
            "load.latency.p50_ms 100.00 -> 120.00 (+20.0 percent, limit 15 percent)"
        );
    }

    fn release(id: &str, state: &str) -> Release {
        serde_json::from_value(serde_json::json!({
            "id": id, "project": "p", "repo": "r", "version": "1.0.0", "channel": "stable",
            "lane": "main", "from": "abc", "state": state, "notes": "", "changes": [],
            "artifacts": [], "verdicts": [], "promotions": [], "sync": [],
            "created_ms": 0, "updated_ms": 0,
        }))
        .expect("a release")
    }

    /// A lab run of project `p` that qualifies `release` and is changed by `change`.
    fn lab_run(lab: &Store, id: &str, release: &str, change: &str, token: &str) {
        lab.inner.lock().unwrap().runs.push(LabRun {
            id: id.into(),
            project: "p".into(),
            channel: "stable".into(),
            state: "running".into(),
            cpus: 1,
            memory_mib: 512,
            needs: Vec::new(),
            reservations: Vec::new(),
            enqueued_ms: 1_700_000_000_000,
            started_ms: Some(1_700_000_000_000),
            ended_ms: None,
            outcome: None,
            change: change.into(),
            topology: "fake-run".into(),
            release: release.into(),
            token_sha: crate::lab_observe::token_hash(token),
            observed: 0,
            dropped: 0,
        });
    }

    /// Runs the built-in generator against `server` for a short steady profile, and records its summary at run `id`.
    fn load_into(
        lab: &Store,
        metrics: &Metrics,
        server: &apz_topology::slowserve::SlowServer,
        id: &str,
        token: &str,
    ) {
        let scratch = tempfile::tempdir().unwrap();
        let profile = apz_topology::load::parse("steady-20rps", Some(3_000)).unwrap();
        let summary = apz_topology::load::run_with(
            &apz_topology::load::Tool::Builtin,
            &server.url(),
            &profile,
            scratch.path(),
        )
        .expect("the generator runs");
        let body = apz_topology::load::otlp(&summary, crate::lab_observe::tests::NOW);
        let accepted = crate::lab_observe::ingest(
            lab,
            metrics,
            id,
            token,
            "metrics",
            body.to_string().as_bytes(),
            crate::lab_observe::tests::NOW,
        )
        .expect("the summary is recorded at the run");
        assert_eq!(accepted.records, 6);
        for run in &mut lab.inner.lock().unwrap().runs {
            if run.id == id {
                run.state = "done".into();
            }
        }
    }

    const TOPOLOGY: &str = "name = \"fake-run\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\n";

    #[test]
    fn a_seeded_slowdown_fails_against_the_last_qualified_release_and_files_a_performance_finding()
    {
        let (_files, metrics) = crate::lab_observe::tests::metrics_in("lab-performance-slow");
        let dir = tempfile::tempdir().unwrap();
        let reviews = crate::lab_observe::tests::reviews_in(&dir);
        reviews.put_run(crate::lab_observe::tests::review_of("c1"));
        let lab = Store::default();
        let server = apz_topology::slowserve::SlowServer::start(100).unwrap();

        // The previous release, qualified at 100 ms: its run records its load.
        lab_run(&lab, "q1", "rel-1", "", "t1");
        load_into(&lab, &metrics, &server, "q1", "t1");
        // The candidate, seeded 20 % slower.
        server.set_delay(120);
        lab_run(&lab, "q2", "rel-2", "c1", "t2");
        load_into(&lab, &metrics, &server, "q2", "t2");

        let releases = [release("rel-2", "candidate"), release("rel-1", "qualified")];
        let topology = apz_topology::spec::parse(TOPOLOGY).unwrap();
        let verdicts =
            crate::lab_observe::assess(&lab, &metrics, &reviews, "q2", &topology, &releases, &[])
                .unwrap();
        let p50 = verdicts
            .iter()
            .find(|v| v.id == "perf:load.latency.p50_ms")
            .expect("the median is compared");
        assert!(!p50.passed, "{p50:?}");
        assert_eq!(p50.kind, KIND);
        assert!(p50.detail.contains("->"), "{}", p50.detail);
        assert!(
            p50.cites.iter().any(|c| c.contains("rel-1")),
            "{:?}",
            p50.cites
        );

        // Filed on the change's review tree, its proof the numbers, of kind performance.
        let (review, id) = p50.finding.split_once('/').expect("the finding is filed");
        let run = reviews.run(review).unwrap();
        let finding = run.findings().find(|f| f.id == id).expect("on the run");
        assert_eq!(finding.kind, KIND);
        assert_eq!(finding.verdict, FindingVerdict::Confirmed);
        assert!(
            finding.proof.contains("load.latency.p50_ms"),
            "{}",
            finding.proof
        );
        assert!(finding.proof.contains("(+"), "{}", finding.proof);

        // The read route's view agrees: the run has a baseline and a failed comparison.
        let candidate = lab.runs().into_iter().find(|r| r.id == "q2").unwrap();
        let outcome = compare_run(&lab, &metrics, &releases, &candidate, &[]).unwrap();
        assert_eq!(outcome.baseline.release, "rel-1");
        assert!(!compare::failures(&outcome.comparisons).is_empty());
    }

    #[test]
    fn an_unchanged_latency_files_no_performance_finding() {
        let (_files, metrics) = crate::lab_observe::tests::metrics_in("lab-performance-same");
        let dir = tempfile::tempdir().unwrap();
        let reviews = crate::lab_observe::tests::reviews_in(&dir);
        reviews.put_run(crate::lab_observe::tests::review_of("c1"));
        let lab = Store::default();
        let server = apz_topology::slowserve::SlowServer::start(100).unwrap();
        lab_run(&lab, "q1", "rel-1", "", "t1");
        load_into(&lab, &metrics, &server, "q1", "t1");
        lab_run(&lab, "q2", "rel-2", "c1", "t2");
        load_into(&lab, &metrics, &server, "q2", "t2");
        let releases = [release("rel-2", "candidate"), release("rel-1", "qualified")];
        let topology = apz_topology::spec::parse(TOPOLOGY).unwrap();
        let verdicts =
            crate::lab_observe::assess(&lab, &metrics, &reviews, "q2", &topology, &releases, &[])
                .unwrap();
        assert!(
            verdicts
                .iter()
                .filter(|v| v.id.starts_with("perf:"))
                .all(|v| v.passed),
            "{verdicts:?}"
        );
        assert!(verdicts.iter().all(|v| v.finding.is_empty()));
        assert_eq!(reviews.run("rv-c1").unwrap().findings().count(), 0);
    }

    #[test]
    fn a_run_with_no_qualified_baseline_has_no_comparison() {
        let (_files, metrics) = crate::lab_observe::tests::metrics_in("lab-performance-none");
        let lab = Store::default();
        lab_run(&lab, "q1", "rel-2", "c1", "t1");
        let run = lab.runs().remove(0);
        // Only the candidate's own draft exists: nothing qualified to compare with.
        let releases = [release("rel-2", "candidate")];
        assert!(compare_run(&lab, &metrics, &releases, &run, &[]).is_none());
        // A run that qualifies no release has no baseline either.
        lab_run(&lab, "q3", "", "", "t3");
        let none = lab.runs().into_iter().find(|r| r.id == "q3").unwrap();
        assert!(
            compare_run(&lab, &metrics, &[release("rel-1", "qualified")], &none, &[]).is_none()
        );
    }
}
