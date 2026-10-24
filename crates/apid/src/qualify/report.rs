//! The release report and the trends (PLAN 22.11), built as JSON from the qualification's records (pure: the caller gives
//! the records and the lab's minutes). The report has every stage with its result, its evidence and its findings; the
//! deltas and the new and fixed findings against the previous qualification; the acceptances; the quarantine and the
//! flaky tests; the matrices; and the cost (model tokens and lab minutes).

use serde_json::{Value, json};

use crate::releases::entity::Release;

use super::accept::Acceptance;
use super::plan::Stage;
use super::result::{Metric, StageResult, State};
use super::store::QualifyRun;
use super::testcases::History;

/// What a report is built from.
pub struct Input<'a> {
    pub release: &'a Release,
    pub run: &'a QualifyRun,
    /// The project's previous qualification, the one the deltas compare with.
    pub previous: Option<&'a QualifyRun>,
    pub previous_release: Option<&'a Release>,
    pub acceptances: &'a [Acceptance],
    /// The project's test histories.
    pub histories: &'a [History],
    /// The lab minutes the run's lab runs held.
    pub lab_minutes: u64,
}

/// The percentage change from `before` to `after` (0 when `before` is 0).
fn change_pct(before: f64, after: f64) -> f64 {
    if before == 0.0 {
        0.0
    } else {
        (after - before) / before * 100.0
    }
}

/// The findings a run's failed stages raised, as `stage:id`.
fn failed_findings(run: &QualifyRun) -> Vec<String> {
    run.results
        .iter()
        .filter(|r| r.state == State::Failed)
        .flat_map(|r| {
            r.findings
                .iter()
                .map(move |f| format!("{}:{}", r.stage, f.id))
        })
        .collect()
}

fn stage_json(stage: Stage, run: &QualifyRun) -> Value {
    let in_depth = run.depth.iter().any(|k| k == stage.key());
    let excluded = run
        .excluded
        .iter()
        .find(|e| e.stage == stage)
        .map(|e| e.why.clone());
    let result: Option<&StageResult> = run.result(stage);
    let state = match result {
        Some(r) => serde_json::to_value(r.state).unwrap_or(Value::Null),
        None if !in_depth => json!("not_in_depth"),
        None => json!("pending"),
    };
    json!({
        "number": stage.number(),
        "key": stage.key(),
        "title": stage.title(),
        "in_depth": in_depth,
        "short": run.short.iter().any(|k| k == stage.key()),
        "state": state,
        "reason": result.map(|r| r.reason.clone()).or(excluded).unwrap_or_default(),
        "evidence": result.map(|r| r.evidence.clone()).unwrap_or_default(),
        "findings": result.map(|r| r.findings.clone()).unwrap_or_default(),
        "metrics": result.map(|r| r.metrics.clone()).unwrap_or_default(),
        "parts": result.map(|r| r.parts.clone()).unwrap_or_default(),
    })
}

/// The release report (PLAN 22.11): the qualification's stages, compared with the previous qualification.
pub fn report(input: &Input) -> Value {
    let run = input.run;
    let release = input.release;
    let stages: Vec<Value> = Stage::ALL.into_iter().map(|s| stage_json(s, run)).collect();
    let deltas: Vec<Value> = match input.previous {
        Some(prev) => metric_deltas(run, prev),
        None => Vec::new(),
    };
    let now_failed = failed_findings(run);
    let before = input.previous.map(failed_findings).unwrap_or_default();
    let new: Vec<&String> = now_failed.iter().filter(|f| !before.contains(f)).collect();
    let fixed: Vec<&String> = before.iter().filter(|f| !now_failed.contains(f)).collect();
    let quarantine: Vec<Value> = input
        .histories
        .iter()
        .filter(|h| h.quarantined())
        .map(|h| {
            json!({
                "test": h.name,
                "since_ms": h.quarantined_ms,
                "flaked_runs": h.runs.iter().filter(|r| r.flaked).count(),
                "fix_task": h.fix_task,
            })
        })
        .collect();
    let flaky: Vec<&str> = input
        .histories
        .iter()
        .filter(|h| h.flaky())
        .map(|h| h.name.as_str())
        .collect();
    let tokens: u64 = input.acceptances.iter().map(|a| a.tokens).sum();
    json!({
        "release": {
            "id": release.id,
            "version": release.version,
            "channel": release.channel,
            "state": release.state,
        },
        "qualification": {
            "id": run.id,
            "state": run.state,
            "verdict": run.verdict,
            "reasons": run.reasons,
            "depth": {
                "stages": run.depth,
                "short": run.short,
                "reason": run.reason,
                "excluded": run.excluded,
            },
            "lab_runs": run.lab_runs,
            "review_run": run.review_run,
            "started_ms": run.started_ms,
            "ended_ms": run.ended_ms,
        },
        "stages": stages,
        "compared_with": input.previous.map(|prev| json!({
            "release": prev.release,
            "version": input.previous_release.map(|r| r.version.clone()).unwrap_or_default(),
            "id": prev.id,
        })),
        "deltas": deltas,
        "findings": { "new": new, "fixed": fixed },
        "acceptances": input.acceptances,
        "quarantine": quarantine,
        "flaky": flaky,
        "matrices": run.result(Stage::Ux).map(|r| r.parts.clone()).unwrap_or_default(),
        "cost": {
            "model_tokens": tokens,
            "lab_minutes": input.lab_minutes,
            "lab_runs": run.lab_runs.len(),
        },
        "fix_tasks": run.tasks,
    })
}

