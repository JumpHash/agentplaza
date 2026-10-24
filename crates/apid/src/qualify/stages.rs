//! The stages' result paths (PLAN 22.10): what each stage reads to judge a candidate. A stage that the lab judges reads the
//! checks its topologies' runs recorded (their kinds, see `plan::Stage::kinds`), the tests the runs reported, and the
//! numbers they measured; a stage with none of those recorded did not run, and is `not run` with that reason. The verify
//! stage reads the artifacts (`releases::flow::verify`); the compatibility part of stage 9 reads `compat`.
//!
//! The lab is a port ([`Lab`]): production is [`AppLab`] over the test lab, the metrics store and the project's
//! topologies; the tests give it a fake lab with recorded checks and tests.

use std::cell::Cell;

use crate::releases::entity::Release;
use crate::releases::flow::{self, Deps};
use crate::state::App;

use super::plan::Stage;
use super::result::{Evidence, Finding, Metric, Part, StageResult, State};
use super::store::{QualifyRun, Store};
use super::testcases::{self, Attempts};

/// A check a lab run recorded (a lab verdict): its kind, its id, whether it passed and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub run: String,
    pub kind: String,
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

/// A lab run's state: `queued`, `running` or `done`, with its outcome once done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabState {
    pub state: String,
    pub outcome: Option<String>,
    pub enqueued_ms: u64,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

/// The lab, as the qualification sees it.
pub trait Lab {
    /// The topology names the project declares.
    fn topologies(&self, project: &str) -> Vec<String>;
    /// Queues one run of `topology` for `release` (a qualification run); returns its id.
    fn enqueue(&self, project: &str, release: &Release, topology: &str) -> Result<String, String>;
    /// A run's state, or none when there is no such run.
    fn state(&self, run: &str) -> Option<LabState>;
    /// The checks a run recorded.
    fn checks(&self, run: &str) -> Vec<Check>;
    /// The numbers a run measured.
    fn metrics(&self, run: &str) -> Vec<Metric>;
    /// Re-runs one test alone in its run (the n-th re-run): passed or failed, or an error when the runner cannot.
    fn rerun(&self, run: &str, test: &str, attempt: u32) -> Result<bool, String>;
}

/// What a stage needs to judge a candidate.
pub struct Cx<'a> {
    pub deps: &'a Deps<'a>,
    pub lab: &'a dyn Lab,
    pub store: &'a Store,
    pub release: &'a Release,
    pub project: &'a str,
    pub now: u64,
    /// The compatibility part of stage 9: production is `compat::compatibility_adapter`.
    pub compat: &'a dyn Fn(&Release, u64) -> StageResult,
}

const NO_MEASUREMENT: &str = "no measurement was recorded at this run";

fn run_href(run: &str) -> String {
    format!("/v1/test-lab/runs/{run}")
}

fn result(stage: Stage, state: State, reason: &str, now: u64) -> StageResult {
    StageResult::new(stage, state, reason, now)
}

/// Judges one stage over the qualification run `q`. `Ok(None)` while the stage is still waiting on its lab runs.
pub fn poll(stage: Stage, q: &QualifyRun, cx: &Cx) -> Result<Option<StageResult>, String> {
    match stage {
        Stage::Verify => Ok(Some(verify(cx))),
        // The verdict is the engine's: it is judged over every other stage's result.
        Stage::Verdict => Ok(None),
        Stage::Topologies => {
            if q.lab_runs.is_empty() {
                return Ok(Some(result(
                    Stage::Topologies,
                    State::NotRun,
                    "no topology is declared for the project",
                    cx.now,
                )));
            }
            if !q.lab_runs.iter().all(|run| done(cx.lab, run)) {
                return Ok(None);
            }
            Ok(Some(topologies(q, cx)))
        }
        _ => {
            // A lab stage waits for the topologies' stage (its runs are started by it) and for those runs to end.
            let topologies_pending =
                q.stages().contains(&Stage::Topologies) && q.result(Stage::Topologies).is_none();
            if topologies_pending || !q.lab_runs.iter().all(|run| done(cx.lab, run)) {
                return Ok(None);
            }
            Ok(Some(match stage {
                Stage::Functional => functional(q, cx),
                Stage::Ux => ux(q, cx),
                Stage::Security
                | Stage::Data
                | Stage::Observe
                | Stage::Performance
                | Stage::Resilience
                | Stage::Soak => judged(stage, q, cx),
                _ => return Err(format!("{} is not a lab stage", stage.key())),
            }))
        }
    }
}

