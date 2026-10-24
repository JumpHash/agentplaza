//! What a stage reports (PLAN 22.10): its state, the reason it is not passed, its evidence and the findings it raised,
//! and the verdict over every stage (pure). A stage that cannot run is `not run` with its reason, and never passes.

use serde::{Deserialize, Serialize};

use super::plan::Stage;

/// Where a stage is. `Accepted` is a failure the planner accepted with a reason and a second model's agreement
/// (`accept.rs`): it passes, and the report keeps the failure beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Passed,
    Accepted,
    Failed,
    NotRun,
}

impl State {
    /// Whether the state counts as passed. Not run never does.
    pub fn passes(self) -> bool {
        matches!(self, State::Passed | State::Accepted)
    }
}

/// A piece of evidence a report links: a lab run, a log, a test's record, an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, apz_store::Table)]
pub struct Evidence {
    pub kind: String,
    pub label: String,
    pub href: String,
}

/// A finding a stage raised: its id (`test:<name>`, `check:<kind>:<id>`, `stage`), what it says, and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, apz_store::Table)]
pub struct Finding {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub href: String,
}

/// A number a stage measured (a performance value, a size, a start time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Metric {
    pub name: String,
    pub value: f64,
}

/// One part of a stage (the UX checks and the compatibility matrix are the parts of stage 9): its state and reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Part {
    pub name: String,
    pub state: State,
    #[serde(default)]
    pub reason: String,
}

/// One stage's result as the run recorded it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct StageResult {
    /// The stage's key ([`Stage::key`]).
    #[table(key)]
    pub stage: String,
    pub state: State,
    /// Why it is not passed (empty when it passed).
    #[serde(default)]
    pub reason: String,
    pub at_ms: u64,
    #[serde(default)]
    #[table(child = "qualify_stage_evidence")]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    #[table(child = "qualify_stage_finding")]
    pub findings: Vec<Finding>,
    #[serde(default)]
    #[table(child = "qualify_stage_metric")]
    pub metrics: Vec<Metric>,
    #[serde(default)]
    #[table(child = "qualify_stage_part")]
    pub parts: Vec<Part>,
}

impl StageResult {
    /// A result with a state and a reason, and nothing else yet.
    pub fn new(stage: Stage, state: State, reason: &str, at_ms: u64) -> StageResult {
        StageResult {
            stage: stage.key().to_string(),
            state,
            reason: reason.to_string(),
            at_ms,
            evidence: Vec::new(),
            findings: Vec::new(),
            metrics: Vec::new(),
            parts: Vec::new(),
        }
    }
}

/// The verdict over a candidate's stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Outcome {
    /// Every stage of the depth passed (or was accepted). The release moves on.
    Passed,
    /// A stage failed: the release fails, with each failure's reason.
    Failed { reasons: Vec<String> },
    /// Nothing failed, but a stage did not run or has no result yet: the release does not pass, with the reasons.
    Incomplete { reasons: Vec<String> },
}

impl Outcome {
    /// The verdict's name: `passed`, `failed` or `incomplete`.
    pub fn name(&self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed { .. } => "failed",
            Outcome::Incomplete { .. } => "incomplete",
        }
    }

    /// The reasons the verdict gives (none when it passed).
    pub fn reasons(&self) -> Vec<String> {
        match self {
            Outcome::Passed => Vec::new(),
            Outcome::Failed { reasons } | Outcome::Incomplete { reasons } => reasons.clone(),
        }
    }
}

/// The verdict over the stages of `depth` that have results in `results` (pure). A failure fails the release; a stage
/// that did not run, or has no result, keeps it from passing; the verdict stage is not judged here.
pub fn verdict(results: &[StageResult], depth: &[Stage]) -> Outcome {
    let mut failed = Vec::new();
    let mut incomplete = Vec::new();
    for stage in depth.iter().filter(|s| **s != Stage::Verdict) {
        match results.iter().find(|r| r.stage == stage.key()) {
            None => incomplete.push(format!("{}: no result yet", stage.key())),
            Some(result) => match result.state {
                State::Failed => failed.push(format!("{}: {}", stage.key(), result.reason)),
                State::NotRun => {
                    incomplete.push(format!("{}: not run: {}", stage.key(), result.reason))
                }
                State::Passed | State::Accepted => {}
            },
        }
    }
    if !failed.is_empty() {
        Outcome::Failed { reasons: failed }
    } else if !incomplete.is_empty() {
        Outcome::Incomplete {
            reasons: incomplete,
        }
    } else {
        Outcome::Passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed(stage: Stage) -> StageResult {
        StageResult::new(stage, State::Passed, "", 1)
    }

    #[test]
    fn a_stage_that_cannot_run_never_passes_the_release() {
        let depth = [Stage::Verify, Stage::Performance, Stage::Verdict];
        let results = vec![
            passed(Stage::Verify),
            StageResult::new(
                Stage::Performance,
                State::NotRun,
                "no measurement was recorded at this run",
                1,
            ),
        ];
        let outcome = verdict(&results, &depth);
        assert_eq!(outcome.name(), "incomplete");
        assert!(
            outcome.reasons()[0].contains("not run: no measurement"),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_failure_fails_the_release_and_an_accepted_one_passes() {
        let depth = [Stage::Verify, Stage::Security, Stage::Verdict];
        let mut failing = StageResult::new(Stage::Security, State::Failed, "a SQL injection", 1);
        assert_eq!(
            verdict(&[passed(Stage::Verify), failing.clone()], &depth).name(),
            "failed"
        );
        failing.state = State::Accepted;
        assert_eq!(
            verdict(&[passed(Stage::Verify), failing], &depth),
            Outcome::Passed
        );
    }

    #[test]
    fn a_missing_result_is_incomplete_and_the_verdict_stage_is_not_judged() {
        let depth = [Stage::Verify, Stage::Functional, Stage::Verdict];
        assert_eq!(
            verdict(&[passed(Stage::Verify)], &depth).name(),
            "incomplete"
        );
        assert_eq!(
            verdict(&[passed(Stage::Verify), passed(Stage::Functional)], &depth),
            Outcome::Passed
        );
    }
}
