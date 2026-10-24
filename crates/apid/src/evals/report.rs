//! The reports the eval records feed (PLAN 25.2): what runs cost and how they did, grouped by project, task, step or
//! model, and how each reviewer model did (its precision, and what it missed). Pure folds over the records the store
//! reads (`evals.rs`), so the rules are tested here without a database.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{FindingRecord, Run};

/// What a cost report groups its runs by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Project,
    Task,
    Step,
    Model,
}

impl By {
    /// The grouping a query names: `project`, `task`, `step` or `model`.
    pub fn parse(name: &str) -> Option<By> {
        match name {
            "project" => Some(By::Project),
            "task" => Some(By::Task),
            "step" => Some(By::Step),
            "model" => Some(By::Model),
            _ => None,
        }
    }

    /// The group a run belongs to under this grouping.
    fn key(self, run: &Run) -> String {
        match self {
            By::Project => run.project.clone(),
            By::Task => run.task.clone().unwrap_or_else(|| "(no task)".into()),
            By::Step => run.step.clone(),
            By::Model => run.model.clone().unwrap_or_else(|| "(no model)".into()),
        }
    }
}

/// What the runs of one group cost and how they did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostRow {
    pub key: String,
    pub runs: u64,
    pub successes: u64,
    /// Successes over runs, 0 to 1.
    pub success_rate: f64,
    /// Clean successes over runs, 0 to 1: a success that no revert, later finding or incident traced back to it.
    pub quality: f64,
    pub cost_micros: u64,
    /// Micro-dollars per success; None without one.
    pub cost_per_success_micros: Option<f64>,
    pub wall_ms: u64,
    /// Milliseconds per success; None without one.
    pub time_per_success_ms: Option<f64>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cached: u64,
    pub tool_calls: u64,
}

/// Whether a run's success stayed clean: it succeeded, and no revert, later finding or incident traced back to it.
pub fn clean(run: &Run) -> bool {
    run.ok && run.reverted != Some(true) && run.later_findings == 0 && run.incidents == 0
}

/// The totals of one group, as the runs are added.
#[derive(Default)]
struct Tally {
    runs: u64,
    successes: u64,
    clean: u64,
    cost: u64,
    wall: u64,
    tokens_in: u64,
    tokens_out: u64,
    tokens_cached: u64,
    tool_calls: u64,
}

impl Tally {
    fn add(&mut self, run: &Run) {
        self.runs += 1;
        self.successes += u64::from(run.ok);
        self.clean += u64::from(clean(run));
        self.cost += run.cost_micros;
        self.wall += run.wall_ms;
        self.tokens_in += run.tokens_in;
        self.tokens_out += run.tokens_out;
        self.tokens_cached += run.tokens_cached;
        self.tool_calls += run.tool_calls;
    }

    fn row(self, key: String) -> CostRow {
        CostRow {
            key,
            runs: self.runs,
            successes: self.successes,
            success_rate: ratio(self.successes, self.runs),
            quality: ratio(self.clean, self.runs),
            cost_micros: self.cost,
            cost_per_success_micros: per_success(self.cost, self.successes),
            wall_ms: self.wall,
            time_per_success_ms: per_success(self.wall, self.successes),
            tokens_in: self.tokens_in,
            tokens_out: self.tokens_out,
            tokens_cached: self.tokens_cached,
            tool_calls: self.tool_calls,
        }
    }
}

/// The cost report: the runs grouped by `by`, the most expensive group first.
pub fn costs(by: By, runs: &[Run]) -> Vec<CostRow> {
    let mut groups: BTreeMap<String, Tally> = BTreeMap::new();
    for run in runs {
        groups.entry(by.key(run)).or_default().add(run);
    }
    let mut rows: Vec<CostRow> = groups.into_iter().map(|(key, t)| t.row(key)).collect();
    rows.sort_by(|a, b| {
        b.cost_micros
            .cmp(&a.cost_micros)
            .then_with(|| a.key.cmp(&b.key))
    });
    rows
}

/// A review pass: a reviewer session of `model` passed `task`, and ended at `ended_ms`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub model: String,
    pub task: String,
    pub ended_ms: u64,
}

/// How one reviewer model did (PLAN 25.2): the findings it raised and how they were ruled, its precision (confirmed
/// over all its findings), the changes it passed, and what it missed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReviewerScore {
    pub model: String,
    pub findings: u64,
    pub confirmed: u64,
    pub dropped: u64,
    /// Confirmed over all its findings, 0 to 1; None without a finding.
    pub precision: Option<f64>,
    /// The distinct tasks it passed.
    pub changes_passed: u64,
    /// Confirmed findings another reviewer raised, ruled after this model's last pass of their task.
    pub missed_findings: u64,
    /// Tasks it passed that later drew an incident or were reverted.
    pub late_failures: u64,
    /// `missed_findings` plus `late_failures`: what reached a later stage or an incident after it passed.
    pub misses: u64,
}