fn done(lab: &dyn Lab, run: &str) -> bool {
    lab.state(run).is_some_and(|s| s.state == "done")
}

/// Stage 1: the artifacts verify (signatures, provenance, SBOMs) against their bytes (`flow::verify`).
fn verify(cx: &Cx) -> StageResult {
    let now = cx.now;
    match flow::verify(cx.deps, &cx.release.id) {
        Ok(()) => {
            let mut r = result(Stage::Verify, State::Passed, "", now);
            r.evidence = cx
                .release
                .artifacts
                .iter()
                .filter(|a| !a.is_ux())
                .map(|a| Evidence {
                    kind: "artifact".into(),
                    label: a.target.clone(),
                    href: format!("/v1/releases/{}/artifacts/{}", cx.release.id, a.target),
                })
                .collect();
            r
        }
        Err(why) => {
            let mut r = result(Stage::Verify, State::Failed, &why, now);
            r.findings = vec![Finding {
                id: "stage".into(),
                text: why,
                href: String::new(),
            }];
            r
        }
    }
}

/// Stage 2: the topologies' runs. Passed when every run ended passed; a failed or cancelled run decides it.
fn topologies(q: &QualifyRun, cx: &Cx) -> StageResult {
    let now = cx.now;
    if q.lab_runs.is_empty() {
        return result(
            Stage::Topologies,
            State::NotRun,
            "no topology is declared for the project",
            now,
        );
    }
    let mut r = result(Stage::Topologies, State::Passed, "", now);
    r.evidence = q
        .lab_runs
        .iter()
        .map(|run| Evidence {
            kind: "lab run".into(),
            label: run.clone(),
            href: run_href(run),
        })
        .collect();
    let mut failed = Vec::new();
    let mut cancelled = Vec::new();
    for run in &q.lab_runs {
        match cx.lab.state(run).and_then(|s| s.outcome) {
            Some(outcome) if outcome == "passed" => {}
            Some(outcome) if outcome == "cancelled" => cancelled.push(run.clone()),
            _ => failed.push(run.clone()),
        }
    }
    if !failed.is_empty() {
        r.state = State::Failed;
        r.reason = format!("lab run {} did not pass", failed.join(", "));
        r.findings = failed
            .iter()
            .map(|run| Finding {
                id: format!("lab:{run}"),
                text: format!("the topology's run {run} did not pass"),
                href: run_href(run),
            })
            .collect();
    } else if !cancelled.is_empty() {
        r.state = State::NotRun;
        r.reason = format!("lab run {} was cancelled", cancelled.join(", "));
    }
    r
}