/// The performance deltas: each metric both qualifications measured, with its previous and current value and the change.
fn metric_deltas(run: &QualifyRun, prev: &QualifyRun) -> Vec<Value> {
    let metrics = |r: &QualifyRun| -> Vec<Metric> {
        r.results
            .iter()
            .flat_map(|res| res.metrics.clone())
            .collect()
    };
    let before = metrics(prev);
    metrics(run)
        .into_iter()
        .filter_map(|now| {
            let was = before.iter().find(|m| m.name == now.name)?;
            Some(json!({
                "name": now.name,
                "previous": was.value,
                "current": now.value,
                "change_pct": change_pct(was.value, now.value),
            }))
        })
        .collect()
}

/// One qualified release's point in the trends: its measured numbers.
pub struct TrendRun<'a> {
    pub release: &'a Release,
    pub run: &'a QualifyRun,
    /// The lab runs' start latency in milliseconds (queued to started), where they started.
    pub start_ms: Vec<u64>,
}

/// The trends across releases (PLAN 22.11): performance metrics, artifact sizes, lab start times and flake rates, one
/// point per qualified release, oldest first.
pub fn trends(runs: &[TrendRun]) -> Value {
    let mut points = Vec::new();
    for t in runs {
        let mut metrics: Vec<Value> = Vec::new();
        let mut flake = None;
        for result in &t.run.results {
            for m in &result.metrics {
                if m.name == "tests.run" {
                    continue;
                }
                metrics.push(json!({"name": m.name, "value": m.value}));
            }
            if result.stage == Stage::Functional.key() {
                let run_count = result
                    .metrics
                    .iter()
                    .find(|m| m.name == "tests.run")
                    .map(|m| m.value);
                let flaked = result
                    .metrics
                    .iter()
                    .find(|m| m.name == "tests.flaked")
                    .map(|m| m.value);
                if let (Some(total), Some(flaked)) = (run_count, flaked)
                    && total > 0.0
                {
                    flake = Some(flaked / total);
                }
            }
        }
        let sizes: Vec<Value> = t
            .release
            .artifacts
            .iter()
            .filter(|a| !a.is_ux())
            .map(|a| json!({"target": a.target, "size": a.size}))
            .collect();
        let start = if t.start_ms.is_empty() {
            Value::Null
        } else {
            json!(t.start_ms.iter().sum::<u64>() / t.start_ms.len() as u64)
        };
        points.push(json!({
            "release": t.release.id,
            "version": t.release.version,
            "channel": t.release.channel,
            "verdict": t.run.verdict,
            "metrics": metrics,
            "sizes": sizes,
            "start_ms": start,
            "flake_rate": flake,
        }));
    }
    json!({ "releases": points })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qualify::result::{Finding, StageResult};

    #[test]
    fn a_change_is_a_percentage_of_the_previous_value() {
        assert_eq!(change_pct(100.0, 120.0), 20.0);
        assert_eq!(change_pct(0.0, 5.0), 0.0);
    }

    #[test]
    fn a_stage_out_of_the_depth_says_why_and_a_missing_one_is_pending() {
        let run = QualifyRun {
            id: "qual-1".into(),
            release: "rel-1".into(),
            project: "app".into(),
            planner: String::new(),
            second: String::new(),
            channel: "nightly".into(),
            depth: vec!["verify".into(), "data".into()],
            short: Vec::new(),
            reason: String::new(),
            excluded: vec![super::super::plan::Excluded {
                stage: Stage::Ux,
                why: "not in the nightly depth".into(),
            }],
            recipe_run: String::new(),
            lab_runs: Vec::new(),
            results: vec![StageResult {
                findings: vec![Finding {
                    id: "stage".into(),
                    text: "bad".into(),
                    href: String::new(),
                }],
                ..StageResult::new(Stage::Verify, State::Failed, "bad", 1)
            }],
            tasks: Vec::new(),
            accepted: Vec::new(),
            verdict: "failed".into(),
            reasons: vec!["verify: bad".into()],
            state: "done".into(),
            review_run: String::new(),
            started_ms: 0,
            ended_ms: Some(1),
        };
        assert_eq!(stage_json(Stage::Ux, &run)["state"], json!("not_in_depth"));
        assert_eq!(
            stage_json(Stage::Ux, &run)["reason"],
            json!("not in the nightly depth")
        );
        assert_eq!(stage_json(Stage::Data, &run)["state"], json!("pending"));
        assert_eq!(stage_json(Stage::Verify, &run)["state"], json!("failed"));
        assert_eq!(failed_findings(&run), vec!["verify:stage".to_string()]);
    }
}
