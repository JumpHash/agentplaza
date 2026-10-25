//! The qualification end to end over a real Weft (the release tests' harness) with fake stage results: the lab is a fake
//! that records the checks, tests and numbers its runs produced, and the models of an acceptance are scripted.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use apz_topology::node_loss::NodeStop;
use apz_topology::spec::{self, Scenario, Topology};
use serde_json::json;

use crate::recipes::runs::RunState;
use crate::releases::entity::ReleaseState;
use crate::releases::flow::{self, Cut, Deps};
use crate::releases::tests::{FakeBuilder, World, cut, lab};

use super::accept::{Answer, Judge};
use super::compat::CompatPort;
use super::engine::{self, Setup};
use super::plan::Stage;
use super::report;
use super::result::{Metric, StageResult, State};
use super::stages::{self, Check, Cx, Lab, LabState, Scrubbed};
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
    /// Each topology name's TOML, and the topology each run observes.
    texts: RefCell<BTreeMap<String, String>>,
    run_topology: RefCell<BTreeMap<String, String>>,
    /// What the scrub answers (copied when unset), and the outcome each run's copy came to.
    scrub_answer: RefCell<Option<Scrubbed>>,
    scrubs: RefCell<BTreeMap<String, Scrubbed>>,
    /// Where each machine runs, and the probe address the hub reaches a service at.
    placed: RefCell<BTreeMap<String, String>>,
    probe_url: RefCell<Option<String>>,
    /// The node stop's state: the probe server answers 503 while `down` is set; `answered` is the stops and resumes.
    down: Arc<AtomicBool>,
    answered: RefCell<Vec<String>>,
    /// Why the lab refuses to queue a run, when set (no run is made then).
    refuse: RefCell<Option<String>>,
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

    fn with_topology(&self, name: &str, text: &str) {
        self.texts
            .borrow_mut()
            .insert(name.to_string(), text.to_string());
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
        topology: &str,
    ) -> Result<String, String> {
        if let Some(why) = self.refuse.borrow().clone() {
            return Err(why);
        }
        *self.next.borrow_mut() += 1;
        let run = format!("lab-{}", self.next.borrow());
        self.run_topology
            .borrow_mut()
            .insert(run.clone(), topology.to_string());
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
            .ok_or_else(|| "not run: no lab runner can re-run it".to_string())
    }

    fn topology(&self, run: &str) -> Option<Topology> {
        let name = self.run_topology.borrow().get(run)?.clone();
        let text = self.texts.borrow().get(&name)?.clone();
        spec::parse(&text).ok()
    }

    fn scrub(&self, run: &str, _source: &str) -> Scrubbed {
        let answer = self
            .scrub_answer
            .borrow()
            .clone()
            .unwrap_or(Scrubbed::Copied);
        self.scrubs
            .borrow_mut()
            .insert(run.to_string(), answer.clone());
        answer
    }

    fn scrubbed(&self, run: &str) -> Option<Scrubbed> {
        self.scrubs.borrow().get(run).cloned()
    }

    fn placement(&self, _run: &str, _topology: &Topology) -> BTreeMap<String, String> {
        self.placed.borrow().clone()
    }

    fn probe(&self, _run: &str, _scenario: &Scenario) -> Option<String> {
        self.probe_url.borrow().clone()
    }

    fn node_stop(&self, _placement: &BTreeMap<String, String>) -> Box<dyn NodeStop + '_> {
        Box::new(FakeNodes { lab: self })
    }
}

/// The hub's node stop, faked: a stop takes the dependants' probe server down and a resume brings it back.
struct FakeNodes<'a> {
    lab: &'a FakeLab,
}

impl NodeStop for FakeNodes<'_> {
    fn stop(&mut self, _machine: &str) -> Result<(), String> {
        self.lab.down.store(true, Ordering::SeqCst);
        self.lab.answered.borrow_mut().push("stopped".into());
        Ok(())
    }
    fn resume(&mut self, _machine: &str) -> Result<(), String> {
        self.lab.down.store(false, Ordering::SeqCst);
        self.lab.answered.borrow_mut().push("active".into());
        Ok(())
    }
}

