//! The recipe walker (PLAN 4.1, 21.1): the state of one run's graph, with no I/O.
//!
//! - A step starts when every forward edge into it has been taken. An entry step (no forward edge into it) is ready
//!   when the run starts. A step whose incoming edge can no longer be taken is skipped, and so are its successors.
//! - A step settles with a [`Report`]: its gate (the conditions in its `gate`, all of which must hold) decides pass or
//!   fail. Each outgoing edge then takes or drops, by its `when` (`passed`, `failed`, `findings>0`, a number over the
//!   report).
//! - A loop edge (one with `max`) is taken at most `max` times. Taking it rewinds its target: the target and the steps
//!   after it start again, and the caller is told so with [`Move::Rewind`] and [`Move::Reset`].
//! - A step with `fan_out` n runs n items; it passes when every item passes, and fails at the first item that does not.
//! - The run is done when nothing is left to start or finish. A failed step with no taken edge fails the run.
//!
//! The walk is serialisable, so a run survives a restart; [`Walk::recover`] makes what was running ready again (its
//! session did not survive the hub), as the seats do.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::model::{Fan, Recipe};

/// Where a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// Some edge into it is not taken yet.
    Waiting,
    /// It can start.
    Ready,
    /// Its items run.
    Running,
    /// Its gate held.
    Passed,
    /// Its gate did not hold, or an item failed.
    Failed,
    /// An edge into it can never be taken.
    Skipped,
}

/// Where an edge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum EdgeState {
    /// Its source has not settled.
    Open,
    /// Taken: its target may start.
    Taken,
    /// Not taken (its condition did not hold, or its loop is used up).
    Dead,
}

/// What a step reported. A gate and the edges read these: the step's own verdict, and the numbers a check produced
/// when it has them (`None` when it has not, so a condition about them does not hold).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub passed: bool,
    pub findings: Option<u32>,
    pub tests: Option<bool>,
    pub score: Option<i64>,
    /// The owner's approval, when the step asked for one.
    pub owner: Option<bool>,
}

impl Report {
    /// A report that is only a verdict.
    pub fn verdict(passed: bool) -> Report {
        Report {
            passed,
            ..Report::default()
        }
    }
}

/// What the caller does after a step settled, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Move {
    /// The step can start.
    Ready(String),
    /// A loop went back to this step: it starts again, in its next round.
    Rewind(String),
    /// A step the loop rewound and that waits again.
    Reset(String),
    /// The run passed.
    Done,
    /// The run failed, with why.
    Failed(String),
}

/// A condition of a gate or an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// The step's gate held (on an edge) or its verdict passed (in a gate).
    Passed,
    /// The step's gate did not hold (on an edge) or its verdict did not pass (in a gate).
    Failed,
    /// The step's checks are green.
    Tests,
    /// The step found nothing.
    NoFindings,
    /// The owner approved.
    Owner,
    /// A number the report carries (`findings` or `score`) against a level.
    Number(Number, Op, i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Number {
    Findings,
    Score,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}

impl Cond {
    /// Parses a gate or edge condition: `passed`, `failed`, `tests green`, `no findings`, `owner`, or `<number> <op>
    /// <level>` (`findings>0`, `score >= 3`).
    pub fn parse(text: &str) -> Result<Cond, String> {
        let t = text.trim();
        match t {
            "passed" => return Ok(Cond::Passed),
            "failed" => return Ok(Cond::Failed),
            "tests green" => return Ok(Cond::Tests),
            "no findings" => return Ok(Cond::NoFindings),
            "owner" | "owner approved" => return Ok(Cond::Owner),
            _ => {}
        }
        for (symbol, op) in [
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("==", Op::Eq),
            ("<", Op::Lt),
            (">", Op::Gt),
        ] {
            if let Some((left, right)) = t.split_once(symbol) {
                let number = match left.trim() {
                    "findings" => Number::Findings,
                    "score" => Number::Score,
                    other => return Err(format!("`{t}`: {other} is not a number of a step")),
                };
                let level: i64 = right
                    .trim()
                    .parse()
                    .map_err(|_| format!("`{t}`: {} is not a level", right.trim()))?;
                return Ok(Cond::Number(number, op, level));
            }
        }
        Err(format!(
            "`{t}` is not a condition (passed, failed, tests green, no findings, owner, findings>0, score >= N)"
        ))
    }

    /// Whether the condition holds on a report. `passed` is whether the step's gate held (edges read it; a gate's own
    /// condition reads the verdict, so a gate passes a report with `passed` set).
    fn holds(self, report: &Report, gate_held: bool) -> bool {
        match self {
            Cond::Passed => gate_held,
            Cond::Failed => !gate_held,
            Cond::Tests => report.tests == Some(true),
            Cond::NoFindings => report.findings == Some(0),
            Cond::Owner => report.owner == Some(true),
            Cond::Number(number, op, level) => {
                let value = match number {
                    Number::Findings => report.findings.map(i64::from),
                    Number::Score => report.score,
                };
                value.is_some_and(|v| match op {
                    Op::Lt => v < level,
                    Op::Le => v <= level,
                    Op::Eq => v == level,
                    Op::Ge => v >= level,
                    Op::Gt => v > level,
                })
            }
        }
    }
}

/// One step's state in a walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRec {
    pub state: StepState,
    /// How many times the step started (a loop starts it again).
    pub visits: u32,
    /// The items it runs (its fan-out).
    pub items: u32,
    /// The items reported so far.
    pub reported: u32,
    /// The conditions its last gate check read, as the report had them: what held and what did not.
    pub gate: String,
}

