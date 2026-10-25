//! The stages' result paths (PLAN 22.10): what each stage reads to judge a candidate. A stage that the lab judges reads the
//! checks its topologies' runs recorded (their kinds, see `plan::Stage::kinds`), the tests the runs reported, and the
//! numbers they measured; a stage with none of those recorded did not run, and is `not run` with that reason. The verify
//! stage reads the artifacts (`releases::flow::verify`); the compatibility part of stage 9 reads `compat`.
//!
//! The lab is a port ([`Lab`]): production is [`AppLab`] over the test lab, the metrics store and the project's
//! topologies; the tests give it a fake lab with recorded checks and tests.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use apz_topology::node_loss::{self, NodeStop};
use apz_topology::resilience::{self, Ran, Schedule};
use apz_topology::spec::{Action, Scenario, Topology};

use crate::lab_performance;
use crate::releases::entity::Release;
use crate::releases::flow::{self, Deps};
use crate::state::App;

use super::compat::{self as adapter, CompatPort};
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

/// What a run's scrubbed copy came to (PLAN 10.13): kept for the run, refused by its scan (with what the scan found, by
/// table, column and kind, never a value), or not made (the source or the rules failed, with the reason).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scrubbed {
    Copied,
    Refused(Vec<String>),
    Failed(String),
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
    /// The topology a run observes, parsed (none when the run names none or it is not in the project).
    fn topology(&self, run: &str) -> Option<Topology>;
    /// Makes the scrubbed copy of `source` for a run (10.13) and records its outcome, read back by [`Lab::scrubbed`].
    fn scrub(&self, run: &str, source: &str) -> Scrubbed;
    /// The outcome of a run's scrubbed copy, or none when no copy was asked for.
    fn scrubbed(&self, run: &str) -> Option<Scrubbed>;
    /// Where each of a run's machines runs (machine to node). Empty when the run does not span several nodes.
    fn placement(&self, run: &str, topology: &Topology) -> BTreeMap<String, String>;
    /// The address the hub probes a scenario's service at during the run, or none when the hub cannot reach it.
    fn probe(&self, run: &str, scenario: &Scenario) -> Option<String>;
    /// The hub's node stop for a placement (10.11, PLAN #59): stops and resumes the nodes its machines run on.
    fn node_stop(&self, placement: &BTreeMap<String, String>) -> Box<dyn NodeStop + '_>;
}