/// Stage 3: the functional tests the runs reported. A failed test is re-run alone (`testcases`), its history is kept
/// and a flaky one is quarantined: a quarantined test is left out of the gate and listed.
fn functional(q: &QualifyRun, cx: &Cx) -> StageResult {
    let now = cx.now;
    let mut tests = Vec::new();
    for run in &q.lab_runs {
        tests.extend(
            cx.store
                .lab_tests(run)
                .into_iter()
                .map(|t| (run.clone(), t)),
        );
    }
    if tests.is_empty() {
        return result(Stage::Functional, State::NotRun, NO_MEASUREMENT, now);
    }
    let mut r = result(Stage::Functional, State::Passed, "", now);
    let (mut run_count, mut flaked_count, mut quarantined_count) = (0.0, 0.0, 0.0);
    let mut failing: Vec<String> = Vec::new();
    for (run, test) in tests {
        run_count += 1.0;
        let attempts = if test.passed {
            Attempts {
                name: test.name.clone(),
                outcomes: vec![true],
                reason: String::new(),
            }
        } else {
            testcases::run_with_reruns(&test.name, false, &mut |n| {
                cx.lab.rerun(&run, &test.name, n)
            })
        };
        let newly = Cell::new(false);
        let history = cx.store.update_history(cx.project, &test.name, |h| {
            h.record(&run, now, &attempts);
            newly.set(h.quarantine_if_due(now));
        });
        if attempts.flaked() {
            flaked_count += 1.0;
        }
        if newly.get() {
            r.findings.push(Finding {
                id: format!("quarantine:{}", test.name),
                text: format!(
                    "{} flaked in {} runs within {} days: quarantined, a fix task is filed",
                    test.name,
                    history.runs.iter().filter(|h| h.flaked).count(),
                    testcases::QUARANTINE_DAYS
                ),
                href: run_href(&run),
            });
        }
        if history.quarantined() {
            quarantined_count += 1.0;
            continue;
        }
        r.evidence.push(Evidence {
            kind: "test".into(),
            label: test.name.clone(),
            href: run_href(&run),
        });
        if !attempts.passed() {
            let why = if attempts.reason.is_empty() {
                String::new()
            } else {
                format!(" (re-run: {})", attempts.reason)
            };
            failing.push(test.name.clone());
            r.findings.push(Finding {
                id: format!("test:{}", test.name),
                text: format!(
                    "{} failed in {} attempts{why}",
                    test.name,
                    attempts.outcomes.len()
                ),
                href: run_href(&run),
            });
        }
    }
    r.metrics = vec![
        Metric {
            name: "tests.run".into(),
            value: run_count,
        },
        Metric {
            name: "tests.flaked".into(),
            value: flaked_count,
        },
        Metric {
            name: "tests.quarantined".into(),
            value: quarantined_count,
        },
        Metric {
            name: "tests.failed".into(),
            value: failing.len() as f64,
        },
    ];
    if !failing.is_empty() {
        r.state = State::Failed;
        r.reason = format!("{} test(s) failed: {}", failing.len(), failing.join(", "));
    }
    r
}

/// Stages 4 to 8 and 10: the checks of the stage's kinds that the runs recorded, judged (a failing check fails the stage),
/// with the numbers the runs measured for the performance stage.
fn judged(stage: Stage, q: &QualifyRun, cx: &Cx) -> StageResult {
    let now = cx.now;
    let mut r = result(stage, State::Passed, "", now);
    let mut checks = Vec::new();
    for run in &q.lab_runs {
        checks.extend(
            cx.lab
                .checks(run)
                .into_iter()
                .filter(|c| stage.kinds().contains(&c.kind.as_str())),
        );
    }
    if stage == Stage::Performance {
        for run in &q.lab_runs {
            r.metrics.extend(cx.lab.metrics(run));
        }
    }
    r.evidence = checks
        .iter()
        .map(|c| Evidence {
            kind: c.kind.clone(),
            label: format!("{}: {}", c.kind, c.id),
            href: run_href(&c.run),
        })
        .collect();
    r.evidence.extend(r.metrics.iter().map(|m| {
        Evidence {
            kind: "metric".into(),
            label: format!("{} = {}", m.name, m.value),
            href: q
                .lab_runs
                .first()
                .map(|run| run_href(run))
                .unwrap_or_default(),
        }
    }));
    let failing: Vec<&Check> = checks.iter().filter(|c| !c.passed).collect();
    r.findings = failing
        .iter()
        .map(|c| Finding {
            id: format!("check:{}:{}", c.kind, c.id),
            text: c.detail.clone(),
            href: run_href(&c.run),
        })
        .collect();
    if !failing.is_empty() {
        r.state = State::Failed;
        r.reason = format!("{} check(s) failed", failing.len());
    } else if checks.is_empty() && r.metrics.is_empty() {
        r.state = State::NotRun;
        r.reason = NO_MEASUREMENT.into();
    }
    r
}

