//! The driver's loop (PLAN 19.2) over one scenario: observe (the tree and the screenshot), decide (the driver's pick, the
//! stronger model below its threshold), act (through the target), check (the step's assertions, the checkpoints, the
//! timings) and record (the tree, the screenshot and the action, as evidence in the object store).
//!
//! A scenario stops at its first failure: a step whose expectation does not hold within its action budget, a driver
//! with no action to take, a checkpoint the checker reads as absent, or a final assertion that fails. The timings are
//! read once the steps are done: a timing over its budget is a performance failure, unless a feature failed first.

use apz_blobs::Blobs;

use super::models::{Models, Pick, PickRequest, Tier};
use super::scenario::{Assertion, Scenario};
use super::target::{Screenshot, Target, Tree};
use crate::recipes::runs::EvidenceRow;

/// What the run reads and writes: the models, the object store for the evidence, the driver's threshold and its
/// default action budget.
pub struct Ctx<'a> {
    pub models: &'a dyn Models,
    pub blobs: &'a Blobs,
    pub threshold: f64,
    pub actions: u32,
}

/// Why a scenario failed: a feature (an expectation, a checkpoint or the end did not hold), or a timing over budget.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    Feature {
        step: Option<usize>,
        why: String,
    },
    Performance {
        timing: String,
        ms: u64,
        budget_ms: u64,
    },
}

impl Failure {
    /// The failure in a line, as the planner reads it and the finding says it.
    pub fn text(&self) -> String {
        match self {
            Failure::Feature {
                step: Some(step),
                why,
            } => format!("step {}: {why}", step + 1),
            Failure::Feature { step: None, why } => why.clone(),
            Failure::Performance {
                timing,
                ms,
                budget_ms,
            } => {
                format!("{timing} took {ms} ms, over its budget of {budget_ms} ms")
            }
        }
    }

    /// Whether the two failures are of one kind (a re-run that fails differently still confirms a performance failure).
    pub fn same_kind(&self, other: &Failure) -> bool {
        matches!(
            (self, other),
            (Failure::Feature { .. }, Failure::Feature { .. })
                | (Failure::Performance { .. }, Failure::Performance { .. })
        )
    }
}

/// One timing read: its measured time (`None` when the target has no clock) and whether it held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed {
    pub name: String,
    pub ms: Option<u64>,
    pub budget_ms: u64,
    pub passed: bool,
}

/// One scenario's run: its failure if any, its timings, and its evidence (seq numbers are the caller's).
pub struct ScenarioRun {
    pub failure: Option<Failure>,
    pub timings: Vec<Timed>,
    pub evidence: Vec<EvidenceRow>,
}

/// What the loop saw: the tree and the screenshot, both in the object store.
struct Seen {
    tree: Tree,
    shot: Screenshot,
    tree_blob: String,
    shot_blob: String,
}

fn observe(ctx: &Ctx, target: &mut dyn Target) -> Result<Seen, String> {
    let tree = target.tree()?;
    let shot = target.screenshot()?;
    let tree_bytes = serde_json::to_vec(&tree).map_err(|e| e.to_string())?;
    let tree_blob = ctx.blobs.put(&tree_bytes)?.hex();
    let shot_blob = ctx.blobs.put(&shot.bytes)?.hex();
    Ok(Seen {
        tree,
        shot,
        tree_blob,
        shot_blob,
    })
}

fn row(scenario: &Scenario, step: usize, action: &str, note: &str, seen: &Seen) -> EvidenceRow {
    EvidenceRow {
        seq: 0,
        scenario: scenario.id.clone(),
        step: step as u32,
        action: action.to_string(),
        tree: seen.tree_blob.clone(),
        screenshot: seen.shot_blob.clone(),
        note: note.to_string(),
    }
}

/// The first assertion that does not hold in what the target shows, and why. An assertion is read from the tree; with
/// no tree the checker looks at the screenshot.
fn unmet(ctx: &Ctx, asserts: &[Assertion], seen: &Seen) -> Result<Option<String>, String> {
    for assertion in asserts {
        let question = assertion.question();
        if seen.tree.is_empty() {
            let look = ctx.models.look(&question, &seen.shot)?;
            if !look.holds {
                return Ok(Some(format!("{question} ({})", look.reason)));
            }
        } else if !assertion.holds_in(&seen.tree) {
            return Ok(Some(question));
        }
    }
    Ok(None)
}

struct Decision {
    pick: Pick,
    escalated: bool,
}