/// One edge's state in a walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeRec {
    pub state: EdgeState,
    /// How many times a loop edge was taken.
    pub taken: u32,
}

/// The state of one run's graph, indexed like its recipe (steps by id, edges by position).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Walk {
    pub steps: BTreeMap<String, StepRec>,
    pub edges: Vec<EdgeRec>,
    /// The run's end, once it has one.
    pub outcome: Option<Outcome>,
}

/// How a walk ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Passed,
    Failed(String),
}

impl Walk {
    /// The walk of a recipe that starts now. `items` gives a step's fan-out count (a step absent from it runs one item,
    /// or its fixed count when it has one). Returns the walk and the moves: the entry steps are ready.
    pub fn start(recipe: &Recipe, items: &BTreeMap<String, u32>) -> (Walk, Vec<Move>) {
        let steps = recipe
            .steps
            .iter()
            .map(|s| {
                let count = match &s.fan_out {
                    Fan::Count(n) => *n,
                    Fan::Named(_) => items.get(&s.id).copied().unwrap_or(1),
                }
                .max(1);
                (
                    s.id.clone(),
                    StepRec {
                        state: StepState::Waiting,
                        visits: 0,
                        items: count,
                        reported: 0,
                        gate: String::new(),
                    },
                )
            })
            .collect();
        let mut walk = Walk {
            steps,
            edges: vec![
                EdgeRec {
                    state: EdgeState::Open,
                    taken: 0,
                };
                recipe.edges.len()
            ],
            outcome: None,
        };
        let mut moves = Vec::new();
        for def in &recipe.steps {
            let entry = !recipe
                .edges
                .iter()
                .any(|e| e.to == def.id && e.max.is_none());
            if entry {
                walk.steps
                    .get_mut(&def.id)
                    .expect("every step has a record")
                    .state = StepState::Ready;
                moves.push(Move::Ready(def.id.clone()));
            }
        }
        walk.activate(recipe, &mut moves);
        (walk, moves)
    }

    /// A ready step starts: its session (or its item) is in flight. Only a running step settles.
    pub fn started(&mut self, step: &str) {
        if let Some(rec) = self.steps.get_mut(step)
            && rec.state == StepState::Ready
        {
            rec.state = StepState::Running;
            rec.visits += 1;
        }
    }

    /// The run's end was reached (no more steps to start or finish).
    pub fn ended(&self) -> bool {
        self.outcome.is_some()
    }

    /// After a restart: what was running is ready again, as its session did not survive.
    pub fn recover(&mut self) {
        for rec in self.steps.values_mut() {
            if rec.state == StepState::Running {
                rec.state = StepState::Ready;
                rec.reported = 0;
            }
        }
    }

