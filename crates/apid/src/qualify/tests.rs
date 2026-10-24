//! The qualification end to end over a real Weft (the release tests' harness) with fake stage results: the lab is a fake
//! that records the checks, tests and numbers its runs produced, and the models of an acceptance are scripted.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

use serde_json::json;

use crate::releases::entity::ReleaseState;
use crate::releases::flow::{self, Cut, Deps};
use crate::releases::tests::{FakeBuilder, World, cut, lab};

use super::accept::{Answer, Judge};
use super::engine::{self, Setup};
use super::plan::Stage;
use super::report;
use super::result::{Metric, StageResult, State};
use super::stages::{self, Check, Cx, Lab, LabState};
use super::store::{FixTask, LabTest, QualifyRun, Store};

/// A lab that records what the test gives its runs: their states, checks, numbers, and the re-runs its runner answers.
#[derive(Default)]
struct FakeLab {
    topologies: Vec<String>,
    states: RefCell<BTreeMap<String, LabState>>,
    checks: RefCell<BTreeMap<String, Vec<Check>>>,
    metrics: RefCell<BTreeMap<String, Vec<Metric>>>,
    reruns: RefCell<VecDeque<bool>>,
    next: RefCell<u32>,
}

impl FakeLab {
    fn with(topologies: &[&str]) -> FakeLab {
        FakeLab {
            topologies: topologies.iter().map(|t| t.to_string()).collect(),
            ..FakeLab::default()
        }
    }

    /// Ends a run with `outcome`.
    fn finish(&self, run: &str, outcome: &str) {
        self.states.borrow_mut().insert(
            run.to_string(),
            LabState {
                state: "done".into(),
                outcome: Some(outcome.into()),
                enqueued_ms: 0,
                started_ms: Some(10),
                ended_ms: Some(70_010),
            },
        );
    }

    fn check(&self, run: &str, kind: &str, id: &str, passed: bool) {
        self.checks
            .borrow_mut()
            .entry(run.to_string())
            .or_default()
            .push(Check {
                run: run.to_string(),
                kind: kind.to_string(),
                id: id.to_string(),
                passed,
                detail: format!("{id} {}", if passed { "held" } else { "failed" }),
            });
    }

    fn measure(&self, run: &str, name: &str, value: f64) {
        self.metrics
            .borrow_mut()
            .entry(run.to_string())
            .or_default()
            .push(Metric {
                name: name.to_string(),
                value,
            });
    }
}

impl Lab for FakeLab {
    fn topologies(&self, _project: &str) -> Vec<String> {
        self.topologies.clone()
    }

    fn enqueue(
        &self,
        _project: &str,
        _release: &crate::releases::entity::Release,
        _topology: &str,
    ) -> Result<String, String> {
        *self.next.borrow_mut() += 1;
        let run = format!("lab-{}", self.next.borrow());
        self.states.borrow_mut().insert(
            run.clone(),
            LabState {
                state: "queued".into(),
                outcome: None,
                enqueued_ms: 0,
                started_ms: None,
                ended_ms: None,
            },
        );
        Ok(run)
    }

    fn state(&self, run: &str) -> Option<LabState> {
        self.states.borrow().get(run).cloned()
    }

    fn checks(&self, run: &str) -> Vec<Check> {
        self.checks.borrow().get(run).cloned().unwrap_or_default()
    }

    fn metrics(&self, run: &str) -> Vec<Metric> {
        self.metrics.borrow().get(run).cloned().unwrap_or_default()
    }

    fn rerun(&self, _run: &str, _test: &str, _attempt: u32) -> Result<bool, String> {
        self.reruns
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| "no re-run was scripted".to_string())
    }
}

/// The compatibility part a test gives stage 9 (production is the adapter that reports not run until 10.10).
fn compat_passes(_release: &crate::releases::entity::Release, now: u64) -> StageResult {
    StageResult::new(Stage::Ux, State::Passed, "", now)
}

