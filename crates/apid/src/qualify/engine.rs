//! The qualification engine (PLAN 22.10): starts a candidate's qualification on the `qualify` recipe, restricted to the
//! depth the planner chose, walks its steps (independent stages run in parallel: every step the walker has ready is
//! polled), judges each stage through [`stages::poll`], and when the walk stops, decides the verdict. A failure fails the
//! release (after the planner's acceptances, if the qualification was given models for them), its findings become fix
//! tasks, and the release's own review graph records what qualification found.

use std::collections::BTreeMap;

use crate::recipes::Move;
use crate::recipes::graph::{Report, StepState};
use crate::recipes::model::{Edge, Recipe, builtin_named};
use crate::recipes::runs::View;
use crate::releases::flow;
use crate::review_graph::recipe::NodeDef;
use crate::review_graph::{NodeRun, NodeState, Outcome as ReviewOutcome, Run, RunState};

use super::accept::{self, Judge};
use super::plan::{self, Requirements, Stage};
use super::result::{Finding, Outcome, StageResult, State, verdict};
use super::stages::{self, Cx};
use super::store::{FixTask, QualifyRun};

/// What the planner decides a qualification from: the project's depth and soak, its topologies, the release's risk, and
/// the two models an acceptance needs (empty: failures are not accepted; kept on the run).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Setup {
    /// The project's depth for the channel, when it sets one.
    pub override_: Option<Vec<Stage>>,
    pub has_topologies: bool,
    pub risky: bool,
    pub soak_asked: bool,
    pub planner: String,
    pub second: String,
}

/// The restricted recipe: `recipe` with the steps outside `keep` taken out, each edge through a removed step replaced by
/// one from its source to its target (so the verdict still waits for every stage that runs).
pub fn restrict(recipe: &Recipe, keep: &[&str]) -> Recipe {
    let mut out = recipe.clone();
    let removed: Vec<String> = out
        .steps
        .iter()
        .map(|s| s.id.clone())
        .filter(|id| !keep.contains(&id.as_str()))
        .collect();
    for gone in removed {
        let incoming: Vec<Edge> = out
            .edges
            .iter()
            .filter(|e| e.to == gone && e.max.is_none())
            .cloned()
            .collect();
        let outgoing: Vec<Edge> = out
            .edges
            .iter()
            .filter(|e| e.from == gone)
            .cloned()
            .collect();
        out.edges.retain(|e| e.from != gone && e.to != gone);
        for i in &incoming {
            for o in &outgoing {
                let exists = out.edges.iter().any(|e| e.from == i.from && e.to == o.to);
                if i.from != o.to && !exists {
                    out.edges.push(Edge {
                        from: i.from.clone(),
                        to: o.to.clone(),
                        when: "passed".into(),
                        max: None,
                    });
                }
            }
        }
        out.steps.retain(|s| s.id != gone);
    }
    out
}

/// Starts the qualification of `cx.release`: the release goes to qualifying in Weft, the depth is planned and the
/// restricted recipe run started, and the walk advances as far as it can.
pub fn start(cx: &Cx, judge: Option<&dyn Judge>, setup: &Setup) -> Result<QualifyRun, String> {
    flow::qualify(cx.deps, &cx.release.id)?;
    let depth = plan::plan(
        &cx.release.channel,
        setup.override_.as_deref(),
        &Requirements {
            has_topologies: setup.has_topologies,
            risky: setup.risky,
            soak_asked: setup.soak_asked,
        },
    );
    let keep: Vec<&str> = depth.stages.iter().map(|s| s.key()).collect();
    let base = builtin_named("qualify").ok_or("no qualify recipe is on the hub")?;
    let recipe = restrict(&base, &keep);
    recipe.check()?;
    let now = cx.now;
    let (run, moves) = cx.deps.runs.start_with_items(
        &recipe,
        "built_in",
        "release",
        None,
        Some(cx.project),
        Some(&cx.release.id),
        &BTreeMap::new(),
        now,
    );
    for m in moves {
        if let Move::Ready(step) = m {
            cx.deps.runs.started(&run, &step, now);
        }
    }
    let id = cx.store.next_id();
    cx.store.put_run(QualifyRun {
        id: id.clone(),
        release: cx.release.id.clone(),
        project: cx.project.to_string(),
        planner: setup.planner.clone(),
        second: setup.second.clone(),
        channel: cx.release.channel.clone(),
        depth: depth.stages.iter().map(|s| s.key().to_string()).collect(),
        short: depth.short.iter().map(|s| s.key().to_string()).collect(),
        reason: depth.reason.clone(),
        excluded: depth.excluded.clone(),
        recipe_run: run,
        lab_runs: Vec::new(),
        results: Vec::new(),
        tasks: Vec::new(),
        accepted: Vec::new(),
        verdict: String::new(),
        reasons: Vec::new(),
        state: "running".into(),
        review_run: String::new(),
        started_ms: now,
        ended_ms: None,
    });
    advance(cx, judge, &id)
}