/// Stage 9: the UI checks the lab recorded, and the compatibility and install-and-upgrade part (`compat`). The stage
/// passes only when both parts do; a part that did not run keeps it from passing.
fn ux(q: &QualifyRun, cx: &Cx) -> StageResult {
    let now = cx.now;
    let mut checks = Vec::new();
    for run in &q.lab_runs {
        checks.extend(
            cx.lab
                .checks(run)
                .into_iter()
                .filter(|c| Stage::Ux.kinds().contains(&c.kind.as_str())),
        );
    }
    let checks_state = if checks.iter().any(|c| !c.passed) {
        State::Failed
    } else if checks.is_empty() {
        State::NotRun
    } else {
        State::Passed
    };
    let compat = (cx.compat)(cx.release, now);
    let mut r = result(Stage::Ux, State::Passed, "", now);
    let checks_reason = if checks.is_empty() {
        NO_MEASUREMENT.to_string()
    } else {
        String::new()
    };
    r.parts = vec![
        Part {
            name: "checks".into(),
            state: checks_state,
            reason: checks_reason.clone(),
        },
        Part {
            name: "compatibility".into(),
            state: compat.state,
            reason: compat.reason.clone(),
        },
    ];
    r.findings = checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| Finding {
            id: format!("check:{}:{}", c.kind, c.id),
            text: c.detail.clone(),
            href: run_href(&c.run),
        })
        .collect();
    r.evidence = checks
        .iter()
        .map(|c| Evidence {
            kind: c.kind.clone(),
            label: format!("{}: {}", c.kind, c.id),
            href: run_href(&c.run),
        })
        .collect();
    if r.parts.iter().any(|p| p.state == State::Failed) {
        r.state = State::Failed;
        r.reason = "a UX check failed".into();
    } else if let Some(part) = r.parts.iter().find(|p| p.state == State::NotRun) {
        r.state = State::NotRun;
        r.reason = if part.reason.is_empty() {
            NO_MEASUREMENT.into()
        } else {
            part.reason.clone()
        };
    }
    r
}

/// The production lab: the hub's test lab, its metrics store, the project's topologies, and the lab's tests the runs
/// reported (the qualification store).
pub struct AppLab<'a> {
    pub app: &'a App,
}

impl Lab for AppLab<'_> {
    fn topologies(&self, project: &str) -> Vec<String> {
        crate::topology::read(self.app, project)
            .map(|found| {
                found
                    .into_iter()
                    .filter_map(|f| f.topology.ok().map(|t| t.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn enqueue(&self, project: &str, release: &Release, topology: &str) -> Result<String, String> {
        let (run, _token) = crate::test_lab::enqueue(
            self.app,
            project,
            &release.channel,
            2,
            2048,
            Vec::new(),
            ("", topology),
            &release.id,
        )?;
        Ok(run.id)
    }

    fn state(&self, run: &str) -> Option<LabState> {
        self.app
            .test_lab
            .runs()
            .into_iter()
            .find(|r| r.id == run)
            .map(|r| LabState {
                state: r.state,
                outcome: r.outcome,
                enqueued_ms: r.enqueued_ms,
                started_ms: r.started_ms,
                ended_ms: r.ended_ms,
            })
    }

    fn checks(&self, run: &str) -> Vec<Check> {
        self.app
            .test_lab
            .verdicts_of(run)
            .into_iter()
            .map(|v| Check {
                run: run.to_string(),
                kind: v.kind,
                id: v.id,
                passed: v.passed,
                detail: v.detail,
            })
            .collect()
    }

    fn metrics(&self, run: &str) -> Vec<Metric> {
        let limits = crate::lab_performance::limits_of(
            &self
                .app
                .project_settings
                .get(&self.project_of(run))
                .performance_limits,
        );
        let names = crate::lab_performance::names(&limits);
        crate::lab_performance::values(&self.app.metrics, run, &names)
            .into_iter()
            .map(|m| Metric {
                name: m.name,
                value: m.value,
            })
            .collect()
    }

    fn rerun(&self, _run: &str, _test: &str, _attempt: u32) -> Result<bool, String> {
        Err("the lab's test runner takes re-runs on a KVM host: not run here".into())
    }
}

impl AppLab<'_> {
    fn project_of(&self, run: &str) -> String {
        self.app
            .test_lab
            .runs()
            .into_iter()
            .find(|r| r.id == run)
            .map(|r| r.project)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stage_judged_by_the_lab_has_kinds_and_the_verify_stage_has_none() {
        assert!(Stage::Verify.kinds().is_empty());
        assert_eq!(Stage::Performance.kinds(), &["performance"]);
        assert!(Stage::Ux.kinds().contains(&"visual"));
    }
}