/// A judge that answers the planner's proposal and the second model's agreement from fixed answers.
struct Scripted {
    second_agrees: bool,
}

impl Judge for Scripted {
    fn ask(&self, _model: &str, role: &str, _body: &str) -> Result<Answer, String> {
        let value = if role.contains("planner") {
            json!({"accept": true, "reason": "the scanner flags a fixture, not the product"})
        } else {
            json!({"agree": self.second_agrees, "reason": "agreed on the fixture"})
        };
        Ok(Answer { value, tokens: 50 })
    }
}

fn setup() -> Setup {
    Setup {
        override_: None,
        has_topologies: true,
        risky: false,
        soak_asked: true,
        planner: String::new(),
        second: String::new(),
    }
}

fn cx<'a>(
    d: &'a Deps<'a>,
    lab: &'a FakeLab,
    store: &'a Store,
    release: &'a crate::releases::entity::Release,
    now: u64,
) -> Cx<'a> {
    Cx {
        deps: d,
        lab,
        store,
        release,
        project: "app",
        now,
        compat: &compat_passes,
    }
}

/// Gives a qualification's runs everything every stage judges, as a healthy run records it.
fn healthy_run(lab: &FakeLab, store: &Store, run: &str, latency: f64) {
    lab.finish(run, "passed");
    for kind in [
        "data",
        "cache",
        "messaging",
        "observability",
        "resilience",
        "security",
        "visual",
        "soak",
    ] {
        lab.check(run, kind, &format!("{kind}-check"), true);
    }
    lab.measure(run, "latency.p95", latency);
    store.record_lab_tests(
        run,
        &[LabTest {
            run: run.to_string(),
            name: "cart::pay".into(),
            passed: true,
        }],
    );
}

/// Cuts a candidate of `version` (on `channel`), with the given changes.
fn candidate(d: &Deps, version: &str, channel: &str) -> crate::releases::entity::Release {
    let cut = Cut {
        channel: Some(channel.into()),
        ..cut(version)
    };
    let drafted = flow::draft(d, "app", "app", &cut).unwrap();
    flow::candidate(d, &drafted.id, &FakeBuilder, Some(1)).unwrap()
}