fn steps_in(view: &View, state: StepState) -> Vec<String> {
    view.walk
        .steps
        .iter()
        .filter(|(_, s)| s.state == state)
        .map(|(name, _)| name.clone())
        .collect()
}

/// Records a stage's result on the run, and the fix task each quarantine finding asks for.
pub(super) fn record(cx: &Cx, id: &str, result: StageResult) {
    cx.store.update_run(id, |q| {
        for f in result
            .findings
            .iter()
            .filter(|f| f.id.starts_with("quarantine:"))
        {
            let name = f.id.trim_start_matches("quarantine:");
            q.tasks.push(FixTask {
                stage: result.stage.clone(),
                goal: format!("Fix the flaky test {name}: it is quarantined until fixed"),
                task: String::new(),
            });
        }
        q.results.retain(|r| r.stage != result.stage);
        q.results.push(result);
    });
}

/// Walks the qualification as far as it goes: ready steps start, and every running step is judged; a judged step settles
/// its gate. When nothing is left to start or finish, the qualification is finished.
pub fn advance(cx: &Cx, judge: Option<&dyn Judge>, id: &str) -> Result<QualifyRun, String> {
    loop {
        let q = cx.store.run(id).ok_or("no such qualification")?;
        if q.done() {
            return Ok(q);
        }
        let view = cx
            .deps
            .runs
            .view(&q.recipe_run)
            .ok_or("the qualification's recipe run is gone")?;
        let ready = steps_in(&view, StepState::Ready);
        let running = steps_in(&view, StepState::Running);
        for step in &ready {
            cx.deps.runs.started(&q.recipe_run, step, cx.now);
        }
        let mut progressed = false;
        for step in ready.iter().chain(running.iter()) {
            let Some(stage) = Stage::of_key(step) else {
                continue;
            };
            let mut q = cx.store.run(id).ok_or("no such qualification")?;
            if q.result(stage).is_some() {
                continue;
            }
            if stage == Stage::Topologies && q.lab_runs.is_empty() {
                let mut ids = Vec::new();
                let mut failure = None;
                for topology in cx.lab.topologies(cx.project) {
                    match cx.lab.enqueue(cx.project, cx.release, &topology) {
                        Ok(run) => {
                            // A topology that declares a production source gets its scrubbed copy as its run is queued
                            // (10.13): the data stage reads the copy's outcome. The copy is made before the data stage
                            // can start, which waits on the runs.
                            let source = cx
                                .lab
                                .topology(&run)
                                .and_then(|t| t.data)
                                .and_then(|data| data.production);
                            if let Some(source) = source {
                                cx.lab.scrub(&run, &source);
                            }
                            ids.push(run);
                        }
                        Err(why) => {
                            failure = Some(why);
                            break;
                        }
                    }
                }
                if let Some(why) = failure {
                    record(cx, id, StageResult::new(stage, State::Failed, &why, cx.now));
                    cx.deps
                        .runs
                        .settle(&q.recipe_run, step, &Report::verdict(false), cx.now);
                    progressed = true;
                    continue;
                }
                cx.store.update_run(id, |r| r.lab_runs = ids.clone());
                q.lab_runs = ids;
            }
            let judged = if stage == Stage::Verdict {
                let outcome = verdict(&q.results, &q.stages());
                let state = match outcome {
                    Outcome::Passed => State::Passed,
                    Outcome::Failed { .. } => State::Failed,
                    Outcome::Incomplete { .. } => State::NotRun,
                };
                Ok(Some(StageResult::new(
                    stage,
                    state,
                    &outcome.reasons().join("; "),
                    cx.now,
                )))
            } else {
                stages::poll(stage, &q, cx)
            };
            match judged {
                Ok(None) => {}
                Ok(Some(result)) => {
                    let passed = result.state.passes();
                    record(cx, id, result);
                    cx.deps
                        .runs
                        .settle(&q.recipe_run, step, &Report::verdict(passed), cx.now);
                    progressed = true;
                }
                Err(why) => {
                    record(cx, id, StageResult::new(stage, State::Failed, &why, cx.now));
                    cx.deps
                        .runs
                        .settle(&q.recipe_run, step, &Report::verdict(false), cx.now);
                    progressed = true;
                }
            }
        }
        if !progressed {
            break;
        }
    }
    // The walk ends at a failure with no taken edge, while the stages started beside it may still be running: those are
    // judged on their recorded results (a settle after the walk ended changes nothing), so the qualification waits on
    // the started stages that have no result yet.
    let q = cx.store.run(id).ok_or("no such qualification")?;
    let view = cx
        .deps
        .runs
        .view(&q.recipe_run)
        .ok_or("the qualification's recipe run is gone")?;
    let open = steps_in(&view, StepState::Ready)
        .into_iter()
        .chain(steps_in(&view, StepState::Running))
        .filter(|step| Stage::of_key(step).is_some_and(|stage| q.result(stage).is_none()))
        .count();
    if open == 0 {
        return finish(cx, judge, id);
    }
    cx.store
        .run(id)
        .ok_or_else(|| "no such qualification".to_string())
}