    /// A step settled with `report`. Returns the moves the caller applies, in order. A report for a step that is not
    /// running (or after the run ended) changes nothing.
    pub fn settle(&mut self, recipe: &Recipe, step: &str, report: &Report) -> Vec<Move> {
        let Some(def) = recipe.step(step) else {
            return Vec::new();
        };
        let running = self
            .steps
            .get(step)
            .is_some_and(|r| r.state == StepState::Running);
        if !running || self.ended() {
            return Vec::new();
        }
        let texts = def.gate_conditions();
        let held: Vec<bool> = texts
            .iter()
            .filter_map(|c| Cond::parse(c).ok())
            .map(|c| c.holds(report, report.passed))
            .collect();
        let gate_held = held.iter().all(|h| *h);
        let rec = self.steps.get_mut(step).expect("the step is in the walk");
        rec.reported += 1;
        rec.gate = texts
            .iter()
            .zip(&held)
            .map(|(text, h)| format!("{text}={h}"))
            .collect::<Vec<_>>()
            .join(", ");
        // Items of a fan-out pass together: the first that fails fails the step, the last that passes passes it.
        if gate_held && rec.reported < rec.items {
            return Vec::new();
        }
        rec.state = if gate_held {
            StepState::Passed
        } else {
            StepState::Failed
        };
        self.edges_of(recipe, step, report, gate_held)
    }

    /// Takes or drops the edges out of a settled step, then starts what they open.
    fn edges_of(
        &mut self,
        recipe: &Recipe,
        step: &str,
        report: &Report,
        gate_held: bool,
    ) -> Vec<Move> {
        let mut moves = Vec::new();
        let mut any_taken = false;
        for (i, edge) in recipe.edges.iter().enumerate() {
            if edge.from != step || self.edges[i].state != EdgeState::Open {
                continue;
            }
            let cond = Cond::parse(&edge.when).unwrap_or(Cond::Failed);
            if !cond.holds(report, gate_held) {
                self.edges[i].state = EdgeState::Dead;
                continue;
            }
            match edge.max {
                None => {
                    self.edges[i].state = EdgeState::Taken;
                    any_taken = true;
                }
                Some(max) if self.edges[i].taken < max => {
                    self.edges[i].taken += 1;
                    self.edges[i].state = EdgeState::Taken;
                    any_taken = true;
                    self.rewind(recipe, i, &mut moves);
                }
                Some(_) => {
                    // The loop is used up: the edge is not taken, and the step's failure stands.
                    self.edges[i].state = EdgeState::Dead;
                }
            }
        }
        if !gate_held && !any_taken {
            let why = format!("the {step} step failed");
            self.steps.get_mut(step).expect("settled").state = StepState::Failed;
            self.outcome = Some(Outcome::Failed(why.clone()));
            moves.push(Move::Failed(why));
            return moves;
        }
        self.activate(recipe, &mut moves);
        moves
    }

    /// A loop edge was taken: its target and what follows it start again.
    fn rewind(&mut self, recipe: &Recipe, edge: usize, moves: &mut Vec<Move>) {
        let target = recipe.edges[edge].to.clone();
        let mut after = vec![target.clone()];
        let mut i = 0;
        while i < after.len() {
            let here = after[i].clone();
            for e in recipe
                .edges
                .iter()
                .filter(|e| e.from == here && e.max.is_none())
            {
                if !after.contains(&e.to) {
                    after.push(e.to.clone());
                }
            }
            i += 1;
        }
        // The edges out of the rewound part open again (their sources settle once more); a loop keeps its count.
        for (j, e) in recipe.edges.iter().enumerate() {
            if after.contains(&e.from) {
                self.edges[j].state = EdgeState::Open;
            }
        }
        for name in &after {
            if let Some(rec) = self.steps.get_mut(name) {
                rec.state = StepState::Waiting;
                rec.reported = 0;
                rec.gate.clear();
            }
        }
        if let Some(rec) = self.steps.get_mut(&target) {
            rec.state = StepState::Ready;
        }
        moves.push(Move::Rewind(target.clone()));
        moves.extend(
            after
                .iter()
                .filter(|s| **s != target)
                .map(|s| Move::Reset(s.clone())),
        );
    }