#[test]
fn a_stable_candidate_goes_from_draft_to_promoted_with_every_stage_in_its_report() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-stable");
    let world = World::new(client, "qualify-stable");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["web"]);

    // The first release qualifies and goes out: its numbers are the baseline of the second.
    let first = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &first, 1);
    let q1 = engine::start(&lab_cx, None, &setup()).unwrap();
    assert_eq!(q1.lab_runs.len(), 1, "one run per declared topology");
    healthy_run(&fake, &store, &q1.lab_runs[0], 100.0);
    let lab_cx = cx(&d, &fake, &store, &first, 2);
    let q1 = engine::advance(&lab_cx, None, &q1.id).unwrap();
    assert_eq!(q1.verdict, "passed", "{q1:?}");
    assert_eq!(
        flow::get(&d, &first.id).unwrap().state,
        ReleaseState::Qualified
    );
    flow::publish(&d, &first.id).unwrap();
    flow::promote(&d, &first.id, "production").unwrap();

    // The second is slower by a fifth: the report shows the delta, the stages with their evidence, and the review graph.
    let second = candidate(&d, "v0.2.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &second, 3);
    let q2 = engine::start(&lab_cx, None, &setup()).unwrap();
    healthy_run(&fake, &store, &q2.lab_runs[0], 120.0);
    let lab_cx = cx(&d, &fake, &store, &second, 4);
    let q2 = engine::advance(&lab_cx, None, &q2.id).unwrap();
    assert_eq!(q2.verdict, "passed", "{q2:?}");
    flow::publish(&d, &second.id).unwrap();
    let promoted = flow::promote(&d, &second.id, "production").unwrap();
    assert_eq!(promoted.state, ReleaseState::Promoted);

    let previous_release = flow::get(&d, &first.id).unwrap();
    let release = flow::get(&d, &second.id).unwrap();
    let histories = store.histories("app");
    let report = report::report(&report::Input {
        release: &release,
        run: &store.run(&q2.id).unwrap(),
        previous: Some(&store.run(&q1.id).unwrap()),
        previous_release: Some(&previous_release),
        acceptances: &[],
        histories: &histories,
        lab_minutes: 1,
    });
    let stages = report["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 11, "every stage is in the report");
    for stage in stages {
        assert_eq!(stage["state"], "passed", "{stage}");
    }
    let performance = stages.iter().find(|s| s["key"] == "performance").unwrap();
    assert!(
        !performance["evidence"].as_array().unwrap().is_empty(),
        "evidence links"
    );
    let delta = report["deltas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "latency.p95")
        .unwrap();
    assert_eq!(delta["change_pct"], 20.0);
    assert_eq!(report["findings"]["new"], json!([]));
    assert_eq!(report["compared_with"]["version"], "v0.1.0");
    assert_eq!(report["cost"]["lab_minutes"], 1);

    // The release's own review graph: one release-level node per stage that qualification judged.
    let review = d
        .reviews
        .run(&q2.review_run)
        .expect("the release's review run");
    assert_eq!(review.purpose, "release");
    assert_eq!(review.nodes.len(), 10);
}

#[test]
fn a_nightly_candidate_runs_its_stages_and_shows_the_others_as_not_in_its_depth() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-nightly");
    let world = World::new(client, "qualify-nightly");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["web"]);
    let release = candidate(&d, "v0.1.0", "nightly");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    assert_eq!(
        q.depth,
        vec![
            "verify",
            "topologies",
            "functional",
            "data",
            "observe",
            "performance",
            "verdict"
        ]
    );
    healthy_run(&fake, &store, &q.lab_runs[0], 100.0);
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    assert_eq!(
        q.verdict, "passed",
        "a nightly passes without the stages it does not run: {q:?}"
    );
    let histories = store.histories("app");
    let report = report::report(&report::Input {
        release: &flow::get(&d, &release.id).unwrap(),
        run: &q,
        previous: None,
        previous_release: None,
        acceptances: &[],
        histories: &histories,
        lab_minutes: 0,
    });
    let ux = report["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "ux")
        .unwrap()
        .clone();
    assert_eq!(ux["state"], "not_in_depth", "{ux}");
    assert_eq!(ux["reason"], "not in the nightly depth");
}

#[test]
fn a_stage_that_cannot_run_is_not_run_and_the_release_does_not_pass_on_it() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-not-run");
    let world = World::new(client, "qualify-not-run");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["web"]);
    let release = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    // Every check is recorded except the performance one: nothing measured the numbers.
    healthy_run(&fake, &store, &q.lab_runs[0], 100.0);
    fake.metrics.borrow_mut().clear();
    fake.checks
        .borrow_mut()
        .get_mut(&q.lab_runs[0])
        .unwrap()
        .retain(|c| c.kind != "performance");
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    let performance = q
        .result(Stage::Performance)
        .expect("performance has a result");
    assert_eq!(performance.state, State::NotRun, "{performance:?}");
    assert_eq!(
        q.verdict, "incomplete",
        "a not-run stage never passes the release"
    );
    assert!(
        q.reasons.iter().any(|r| r.contains("performance: not run")),
        "{:?}",
        q.reasons
    );
    assert_eq!(
        flow::get(&d, &release.id).unwrap().state,
        ReleaseState::Qualifying,
        "nothing moved"
    );
}

