//! User-test runs on the change tree (PLAN 19.5): a `test:user` step's findings are recorded as a review run of the change,
//! with a node that reads `fea` (feature: a scenario failed) and a node that reads `prf` (performance: a timing ran over
//! its budget). The run is recorded finished, with each finding's verdict: confirmed by the verifier's re-run, or dropped
//! with its reason. The run's id is `ut-` and the recipe run's id, so the run is found from the step's run.

use super::{
    Book, By, Finding, NodeDef, NodeRun, NodeState, Outcome, Reviews, Run, RunState, Verdict,
};

/// The node that reads feature (`fea`): its findings are scenarios that failed.
pub const FEATURE_NODE: &str = "user";
/// The node that reads performance (`prf`): its findings are timings over budget.
pub const TIMING_NODE: &str = "timing";

/// The kinds of finding a user test records (a scenario's failure is `user-test`, a timing's is `performance`).
pub const KINDS: [&str; 2] = ["user-test", "performance"];

impl Reviews {
    /// Records the review run of a user test: its two nodes, and each finding on the node its axis names (`fea` or
    /// `prf`), numbered `f<n>` in the run. The run is finished: a confirmed finding fails it.
    pub fn record_user_run(
        &self,
        id: &str,
        task: &str,
        project: &str,
        change: &str,
        findings: Vec<(&'static str, Finding)>,
        now: u64,
    ) -> Result<(), String> {
        let failed = findings
            .iter()
            .any(|(_, f)| f.verdict == Verdict::Confirmed);
        let mut nodes = vec![
            node(FEATURE_NODE, "fea", now),
            node(TIMING_NODE, "prf", now),
        ];
        for (number, (axis, mut finding)) in findings.into_iter().enumerate() {
            let target = if axis == "prf" {
                TIMING_NODE
            } else {
                FEATURE_NODE
            };
            finding.node = target.to_string();
            finding.round = 1;
            finding.id = format!("f{}", number + 1);
            let slot = nodes
                .iter_mut()
                .find(|n| n.def.id == target)
                .ok_or("a user test's node is missing")?;
            if finding.verdict == Verdict::Confirmed {
                slot.state = NodeState::Failed;
            }
            slot.findings.push(finding);
        }
        for n in &mut nodes {
            if n.state != NodeState::Failed {
                n.state = NodeState::Passed;
            }
        }
        let outcome = Outcome {
            decision: if failed { "fail" } else { "pass" }.into(),
            reasons: nodes
                .iter()
                .flat_map(|n| n.findings.iter())
                .filter(|f| f.verdict == Verdict::Confirmed)
                .map(|f| format!("{} {}: {}", f.id, f.symbol, f.text))
                .collect(),
            replan: Vec::new(),
            at_ms: now,
            by: "user-test".into(),
        };
        let run = Run {
            id: id.to_string(),
            task: task.to_string(),
            project: project.to_string(),
            worker: 0,
            change: change.to_string(),
            revision: change.to_string(),
            recipe: "user-test".into(),
            round: 1,
            rounds: 1,
            kind: "user".into(),
            state: if failed {
                RunState::Failed
            } else {
                RunState::Passed
            },
            nodes,
            rules: Vec::new(),
            owner_asked: false,
            outcome: Some(outcome),
            tree_blob: None,
            summary: String::new(),
            purpose: "user".into(),
            repo: String::new(),
            models: Vec::new(),
            created_ms: now,
            updated_ms: now,
        };
        self.with(|b: &mut Book| {
            b.runs.insert(run.id.clone(), run);
        });
        Ok(())
    }

    /// The scenarios of the confirmed user findings of `project`'s runs, as the JSON each finding keeps: the regression
    /// scenarios the next run of the step includes.
    pub fn confirmed_scenarios(&self, project: &str) -> Vec<String> {
        let book = self.book.lock().unwrap_or_else(|p| p.into_inner());
        book.runs
            .values()
            .filter(|r| r.project == project)
            .flat_map(|r| r.findings())
            .filter(|f| f.verdict == Verdict::Confirmed && KINDS.contains(&f.kind.as_str()))
            .map(|f| f.scenario.clone())
            .collect()
    }
}

fn node(id: &str, axis: &str, now: u64) -> NodeRun {
    NodeRun {
        def: NodeDef {
            id: id.into(),
            kind: "check".into(),
            tool: None,
            role: Some("user".into()),
            axes: vec![axis.into()],
            commands: Vec::new(),
            after: Vec::new(),
            sub_reviews: 0,
            constraints: false,
            ask: String::new(),
        },
        parent: None,
        state: NodeState::Running,
        assignee: None,
        work: None,
        tries: 0,
        scores: Vec::new(),
        findings: Vec::new(),
        touches: Vec::new(),
        notes: String::new(),
        record: None,
        started_ms: now,
        ended_ms: now,
        spawned: 0,
    }
}

/// The `By` a user test's finding names: the step's driver, as its seat.
pub fn by(seat: &str, model: &str) -> By {
    By {
        seat: seat.into(),
        model: model.into(),
        session: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn finding(verdict: Verdict, scenario: &str, kind: &str) -> Finding {
        Finding {
            symbol: "cart::pay".into(),
            lineage: None,
            text: "Pay did not reach the receipt".into(),
            severity: "major".into(),
            node: String::new(),
            round: 0,
            id: String::new(),
            scenario: scenario.into(),
            kind: kind.into(),
            verdict,
            proof: String::new(),
            reason: String::new(),
            by: Some(by("user", "mock/script")),
            verified_by: None,
            contest: String::new(),
            ruling: String::new(),
            ruling_reason: String::new(),
        }
    }

    #[test]
    fn a_confirmed_failure_reads_feature_and_a_slow_timing_reads_performance() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = apz_blobs::Blobs::scratch(dir.path()).unwrap();
        let reviews = Reviews::new(std::sync::Arc::new(blobs));
        let scenario = json!({"id": "pay"}).to_string();
        reviews
            .record_user_run(
                "ut-rr-1",
                "t1",
                "p1",
                "t1",
                vec![
                    ("fea", finding(Verdict::Confirmed, &scenario, "user-test")),
                    ("prf", finding(Verdict::Dropped, &scenario, "performance")),
                ],
                5,
            )
            .unwrap();
        let run = reviews.run("ut-rr-1").unwrap();
        assert_eq!(run.state, RunState::Failed);
        assert_eq!(run.node(FEATURE_NODE).unwrap().state, NodeState::Failed);
        assert_eq!(run.node(TIMING_NODE).unwrap().state, NodeState::Passed);
        assert_eq!(run.node(FEATURE_NODE).unwrap().findings[0].id, "f1");
        assert_eq!(reviews.confirmed_scenarios("p1"), vec![scenario]);
        assert!(reviews.confirmed_scenarios("p2").is_empty());
    }
}