/// Each reviewer model's precision and misses, from the findings the reviews ruled on, the passes the reviewer sessions
/// gave, and the tasks that later drew an incident or were reverted (`late`). Models sort by name.
pub fn reviewers(
    findings: &[FindingRecord],
    passes: &[Pass],
    late: &HashSet<String>,
) -> Vec<ReviewerScore> {
    let mut scores: BTreeMap<String, ReviewerScore> = BTreeMap::new();
    // Each model's last pass of each task: a finding ruled after it was missed by that model.
    let mut last_pass: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for p in passes {
        let at = last_pass
            .entry(p.model.clone())
            .or_default()
            .entry(p.task.clone())
            .or_insert(0);
        *at = (*at).max(p.ended_ms);
    }
    for f in findings {
        let score = scores
            .entry(f.reviewer.clone())
            .or_insert_with(|| ReviewerScore {
                model: f.reviewer.clone(),
                ..ReviewerScore::default()
            });
        score.findings += 1;
        match f.verdict.as_str() {
            "confirmed" => score.confirmed += 1,
            "dropped" => score.dropped += 1,
            _ => {}
        }
    }
    for (model, tasks) in &last_pass {
        let score = scores
            .entry(model.clone())
            .or_insert_with(|| ReviewerScore {
                model: model.clone(),
                ..ReviewerScore::default()
            });
        score.changes_passed = tasks.len() as u64;
        score.late_failures = tasks.keys().filter(|t| late.contains(*t)).count() as u64;
    }
    for f in findings.iter().filter(|f| f.verdict == "confirmed") {
        let Some(task) = f.task.as_deref() else {
            continue;
        };
        for (model, tasks) in &last_pass {
            let Some(&passed_at) = tasks.get(task) else {
                continue;
            };
            if *model != f.reviewer
                && f.ruled_ms > passed_at
                && let Some(score) = scores.get_mut(model)
            {
                score.missed_findings += 1;
            }
        }
    }
    scores
        .into_values()
        .map(|mut s| {
            s.precision = (s.findings > 0).then(|| ratio(s.confirmed, s.findings));
            s.misses = s.missed_findings + s.late_failures;
            s
        })
        .collect()
}