/// A probe server answering 503 while `down` is set and 200 otherwise (the dependants of a lost node).
fn serve(down: Arc<AtomicBool>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let status = if down.load(Ordering::SeqCst) {
                "503 Service Unavailable"
            } else {
                "200 OK"
            };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
        }
    });
    format!("http://{addr}/")
}

impl CompatPort for FakeLab {
    fn judge(&self, _release: &crate::releases::entity::Release, now: u64) -> StageResult {
        StageResult::new(Stage::Ux, State::Passed, "", now)
    }
}

/// A two-machine topology whose node loss takes the data machine out and probes the edge's service.
const NODE_LOSS: &str = "name = \"pair\"\n\
[[service]]\nname = \"web\"\nimage = \"web:1\"\n\
[[service]]\nname = \"db\"\nimage = \"postgres:17\"\n\
[[machine]]\nname = \"edge\"\nservices = [\"web\"]\n\
[[machine]]\nname = \"data\"\nservices = [\"db\"]\n\
[[resilience]]\nid = \"lose-data\"\naction = \"stop_node\"\ntarget = \"data\"\nat_secs = 0\nfor_secs = 1\nrecover_within_secs = 2\nprobe = { service = \"web\", path = \"/\" }\nexpect = { max_error_percent = 100.0 }\n";

/// A topology that declares a production source for its data stage.
const PRODUCTION: &str = "name = \"shop\"\n\
[[service]]\nname = \"api\"\nimage = \"api:1\"\n\
[data]\nproduction = \"prod\"\n";

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
        compat: lab,
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

/// The data stage's production copy: a refused scan settles the stage as not run, and a clean one lets the stage judge.
#[test]
fn the_production_copy_is_made_as_its_run_is_queued_and_a_refused_scan_never_passes_the_data_stage()
{
    let (client, _handle, _dir, _change, _agent) = lab("qualify-scrub-refused");
    let world = World::new(client, "qualify-scrub-refused");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["shop"]);
    fake.with_topology("shop", PRODUCTION);
    *fake.scrub_answer.borrow_mut() = Some(Scrubbed::Refused(vec![
        "users.email: email (2 samples)".into(),
    ]));
    let release = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    let run = q.lab_runs[0].clone();
    assert!(
        fake.scrubs.borrow().contains_key(&run),
        "the copy was asked as the run was queued"
    );
    healthy_run(&fake, &store, &run, 100.0);
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    let data = q.result(Stage::Data).expect("the data stage was judged");
    assert_eq!(data.state, State::NotRun, "{data:?}");
    assert_eq!(data.reason, "not run: personal data found");
    assert!(
        data.findings.iter().any(|f| f.text.contains("users.email")),
        "{data:?}"
    );
    assert_ne!(q.verdict, "passed", "a refused scan never passes");
}

#[test]
fn a_clean_production_copy_lets_the_data_stage_judge_its_run() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-scrub-clean");
    let world = World::new(client, "qualify-scrub-clean");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["shop"]);
    fake.with_topology("shop", PRODUCTION);
    let release = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    let run = q.lab_runs[0].clone();
    healthy_run(&fake, &store, &run, 100.0);
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    assert_eq!(fake.scrubbed(&run), Some(Scrubbed::Copied));
    let data = q.result(Stage::Data).expect("the data stage was judged");
    assert_eq!(data.state, State::Passed, "{data:?}");
}

/// A node loss runs through the hub's node stop, and its judgement counts in the resilience stage.
#[test]
fn a_node_loss_runs_through_the_hub_node_stop_and_its_judgement_counts_in_the_resilience_stage() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-node-loss");
    let world = World::new(client, "qualify-node-loss");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["pair"]);
    fake.with_topology("pair", NODE_LOSS);
    *fake.placed.borrow_mut() = BTreeMap::from([
        ("edge".to_string(), "n1".to_string()),
        ("data".to_string(), "n2".to_string()),
    ]);
    *fake.probe_url.borrow_mut() = Some(serve(Arc::clone(&fake.down)));
    let release = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    let run = q.lab_runs[0].clone();
    healthy_run(&fake, &store, &run, 100.0);
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    let resilience = q.result(Stage::Resilience).expect("resilience was judged");
    assert_eq!(resilience.state, State::Passed, "{resilience:?}");
    assert!(
        resilience
            .parts
            .iter()
            .any(|p| p.name == "node loss: lose-data" && p.state == State::Passed),
        "{resilience:?}"
    );
    assert_eq!(*fake.answered.borrow(), ["stopped", "active"]);
    // The lost node's probes failed while it was out: that is the degradation the spec allows.
    assert!(
        !fake.down.load(Ordering::SeqCst),
        "the node is back in service"
    );
}