/// Ends the qualification: the planner's acceptances of the failures (when it has models for them), the verdict over the
/// stages, the fix tasks of the failures not accepted, the release's review graph, and the verdict in Weft.
fn finish(cx: &Cx, judge: Option<&dyn Judge>, id: &str) -> Result<QualifyRun, String> {
    let mut q = cx.store.run(id).ok_or("no such qualification")?;
    let mut results = q.results.clone();
    let mut accepted = q.accepted.clone();
    if let Some(judge) = judge.filter(|_| !q.planner.is_empty() && !q.second.is_empty()) {
        for (n, result) in results
            .iter_mut()
            .filter(|r| r.state == State::Failed)
            .enumerate()
        {
            let mut keys: Vec<String> = result
                .findings
                .iter()
                .map(|f| test_key(&result.stage, f))
                .collect();
            keys.sort();
            keys.dedup();
            if keys.is_empty() {
                keys.push(result.stage.clone());
            }
            let mut all = true;
            for (k, key) in keys.iter().enumerate() {
                let test = key.strip_prefix("test:").unwrap_or("").to_string();
                let req = accept::Request {
                    stage: result.stage.clone(),
                    test,
                    reason: String::new(),
                    planner: q.planner.clone(),
                    second: q.second.clone(),
                };
                let id_acc = format!("{id}-acc{}-{k}", n + 1);
                match accept::accept(judge, &req, &result.reason, &cx.release.id, &id_acc, cx.now) {
                    Ok(acceptance) => {
                        accepted.push(acceptance.key.clone());
                        cx.store.put_acceptance(acceptance);
                    }
                    Err(why) => {
                        all = false;
                        result.reason = format!("{} (not accepted: {why})", result.reason);
                    }
                }
            }
            if all {
                result.state = State::Accepted;
            }
        }
    }
    let depth = q.stages();
    let outcome = verdict(&results, &depth);
    if !results.iter().any(|r| r.stage == Stage::Verdict.key()) {
        let state = match outcome {
            Outcome::Passed => State::Passed,
            Outcome::Failed { .. } => State::Failed,
            Outcome::Incomplete { .. } => State::NotRun,
        };
        results.push(StageResult::new(
            Stage::Verdict,
            state,
            &outcome.reasons().join("; "),
            cx.now,
        ));
    }
    let mut tasks = q.tasks.clone();
    for result in results.iter().filter(|r| r.state == State::Failed) {
        for finding in result
            .findings
            .iter()
            .filter(|f| !f.id.starts_with("quarantine:"))
        {
            if accepted.contains(&test_key(&result.stage, finding))
                || accepted.contains(&result.stage)
            {
                continue;
            }
            tasks.push(FixTask {
                stage: result.stage.clone(),
                goal: format!("Fix {}: {}", result.stage, finding.text),
                task: String::new(),
            });
        }
    }
    let review = release_review(cx, id, &results, &outcome)?;
    q.results = results;
    q.tasks = tasks;
    q.accepted = accepted;
    q.verdict = outcome.name().to_string();
    q.reasons = outcome.reasons();
    q.review_run = review;
    // The verdict is Weft's statement: a pass moves the release on, a failure fails it. Incomplete leaves it where it is.
    match &outcome {
        Outcome::Passed => {
            flow::qualification(cx.deps, &cx.release.id, true, "every stage passed")?;
        }
        Outcome::Failed { reasons } => {
            flow::qualification(cx.deps, &cx.release.id, false, &reasons.join("; "))?;
        }
        Outcome::Incomplete { .. } => {}
    }
    q.state = "done".into();
    q.ended_ms = Some(cx.now);
    cx.store.put_run(q.clone());
    Ok(q)
}