/// What a stage needs to judge a candidate.
pub struct Cx<'a> {
    pub deps: &'a Deps<'a>,
    pub lab: &'a dyn Lab,
    pub store: &'a Store,
    pub release: &'a Release,
    pub project: &'a str,
    pub now: u64,
    /// The compatibility part of stage 9: production is the lab's [`CompatPort`] (`compat::compatibility_adapter`).
    pub compat: &'a dyn CompatPort,
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
            if topologies_pending {
                return Ok(None);
            }
            // A refused scrub settles the data stage at once: its copy is never made, so its numbers never come.
            if let Some(refused) = (stage == Stage::Data)
                .then(|| scrub_refusal(q, cx))
                .flatten()
            {
                return Ok(Some(refused));
            }
            if !q.lab_runs.iter().all(|run| done(cx.lab, run)) {
                return Ok(None);
            }
            Ok(Some(match stage {
                Stage::Functional => functional(q, cx),
                Stage::Ux => ux(q, cx),
                Stage::Resilience => resilience(q, cx),
                Stage::Security
                | Stage::Data
                | Stage::Observe
                | Stage::Performance
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
                format!(" (re-run not run: {})", attempts.reason)
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
    let compat = cx.compat.judge(cx.release, now);
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
    // The compatibility part's failing cells are findings of the stage, and its evidence is kept with it.
    r.findings.extend(compat.findings.iter().cloned());
    r.evidence.extend(compat.evidence.iter().cloned());
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

/// Stage 2's data copy refused or not made (10.13): the stage is not run, with the reason, and never passes.
fn scrub_refusal(q: &QualifyRun, cx: &Cx) -> Option<StageResult> {
    for run in &q.lab_runs {
        match cx.lab.scrubbed(run) {
            Some(Scrubbed::Refused(found)) => {
                let mut r = result(
                    Stage::Data,
                    State::NotRun,
                    "not run: personal data found",
                    cx.now,
                );
                r.findings = found
                    .into_iter()
                    .map(|text| Finding {
                        id: "scrub".into(),
                        text,
                        href: run_href(run),
                    })
                    .collect();
                return Some(r);
            }
            Some(Scrubbed::Failed(why)) => {
                return Some(result(
                    Stage::Data,
                    State::NotRun,
                    &format!("not run: the scrubbed copy was not made: {why}"),
                    cx.now,
                ));
            }
            Some(Scrubbed::Copied) | None => {}
        }
    }
    None
}

/// Forwards a node stop it borrows (the node-loss runner takes its stop by value).
struct Borrowed<'a>(&'a mut dyn NodeStop);

impl NodeStop for Borrowed<'_> {
    fn stop(&mut self, machine: &str) -> Result<(), String> {
        self.0.stop(machine)
    }
    fn resume(&mut self, machine: &str) -> Result<(), String> {
        self.0.resume(machine)
    }
}

/// Stage 7: the checks the runs recorded (`judged`), and each `stop_node` scenario of the runs' topologies run through the
/// hub's node stop (10.11). A node loss the run cannot make (its machines on one node, or no probe the hub reaches) is
/// `not run` with its reason; one that ran is judged by its probes, and its judgement counts in the stage.
fn resilience(q: &QualifyRun, cx: &Cx) -> StageResult {
    let mut r = judged(Stage::Resilience, q, cx);
    let mut parts: Vec<Part> = Vec::new();
    for run in &q.lab_runs {
        let Some(topology) = cx.lab.topology(run) else {
            continue;
        };
        for scenario in topology
            .resilience
            .iter()
            .filter(|s| s.action == Action::StopNode)
        {
            let name = format!("node loss: {}", scenario.id);
            let placed = cx.lab.placement(run, &topology);
            let nodes: BTreeSet<&String> = placed.values().collect();
            if nodes.len() < 2 {
                parts.push(Part {
                    name,
                    state: State::NotRun,
                    reason: format!(
                        "not run: {} runs on {} node(s): a node loss needs a multi-VM topology on several nodes (W58)",
                        topology.name,
                        nodes.len()
                    ),
                });
                continue;
            }
            let Some(url) = cx.lab.probe(run, scenario) else {
                parts.push(Part {
                    name,
                    state: State::NotRun,
                    reason: format!(
                        "not run: the probe of {} is not reachable from the hub",
                        scenario.probe.service
                    ),
                });
                continue;
            };
            let mut stop = cx.lab.node_stop(&placed);
            let ran = node_loss::run(
                &topology,
                scenario,
                &url,
                Schedule::of(scenario),
                &mut Borrowed(stop.as_mut()),
            );
            match ran {
                Ran::NotRun(why) => parts.push(Part {
                    name,
                    state: State::NotRun,
                    reason: format!("not run: {why}"),
                }),
                Ran::Measured(measured) => {
                    let judgment = resilience::judge(scenario, &measured);
                    if judgment.passed {
                        parts.push(Part {
                            name,
                            state: State::Passed,
                            reason: String::new(),
                        });
                    } else {
                        r.findings.push(Finding {
                            id: format!("resilience:{}", scenario.id),
                            text: judgment.detail.clone(),
                            href: run_href(run),
                        });
                        parts.push(Part {
                            name,
                            state: State::Failed,
                            reason: judgment.detail,
                        });
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        return r;
    }
    let failed = r.state == State::Failed || parts.iter().any(|p| p.state == State::Failed);
    let missing = parts
        .iter()
        .find(|p| p.state == State::NotRun)
        .map(|p| p.reason.clone());
    r.parts = parts;
    if failed {
        r.state = State::Failed;
        r.reason = "a resilience check failed".into();
    } else if let Some(reason) = missing {
        r.state = State::NotRun;
        r.reason = reason;
    } else {
        r.state = State::Passed;
        r.reason.clear();
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
        // `test_lab::enqueue` only queues the run and reserves its capacity. Nothing takes an admitted run to a node
        // or starts its collector, so the run would stay `running` and the stage would wait forever. Until a lab runner
        // dispatches admitted runs and is given their ingest tokens, the stage fails here: no run is queued and no
        // token is made.
        Err(no_lab_runner(project, &release.id, topology))
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

    fn rerun(&self, run: &str, test: &str, attempt: u32) -> Result<bool, String> {
        // A re-run is asked of the lab runner, which is a VM runner: only a pool with KVM can hold one.
        match crate::test_lab::lab_nodes(self.app)
            .into_iter()
            .find(|node| node.capabilities.contains("kvm"))
        {
            None => Err(format!(
                "not run: no lab runner can re-run {test} alone ({run}, attempt {attempt}): no pool has kvm"
            )),
            Some(node) => Err(format!(
                "not run: the lab runner on {} takes the re-run of {test} on a KVM host, which this hub does not reach yet",
                node.id
            )),
        }
    }

    fn topology(&self, run: &str) -> Option<Topology> {
        let lab_run = self.app.test_lab.runs().into_iter().find(|r| r.id == run)?;
        if lab_run.topology.is_empty() {
            return None;
        }
        crate::topology::read(self.app, &lab_run.project)
            .ok()?
            .into_iter()
            .find_map(|found| found.topology.ok().filter(|t| t.name == lab_run.topology))
    }

    fn scrub(&self, run: &str, source: &str) -> Scrubbed {
        match crate::lab_scrub::copy_for_run(self.app, run, source) {
            Ok(view) if view["state"] == "copied" => Scrubbed::Copied,
            Ok(view) => Scrubbed::Refused(
                view["report"]["findings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        format!(
                            "{}.{}: {} ({} samples)",
                            f["table"].as_str().unwrap_or_default(),
                            f["column"].as_str().unwrap_or_default(),
                            f["kind"].as_str().unwrap_or_default(),
                            f["samples"]
                        )
                    })
                    .collect(),
            ),
            Err((_, why)) => Scrubbed::Failed(why),
        }
    }

    fn scrubbed(&self, run: &str) -> Option<Scrubbed> {
        let inner = self
            .app
            .test_lab
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let copy = inner.scrubs.iter().rfind(|s| s.run == run)?;
        Some(match copy.state.as_str() {
            "copied" => Scrubbed::Copied,
            "refused" => Scrubbed::Refused(
                inner
                    .scrub_findings
                    .iter()
                    .filter(|f| f.run == run && f.scrub == copy.seq)
                    .map(|f| {
                        format!(
                            "{}.{}: {} ({} samples)",
                            f.table, f.column, f.kind, f.samples
                        )
                    })
                    .collect(),
            ),
            _ => Scrubbed::Failed(copy.reason.clone()),
        })
    }

    fn placement(&self, run: &str, topology: &Topology) -> BTreeMap<String, String> {
        let Some(lab_run) = self.app.test_lab.runs().into_iter().find(|r| r.id == run) else {
            return BTreeMap::new();
        };
        let mut nodes: Vec<String> = lab_run
            .reservations
            .iter()
            .map(|r| r.node.clone())
            .collect();
        nodes.sort();
        nodes.dedup();
        if nodes.len() < 2 {
            return BTreeMap::new();
        }
        topology
            .machines
            .iter()
            .enumerate()
            .map(|(i, machine)| (machine.name.clone(), nodes[i % nodes.len()].clone()))
            .collect()
    }

    fn probe(&self, _run: &str, _scenario: &Scenario) -> Option<String> {
        // A probe reaches a service of the lab VM, which only a KVM host's relay gives the hub.
        None
    }

    fn node_stop(&self, placement: &BTreeMap<String, String>) -> Box<dyn NodeStop + '_> {
        Box::new(HubNodes {
            app: self.app,
            placement: placement.clone(),
        })
    }
}

impl CompatPort for AppLab<'_> {
    fn judge(&self, release: &Release, now: u64) -> StageResult {
        let d = crate::releases::routes::deps(self.app);
        let (releases, _) = lab_performance::context(self.app, &release.project);
        let baseline = releases
            .iter()
            .find(|r| r.id != release.id && lab_performance::qualified(r))
            .map(|r| r.from.clone());
        let mut pools = vec![apz_topology::compat::local_pool(crate::fleet::LOCAL)];
        pools.extend(
            crate::test_lab::lab_nodes(self.app)
                .into_iter()
                .filter(|node| node.id != crate::fleet::LOCAL)
                .map(|node| apz_topology::compat::Pool {
                    name: node.id,
                    capabilities: node.capabilities,
                }),
        );
        adapter::compatibility_adapter(
            &d,
            release,
            &pools,
            crate::fleet::LOCAL,
            baseline.as_deref(),
            now,
        )
    }
}

/// The hub's node stop (PLAN #59): a machine's node is taken out of service through the owner's stop route, and put back
/// through its resume route (`mover`), as the Nodes screen does.
struct HubNodes<'a> {
    app: &'a App,
    placement: BTreeMap<String, String>,
}

impl HubNodes<'_> {
    fn node(&self, machine: &str) -> Result<String, String> {
        self.placement
            .get(machine)
            .cloned()
            .ok_or_else(|| format!("no node runs machine {machine}"))
    }
}