    /// Starts the steps whose forward edges are all taken, skips those with a dead one, and ends the run when nothing
    /// is left. Runs to a fixed point: a skipped step's edges are dead, which may skip more.
    fn activate(&mut self, recipe: &Recipe, moves: &mut Vec<Move>) {
        loop {
            let mut changed = false;
            for def in &recipe.steps {
                let incoming: Vec<usize> = recipe
                    .edges
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.to == def.id && e.max.is_none())
                    .map(|(i, _)| i)
                    .collect();
                let waiting = self.steps.get(&def.id).map(|r| r.state) == Some(StepState::Waiting);
                if !waiting || incoming.is_empty() {
                    continue;
                }
                if incoming
                    .iter()
                    .any(|i| self.edges[*i].state == EdgeState::Dead)
                {
                    self.steps.get_mut(&def.id).expect("waiting").state = StepState::Skipped;
                    for (i, e) in recipe.edges.iter().enumerate() {
                        if e.from == def.id && e.max.is_none() {
                            self.edges[i].state = EdgeState::Dead;
                        }
                    }
                    changed = true;
                } else if incoming
                    .iter()
                    .all(|i| self.edges[*i].state == EdgeState::Taken)
                {
                    self.steps.get_mut(&def.id).expect("waiting").state = StepState::Ready;
                    moves.push(Move::Ready(def.id.clone()));
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // Entry steps (no forward edge into them) start with the run, once.
        if self.outcome.is_none()
            && self.steps.values().all(|r| {
                !matches!(
                    r.state,
                    StepState::Waiting | StepState::Ready | StepState::Running
                )
            })
        {
            self.outcome = Some(Outcome::Passed);
            moves.push(Move::Done);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipes::model::Recipe;

    fn parse(text: &str) -> Recipe {
        Recipe::parse(text).unwrap()
    }

    /// Build → review, with review → build a loop of at most `max` rounds.
    fn change(max: u32) -> Recipe {
        parse(&format!(
            r#"
id = "change"
version = 1
[[steps]]
id = "build"
role = "builder"
model = "any"
[[steps]]
id = "review"
role = "reviewer"
model = "differ"
gate = ["passed", "no findings"]
[[edges]]
from = "build"
to = "review"
when = "passed"
[[edges]]
from = "review"
to = "build"
when = "failed"
max = {max}
"#
        ))
    }

    #[test]
    fn a_change_starts_its_builder_then_its_reviewer_then_ends() {
        let recipe = change(2);
        let (mut walk, moves) = Walk::start(&recipe, &BTreeMap::new());
        assert_eq!(moves, vec![Move::Ready("build".into())]);
        walk.started("build");
        let moves = walk.settle(&recipe, "build", &Report::verdict(true));
        assert_eq!(moves, vec![Move::Ready("review".into())]);
        walk.started("review");
        let review = Report {
            passed: true,
            findings: Some(0),
            ..Report::default()
        };
        assert_eq!(walk.settle(&recipe, "review", &review), vec![Move::Done]);
        assert_eq!(walk.outcome, Some(Outcome::Passed));
    }

    #[test]
    fn a_gate_with_a_condition_that_does_not_hold_fails_its_step() {
        let recipe = change(2);
        let (mut walk, _) = Walk::start(&recipe, &BTreeMap::new());
        walk.started("build");
        walk.settle(&recipe, "build", &Report::verdict(true));
        walk.started("review");
        // Passed, but the reviewer left a finding: the gate does not hold, so the review goes back to the builder.
        let found = Report {
            passed: true,
            findings: Some(1),
            ..Report::default()
        };
        let moves = walk.settle(&recipe, "review", &found);
        assert_eq!(moves[0], Move::Rewind("build".into()));
        assert_eq!(moves[1], Move::Reset("review".into()));
        assert_eq!(walk.steps["review"].state, StepState::Waiting);
        assert!(
            walk.steps["review"].gate.is_empty(),
            "a rewound step reads its gate again"
        );
    }

    #[test]
    fn a_loop_returns_at_most_its_bound_then_the_run_fails() {
        let recipe = change(2);
        let (mut walk, _) = Walk::start(&recipe, &BTreeMap::new());
        let mut rewinds = 0;
        for _ in 0..2 {
            walk.started("build");
            walk.settle(&recipe, "build", &Report::verdict(true));
            walk.started("review");
            let moves = walk.settle(&recipe, "review", &Report::verdict(false));
            assert!(moves.contains(&Move::Rewind("build".into())));
            rewinds += 1;
        }
        assert_eq!(rewinds, 2);
        walk.started("build");
        walk.settle(&recipe, "build", &Report::verdict(true));
        walk.started("review");
        let moves = walk.settle(&recipe, "review", &Report::verdict(false));
        assert!(matches!(moves.as_slice(), [Move::Failed(why)] if why.contains("review")));
        assert!(matches!(walk.outcome, Some(Outcome::Failed(_))));
    }

    #[test]
    fn a_fan_out_passes_when_every_item_passes_and_fails_at_the_first_that_does_not() {
        let recipe = parse(
            r#"
id = "fan"
version = 1
[[steps]]
id = "check"
role = "reviewer"
fan_out = "per_file"
"#,
        );
        let mut items = BTreeMap::new();
        items.insert("check".to_string(), 3);
        let (mut walk, _) = Walk::start(&recipe, &items);
        walk.started("check");
        assert!(
            walk.settle(&recipe, "check", &Report::verdict(true))
                .is_empty()
        );
        assert!(
            walk.settle(&recipe, "check", &Report::verdict(true))
                .is_empty()
        );
        assert_eq!(
            walk.settle(&recipe, "check", &Report::verdict(true)),
            vec![Move::Done]
        );

        let (mut walk, _) = Walk::start(&recipe, &items);
        walk.started("check");
        assert_eq!(
            walk.settle(&recipe, "check", &Report::verdict(false)),
            vec![Move::Failed("the check step failed".into())]
        );
        assert_eq!(walk.steps["check"].reported, 1);
    }

    #[test]
    fn a_failed_edge_recovers_and_the_other_branch_is_skipped() {
        let recipe = parse(
            r#"
id = "branch"
version = 1
[[steps]]
id = "test"
role = "qa"
[[steps]]
id = "fix"
role = "builder"
[[steps]]
id = "ship"
role = "builder"
[[edges]]
from = "test"
to = "ship"
when = "passed"
[[edges]]
from = "test"
to = "fix"
when = "failed"
"#,
        );
        let (mut walk, _) = Walk::start(&recipe, &BTreeMap::new());
        walk.started("test");
        let moves = walk.settle(&recipe, "test", &Report::verdict(false));
        assert_eq!(moves, vec![Move::Ready("fix".into())]);
        assert_eq!(walk.steps["ship"].state, StepState::Skipped);
        walk.started("fix");
        assert_eq!(
            walk.settle(&recipe, "fix", &Report::verdict(true)),
            vec![Move::Done]
        );
    }

    #[test]
    fn a_number_on_an_edge_routes_by_the_report() {
        let recipe = parse(
            r#"
id = "route"
version = 1
[[steps]]
id = "review"
role = "reviewer"
[[steps]]
id = "fix"
role = "builder"
[[steps]]
id = "merge"
role = "builder"
[[edges]]
from = "review"
to = "fix"
when = "findings>0"
[[edges]]
from = "review"
to = "merge"
when = "findings == 0"
"#,
        );
        let (mut walk, _) = Walk::start(&recipe, &BTreeMap::new());
        walk.started("review");
        let moves = walk.settle(
            &recipe,
            "review",
            &Report {
                passed: true,
                findings: Some(2),
                ..Report::default()
            },
        );
        assert_eq!(moves, vec![Move::Ready("fix".into())]);
        assert_eq!(walk.steps["merge"].state, StepState::Skipped);
    }

    #[test]
    fn a_settled_step_ignores_a_second_report_and_a_restart_makes_running_steps_ready() {
        let recipe = change(2);
        let (mut walk, _) = Walk::start(&recipe, &BTreeMap::new());
        assert!(
            walk.settle(&recipe, "build", &Report::verdict(true))
                .is_empty()
        );
        walk.started("build");
        walk.settle(&recipe, "build", &Report::verdict(true));
        assert!(
            walk.settle(&recipe, "build", &Report::verdict(true))
                .is_empty()
        );
        walk.started("review");
        walk.recover();
        assert_eq!(walk.steps["review"].state, StepState::Ready);
        let copy: Walk = serde_json::from_str(&serde_json::to_string(&walk).unwrap()).unwrap();
        assert_eq!(copy, walk);
    }

    #[test]
    fn conditions_parse_and_the_bad_ones_are_named() {
        assert_eq!(
            Cond::parse("findings>0"),
            Ok(Cond::Number(Number::Findings, Op::Gt, 0))
        );
        assert_eq!(
            Cond::parse("score >= 3"),
            Ok(Cond::Number(Number::Score, Op::Ge, 3))
        );
        assert_eq!(Cond::parse("tests green"), Ok(Cond::Tests));
        assert!(
            Cond::parse("looks good")
                .unwrap_err()
                .contains("not a condition")
        );
        assert!(
            Cond::parse("colour > 2")
                .unwrap_err()
                .contains("not a number")
        );
    }
}