/// `part` over `whole` as a share, 0 for no whole.
fn ratio(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// `total` per success, or None when nothing succeeded.
fn per_success(total: u64, successes: u64) -> Option<f64> {
    (successes > 0).then(|| total as f64 / successes as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(model: &str, step: &str, ok: bool, cost: u64, wall: u64) -> Run {
        Run {
            id: 1,
            session: "s1".into(),
            project: "pr1".into(),
            task: Some("t1".into()),
            recipe: "change".into(),
            step: step.into(),
            seat: "builder".into(),
            round: 1,
            engine: "harness".into(),
            engine_version: "0.1.0".into(),
            model: Some(model.into()),
            provider: Some("mock".into()),
            harness: "acp".into(),
            node: None,
            pool: None,
            tokens_in: 10,
            tokens_out: 5,
            tokens_cached: 2,
            cost_micros: cost,
            model_calls: 1,
            tool_calls: 3,
            tool_ms: 30,
            wall_ms: wall,
            verdict: None,
            outcome: String::new(),
            ok,
            landed: None,
            reverted: None,
            ended_ms: 1000,
            recipe_version: 2,
            harness_version: String::new(),
            gate: "none".into(),
            later_findings: 0,
            incidents: 0,
            memory_cites: 0,
            qual_stages: 0,
            lab_minutes: 0,
        }
    }

    fn finding(reviewer: &str, task: &str, verdict: &str, ruled_ms: u64) -> FindingRecord {
        FindingRecord {
            id: 0,
            project: "pr1".into(),
            task: Some(task.into()),
            review: "r1".into(),
            finding: "f1".into(),
            reviewer: reviewer.into(),
            reviewer_seat: "reviewer".into(),
            verifier: "mock/third".into(),
            verdict: verdict.into(),
            severity: "major".into(),
            ruled_ms,
        }
    }

    #[test]
    fn costs_group_by_step_and_model_with_success_cost_and_time_per_success() {
        let runs = vec![
            run("mock/agent", "build", true, 100, 900),
            run("mock/agent", "build", true, 100, 1100),
            run("mock/agent", "build", false, 50, 500),
            run("mock/echo", "review", false, 10, 100),
        ];
        let by_step = costs(By::Step, &runs);
        assert_eq!(by_step.len(), 2, "{by_step:?}");
        let build = by_step.iter().find(|r| r.key == "build").unwrap();
        assert_eq!((build.runs, build.successes), (3, 2));
        assert!((build.success_rate - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(build.cost_micros, 250);
        assert_eq!(build.cost_per_success_micros, Some(125.0));
        assert_eq!(build.time_per_success_ms, Some(1250.0));
        assert_eq!((build.tokens_in, build.tool_calls), (30, 9));
        let by_model = costs(By::Model, &runs);
        let agent = by_model.iter().find(|r| r.key == "mock/agent").unwrap();
        assert_eq!((agent.runs, agent.successes), (3, 2));
        let echo = by_model.iter().find(|r| r.key == "mock/echo").unwrap();
        assert_eq!(
            (echo.success_rate, echo.cost_per_success_micros),
            (0.0, None),
            "no success, no cost per success"
        );
        assert_eq!(
            by_model[0].key, "mock/agent",
            "the most expensive group first"
        );
    }

    #[test]
    fn quality_counts_only_successes_that_no_later_finding_or_incident_or_revert_reached() {
        let mut dirty = run("mock/agent", "build", true, 1, 1);
        dirty.later_findings = 1;
        let mut reverted = run("mock/agent", "build", true, 1, 1);
        reverted.reverted = Some(true);
        let clean_one = run("mock/agent", "build", true, 1, 1);
        let rows = costs(By::Step, &[dirty, reverted, clean_one]);
        assert_eq!(rows[0].successes, 3);
        assert!((rows[0].quality - 1.0 / 3.0).abs() < 1e-9, "{:?}", rows[0]);
    }

    #[test]
    fn a_reviewer_is_scored_on_its_precision_and_on_what_it_missed_after_passing() {
        // X raises f1 (confirmed on t1 at 500) and f2 (confirmed on t2 at 900). Y raises f3 (dropped on t1 at 600) and
        // f4 (confirmed on t2 at 100). Y passed t1 at 400 and t2 at 200; X passed t1 at 300 and t2 at 950.
        let findings = vec![
            finding("mock/x", "t1", "confirmed", 500),
            finding("mock/y", "t1", "dropped", 600),
            finding("mock/x", "t2", "confirmed", 900),
            finding("mock/y", "t2", "confirmed", 100),
        ];
        let passes = vec![
            Pass {
                model: "mock/y".into(),
                task: "t1".into(),
                ended_ms: 400,
            },
            Pass {
                model: "mock/y".into(),
                task: "t2".into(),
                ended_ms: 200,
            },
            Pass {
                model: "mock/x".into(),
                task: "t1".into(),
                ended_ms: 300,
            },
            Pass {
                model: "mock/x".into(),
                task: "t2".into(),
                ended_ms: 950,
            },
        ];
        let scores = reviewers(&findings, &passes, &HashSet::new());
        let x = scores.iter().find(|s| s.model == "mock/x").unwrap();
        let y = scores.iter().find(|s| s.model == "mock/y").unwrap();
        assert_eq!((x.findings, x.confirmed, x.dropped), (2, 2, 0));
        assert_eq!(x.precision, Some(1.0));
        assert_eq!((y.findings, y.confirmed, y.dropped), (2, 1, 1));
        assert_eq!(y.precision, Some(0.5));
        assert_eq!(x.changes_passed, 2);
        // Y passed t1 at 400 and t2 at 200; f1 (raised by X, ruled 500) and f2 (raised by X, ruled 900) came after
        // those passes, so Y missed both. f4 (raised by Y) is not a miss of Y, and X's pass of t2 (950) came after f4
        // (ruled 100), so X does not miss it either.
        assert_eq!(y.missed_findings, 2, "{y:?}");
        assert_eq!(x.missed_findings, 0, "{x:?}");
        assert_eq!(y.misses, 2);
    }

    #[test]
    fn a_pass_of_a_task_that_later_drew_an_incident_or_a_revert_is_a_late_failure() {
        let passes = vec![
            Pass {
                model: "mock/x".into(),
                task: "t1".into(),
                ended_ms: 10,
            },
            Pass {
                model: "mock/x".into(),
                task: "t2".into(),
                ended_ms: 10,
            },
        ];
        let late: HashSet<String> = ["t2".to_string()].into_iter().collect();
        let scores = reviewers(&[], &passes, &late);
        let x = &scores[0];
        assert_eq!((x.changes_passed, x.late_failures, x.misses), (2, 1, 1));
        assert_eq!(x.precision, None, "no finding, no precision");
    }
}