impl NodeStop for HubNodes<'_> {
    fn stop(&mut self, machine: &str) -> Result<(), String> {
        let node = self.node(machine)?;
        let answer = crate::mover::stop(self.app, &node, false);
        if answer.status == 200 {
            Ok(())
        } else {
            Err(format!(
                "the stop of {node} was refused ({})",
                answer.status
            ))
        }
    }
    fn resume(&mut self, machine: &str) -> Result<(), String> {
        let node = self.node(machine)?;
        let answer = crate::mover::resume(self.app, &node, false);
        if answer.status == 200 {
            Ok(())
        } else {
            Err(format!(
                "the resume of {node} was refused ({})",
                answer.status
            ))
        }
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

/// Why a lab run cannot start: no lab runner dispatches the admitted runs of the test lab yet, so a run queued now
/// would never execute or report its results.
fn no_lab_runner(project: &str, release: &str, topology: &str) -> String {
    format!(
        "no lab runner dispatches test-lab runs yet: the {topology} run of {release} in {project} cannot start"
    )
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

    #[test]
    fn a_lab_run_no_runner_dispatches_fails_its_stage_with_the_reason() {
        let why = no_lab_runner("agentplaza", "rel-1", "lab-vm");
        assert!(
            why.starts_with("no lab runner dispatches test-lab runs yet"),
            "{why}"
        );
        assert!(
            why.contains("the lab-vm run of rel-1 in agentplaza"),
            "{why}"
        );
    }
}