/// The driver's pick over the screen's actionable elements. The fast picker answers first; below the threshold the
/// stronger model answers with the screenshot.
fn decide(ctx: &Ctx, goal: &str, seen: &Seen) -> Result<Decision, String> {
    let options = seen
        .tree
        .actionable()
        .map(|e| (e.id.clone(), format!("{} \"{}\"", e.role, e.label)))
        .collect();
    let req = PickRequest {
        goal,
        screen: &seen.tree.title,
        options,
    };
    let fast = ctx.models.pick(Tier::Fast, &req, None)?;
    if fast.confidence >= ctx.threshold {
        return Ok(Decision {
            pick: fast,
            escalated: false,
        });
    }
    let strong = ctx.models.pick(Tier::Strong, &req, Some(&seen.shot))?;
    Ok(Decision {
        pick: strong,
        escalated: true,
    })
}

/// Runs one scenario against `target` (a fresh one, for each run).
pub fn run_scenario(
    ctx: &Ctx,
    target: &mut dyn Target,
    scenario: &Scenario,
) -> Result<ScenarioRun, String> {
    let mut evidence = Vec::new();
    // The clock at the start and at the end of each step that finished.
    let mut clocks: Vec<(Option<u64>, Option<u64>)> = Vec::new();
    let mut failure = None;

    'steps: for (i, step) in scenario.steps.iter().enumerate() {
        let start = target.clock_ms();
        let budget = step.actions.unwrap_or(ctx.actions);
        let mut taken = 0;
        loop {
            let seen = observe(ctx, target)?;
            let Some(why) = unmet(ctx, &step.expect, &seen)? else {
                evidence.push(row(scenario, i, "", "holds", &seen));
                break;
            };
            if taken >= budget {
                evidence.push(row(
                    scenario,
                    i,
                    "",
                    &format!("budget of {budget} spent"),
                    &seen,
                ));
                failure = Some(Failure::Feature {
                    step: Some(i),
                    why: format!("{}: {why} (after {budget} actions)", step.goal),
                });
                break 'steps;
            }
            let decision = decide(ctx, &step.goal, &seen)?;
            let note = if decision.escalated {
                format!(
                    "escalated: {} ({:.2})",
                    decision.pick.why, decision.pick.confidence
                )
            } else {
                decision.pick.why.clone()
            };
            let Some(action) = decision.pick.action else {
                evidence.push(row(scenario, i, "", &format!("no action: {note}"), &seen));
                failure = Some(Failure::Feature {
                    step: Some(i),
                    why: format!(
                        "{}: the driver found no action that serves it ({})",
                        step.goal, decision.pick.why
                    ),
                });
                break 'steps;
            };
            target.act(&action)?;
            taken += 1;
            evidence.push(row(scenario, i, &action.describe(), &note, &seen));
        }
        clocks.push((start, target.clock_ms()));
        for checkpoint in scenario.checkpoints.iter().filter(|c| c.after == i) {
            let seen = observe(ctx, target)?;
            let look = ctx.models.look(&checkpoint.question, &seen.shot)?;
            evidence.push(row(
                scenario,
                i,
                "",
                &format!("checkpoint: {}", look.reason),
                &seen,
            ));
            if !look.holds {
                failure = Some(Failure::Feature {
                    step: Some(i),
                    why: format!("checkpoint \"{}\": {}", checkpoint.question, look.reason),
                });
                break 'steps;
            }
        }
    }

    if failure.is_none() && !scenario.assertions.is_empty() {
        let seen = observe(ctx, target)?;
        let last = scenario.steps.len() - 1;
        if let Some(why) = unmet(ctx, &scenario.assertions, &seen)? {
            evidence.push(row(
                scenario,
                last,
                "",
                "final assertion does not hold",
                &seen,
            ));
            failure = Some(Failure::Feature { step: None, why });
        }
    }

    let mut timings = Vec::new();
    for timing in &scenario.timings {
        let from = clocks.get(timing.from).and_then(|c| c.0);
        let to = clocks.get(timing.to).and_then(|c| c.1);
        match (from, to) {
            (Some(a), Some(b)) => {
                let ms = b.saturating_sub(a);
                let passed = ms <= timing.budget_ms;
                if !passed && failure.is_none() {
                    failure = Some(Failure::Performance {
                        timing: timing.name.clone(),
                        ms,
                        budget_ms: timing.budget_ms,
                    });
                }
                timings.push(Timed {
                    name: timing.name.clone(),
                    ms: Some(ms),
                    budget_ms: timing.budget_ms,
                    passed,
                });
            }
            // No clock, or the scenario stopped before the timing's end: nothing measured, nothing failed.
            _ => timings.push(Timed {
                name: timing.name.clone(),
                ms: None,
                budget_ms: timing.budget_ms,
                passed: true,
            }),
        }
    }

    Ok(ScenarioRun {
        failure,
        timings,
        evidence,
    })
}