#[test]
fn a_failed_test_is_rerun_alone_up_to_twice_and_a_pass_on_a_rerun_is_a_flake() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-rerun");
    let world = World::new(client, "qualify-rerun");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["web"]);
    let release = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    let run = q.lab_runs[0].clone();
    fake.finish(&run, "passed");
    store.record_lab_tests(
        &run,
        &[LabTest {
            run: run.clone(),
            name: "cart::pay".into(),
            passed: false,
        }],
    );
    fake.reruns.borrow_mut().extend([false, true]);
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    let result = q.result(Stage::Functional).expect("functional was judged");
    assert_eq!(result.state, State::Passed, "{result:?}");
    assert!(
        fake.reruns.borrow().is_empty(),
        "exactly the two re-runs were asked"
    );
    let flaked = result
        .metrics
        .iter()
        .find(|m| m.name == "tests.flaked")
        .unwrap();
    assert_eq!(flaked.value, 1.0);

    // A test that fails all three attempts fails the stage, and the runner was asked twice.
    fake.reruns.borrow_mut().extend([false, false]);
    store.record_lab_tests(
        &run,
        &[LabTest {
            run: run.clone(),
            name: "cart::refund".into(),
            passed: false,
        }],
    );
    let q = store.run(&q.id).unwrap();
    let result = stages::poll(Stage::Functional, &q, &lab_cx)
        .unwrap()
        .unwrap();
    assert_eq!(result.state, State::Failed);
    assert!(
        result.findings.iter().any(|f| f.id == "test:cart::refund"),
        "{result:?}"
    );
}

#[test]
fn a_flaky_test_is_quarantined_after_three_flaky_runs_in_thirty_days_and_gets_a_fix_task() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-quarantine");
    let world = World::new(client, "qualify-quarantine");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["web"]);
    let release = candidate(&d, "v0.1.0", "stable");
    let day = 86_400_000u64;
    let mut quarantined_on = None;
    for n in 1..=3u64 {
        let run = format!("flaky-{n}");
        fake.finish(&run, "passed");
        store.record_lab_tests(
            &run,
            &[LabTest {
                run: run.clone(),
                name: "cart::pay".into(),
                passed: false,
            }],
        );
        fake.reruns.borrow_mut().push_back(true);
        let mut q = QualifyRun {
            lab_runs: vec![run.clone()],
            ..blank(&format!("q{n}"), &release.id)
        };
        q.state = "running".into();
        let lab_cx = cx(&d, &fake, &store, &release, n * day);
        let result = stages::poll(Stage::Functional, &q, &lab_cx)
            .unwrap()
            .unwrap();
        if result
            .findings
            .iter()
            .any(|f| f.id == "quarantine:cart::pay")
        {
            quarantined_on = Some(n);
            // The engine files the fix task of a quarantine when it records the stage's result.
            store.put_run(q.clone());
            engine::record(&lab_cx, &q.id, result);
            let filed = store.run(&q.id).unwrap();
            assert!(
                filed
                    .tasks
                    .iter()
                    .any(|t: &FixTask| t.goal.contains("cart::pay")),
                "{:?}",
                filed.tasks
            );
        }
    }
    assert_eq!(
        quarantined_on,
        Some(3),
        "quarantined on the third flaky run, not before"
    );
    let history = store.history("app", "cart::pay").unwrap();
    assert!(history.quarantined());
    let histories = store.histories("app");
    let rep = report::report(&report::Input {
        release: &flow::get(&d, &release.id).unwrap(),
        run: &blank("qx", &release.id),
        previous: None,
        previous_release: None,
        acceptances: &[],
        histories: &histories,
        lab_minutes: 0,
    });
    assert_eq!(
        rep["quarantine"][0]["test"], "cart::pay",
        "the quarantine is listed in the report"
    );

    // Fixed with a reason, it leaves the quarantine.
    store.update_history("app", "cart::pay", |h| {
        h.fixed(40 * day, "the retry was missing")
    });
    assert!(!store.history("app", "cart::pay").unwrap().quarantined());
}