/// A topology whose machines share one node cannot lose one: its node loss is not run, with the reason.
#[test]
fn a_topology_without_a_multi_node_placement_shows_its_node_loss_not_run_with_the_reason() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-node-loss-one");
    let world = World::new(client, "qualify-node-loss-one");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["pair"]);
    fake.with_topology("pair", NODE_LOSS);
    *fake.placed.borrow_mut() = BTreeMap::from([
        ("edge".to_string(), "n1".to_string()),
        ("data".to_string(), "n1".to_string()),
    ]);
    let release = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    let run = q.lab_runs[0].clone();
    healthy_run(&fake, &store, &run, 100.0);
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    let resilience = q.result(Stage::Resilience).expect("resilience was judged");
    assert_eq!(resilience.state, State::NotRun, "{resilience:?}");
    assert!(
        resilience.reason.contains("needs a multi-VM topology"),
        "{resilience:?}"
    );
    assert!(fake.answered.borrow().is_empty(), "no node was stopped");
    assert_ne!(q.verdict, "passed");
}

/// A failed test whose re-run no lab runner can take is recorded as not run, and the test keeps its failure.
#[test]
fn a_failed_test_whose_re_run_no_lab_runner_can_take_is_not_run_and_keeps_its_failure() {
    let (client, _handle, _dir, _change, _agent) = lab("qualify-rerun-none");
    let world = World::new(client, "qualify-rerun-none");
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
    // No re-run is scripted: the runner cannot take one.
    let lab_cx = cx(&d, &fake, &store, &release, 2);
    let q = engine::advance(&lab_cx, None, &q.id).unwrap();
    let functional = q.result(Stage::Functional).expect("functional was judged");
    assert_eq!(functional.state, State::Failed, "{functional:?}");
    assert!(
        functional
            .findings
            .iter()
            .any(|f| f.text.contains("re-run not run")),
        "{functional:?}"
    );
    let failed = functional
        .metrics
        .iter()
        .find(|m| m.name == "tests.failed")
        .unwrap();
    assert_eq!(failed.value, 1.0, "the test still counts as failed");
}

/// A lab that refuses to queue a run fails the topologies stage with its reason, and the recipe run settles as failed.
#[test]
fn a_lab_that_refuses_a_run_fails_the_topologies_stage_with_the_reason_and_settles_the_recipe_run()
{
    let (client, _handle, _dir, _change, _agent) = lab("qualify-enqueue-refused");
    let world = World::new(client, "qualify-enqueue-refused");
    let d = world.deps();
    let store = Store::default();
    let fake = FakeLab::with(&["web"]);
    let why = "no lab runner dispatches test-lab runs yet: the web run cannot start";
    *fake.refuse.borrow_mut() = Some(why.into());
    let release = candidate(&d, "v0.1.0", "stable");
    let lab_cx = cx(&d, &fake, &store, &release, 1);
    let q = engine::start(&lab_cx, None, &setup()).unwrap();
    let q = store.run(&q.id).unwrap();
    assert!(q.lab_runs.is_empty(), "no run was queued: {q:?}");
    let topologies = q
        .result(Stage::Topologies)
        .expect("the topologies stage was judged");
    assert_eq!(topologies.state, State::Failed, "{topologies:?}");
    assert_eq!(topologies.reason, why);
    assert_ne!(
        q.verdict, "passed",
        "a refused run never passes the release"
    );
    assert_eq!(
        d.runs.view(&q.recipe_run).unwrap().run.state,
        RunState::Failed,
        "the recipe run settled as failed"
    );
}