/// The key a finding is accepted under: `test:<name>` for a test's finding, else the stage.
fn test_key(stage: &str, finding: &Finding) -> String {
    match finding.id.strip_prefix("test:") {
        Some(name) => format!("test:{name}"),
        None => stage.to_string(),
    }
}

/// The release's own review graph (PLAN 22.11): each change's review nodes (prefixed with the change), and a release-level
/// node per stage that qualification judged, with the reasons of what it found.
fn release_review(
    cx: &Cx,
    id: &str,
    results: &[StageResult],
    outcome: &Outcome,
) -> Result<String, String> {
    let now = cx.now;
    let run_id = format!("qrel-{id}");
    let mut nodes: Vec<NodeRun> = Vec::new();
    for change in &cx.release.changes {
        let Some(review) = change
            .review_run
            .as_deref()
            .and_then(|r| cx.deps.reviews.run(r))
        else {
            continue;
        };
        for node in review.nodes {
            let mut node = node.clone();
            node.def.id = format!("{}/{}", change.change, node.def.id);
            node.parent = None;
            nodes.push(node);
        }
    }
    for result in results.iter().filter(|r| r.stage != Stage::Verdict.key()) {
        let mut notes = result.reason.clone();
        for finding in &result.findings {
            notes.push_str(&format!("\n- {}", finding.text));
        }
        nodes.push(NodeRun {
            def: NodeDef {
                id: format!("qualify:{}", result.stage),
                kind: "gate".into(),
                ..NodeDef::default()
            },
            parent: None,
            state: if result.state.passes() {
                NodeState::Passed
            } else {
                NodeState::Failed
            },
            assignee: None,
            work: None,
            tries: 0,
            scores: Vec::new(),
            findings: Vec::new(),
            touches: Vec::new(),
            notes: notes.trim().to_string(),
            record: None,
            started_ms: result.at_ms,
            ended_ms: result.at_ms,
            spawned: 0,
        });
    }
    let (state, decision) = match outcome {
        Outcome::Passed => (RunState::Passed, "pass"),
        Outcome::Failed { .. } => (RunState::Failed, "fail"),
        Outcome::Incomplete { .. } => (RunState::Owner, "owner"),
    };
    cx.deps.reviews.put_run(Run {
        id: run_id.clone(),
        task: String::new(),
        project: cx.project.to_string(),
        worker: 0,
        change: String::new(),
        revision: cx.release.from.clone(),
        recipe: "qualify@1".into(),
        round: 1,
        rounds: 1,
        kind: "release".into(),
        state,
        nodes,
        rules: Vec::new(),
        owner_asked: decision == "owner",
        outcome: Some(ReviewOutcome {
            decision: decision.into(),
            reasons: outcome.reasons(),
            replan: Vec::new(),
            at_ms: now,
            by: "qualification".into(),
        }),
        tree_blob: None,
        summary: format!(
            "qualification of {}: {}",
            cx.release.version,
            outcome.name()
        ),
        purpose: "release".into(),
        repo: String::new(),
        models: Vec::new(),
        created_ms: now,
        updated_ms: now,
    });
    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricting_the_recipe_keeps_the_join_of_the_stages_that_run() {
        let base = builtin_named("qualify").unwrap();
        let keep = ["verify", "topologies", "functional", "verdict"];
        let r = restrict(&base, &keep);
        r.check().unwrap();
        assert_eq!(r.steps.len(), keep.len());
        let mut into_verdict: Vec<&str> = r
            .edges
            .iter()
            .filter(|e| e.to == "verdict")
            .map(|e| e.from.as_str())
            .collect();
        into_verdict.sort();
        assert_eq!(
            into_verdict,
            vec!["functional", "topologies", "verify"],
            "the removed stages' edges are contracted onto the verdict"
        );
    }
}