#[test]
fn a_failure_is_accepted_only_when_the_second_model_agrees_and_otherwise_fails_the_release() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-accept");
    let world = World::new(client, "qualify-accept");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["web"]);
    let accepting = candidate(&d, "v0.1.0", "stable");
    let refused = candidate(&d, "v0.2.0", "stable");
    let planned = Setup {
        planner: "anthropic/planner".into(),
        second: "openai/second".into(),
        ..setup()
    };

    // The security scan flags a fixture: the planner proposes, the second model agrees, the failure is accepted.
    let lab_cx = cx(&d, &fake, &store, &accepting, 1);
    let q = engine::start(
        &lab_cx,
        Some(&Scripted {
            second_agrees: true,
        }),
        &planned,
    )
    .unwrap();
    healthy_run(&fake, &store, &q.lab_runs[0], 100.0);
    fake.check(&q.lab_runs[0], "security", "sql-injection", false);
    let lab_cx = cx(&d, &fake, &store, &accepting, 2);
    let q = engine::advance(
        &lab_cx,
        Some(&Scripted {
            second_agrees: true,
        }),
        &q.id,
    )
    .unwrap();
    assert_eq!(q.verdict, "passed", "{q:?}");
    let acceptances = store.acceptances(&accepting.id);
    assert_eq!(acceptances.len(), 1);
    assert_eq!(acceptances[0].key, "security");
    assert_eq!(acceptances[0].tokens, 100, "both calls are counted");
    assert_eq!(q.result(Stage::Security).unwrap().state, State::Accepted);

    // The same failure with the second model disagreeing: no acceptance, and the release fails.
    let lab_cx = cx(&d, &fake, &store, &refused, 3);
    let q = engine::start(
        &lab_cx,
        Some(&Scripted {
            second_agrees: false,
        }),
        &planned,
    )
    .unwrap();
    healthy_run(&fake, &store, &q.lab_runs[0], 100.0);
    fake.check(&q.lab_runs[0], "security", "sql-injection", false);
    let lab_cx = cx(&d, &fake, &store, &refused, 4);
    let q = engine::advance(
        &lab_cx,
        Some(&Scripted {
            second_agrees: false,
        }),
        &q.id,
    )
    .unwrap();
    assert_eq!(q.verdict, "failed", "{q:?}");
    assert!(store.acceptances(&refused.id).is_empty());
    assert!(
        q.tasks.iter().any(|t| t.goal.starts_with("Fix security")),
        "{:?}",
        q.tasks
    );
    assert_eq!(
        flow::get(&d, &refused.id).unwrap().state,
        ReleaseState::Failed
    );
}

/// A qualification run with nothing in it yet, for the tests that drive one stage.
fn blank(id: &str, release: &str) -> QualifyRun {
    QualifyRun {
        id: id.into(),
        release: release.into(),
        project: "app".into(),
        planner: String::new(),
        second: String::new(),
        channel: "stable".into(),
        depth: Vec::new(),
        short: Vec::new(),
        reason: String::new(),
        excluded: Vec::new(),
        recipe_run: String::new(),
        lab_runs: Vec::new(),
        results: Vec::new(),
        tasks: Vec::new(),
        accepted: Vec::new(),
        verdict: String::new(),
        reasons: Vec::new(),
        state: "done".into(),
        review_run: String::new(),
        started_ms: 0,
        ended_ms: None,
    }
}

#[test]
fn a_candidate_that_the_planner_would_not_run_for_a_stage_is_out_of_the_depth_with_its_reason() {
    let plan = super::plan::plan(
        "stable",
        None,
        &super::plan::Requirements {
            has_topologies: false,
            risky: false,
            soak_asked: false,
        },
    );
    assert!(
        plan.excluded
            .iter()
            .any(|e| e.stage == Stage::Functional && e.why.contains("no topology"))
    );
    let _ = Metric {
        name: String::new(),
        value: 0.0,
    };
}
