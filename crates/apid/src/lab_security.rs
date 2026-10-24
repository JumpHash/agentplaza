//! A lab run's security stages (PLAN 22.8, 22.10 step 8). The stages the topology declares (`[security]`) run as
//! `apz-security` in the lab VM, or where the release's tree is, and print their outcomes; the run's assessment takes them
//! (`POST /v1/test-lab/runs/{id}/assess`, `security`) and judges them here. A stage that did not run is a verdict that did
//! not pass, with its reason, and files nothing: a missing tool is never a pass. A clean stage passes. A hit is a failing
//! verdict, filed once as a confirmed `security` finding on the run's change, its proof the evidence the stage kept.

use apz_topology::assert::Verdict as Check;
use apz_topology::security::{Hit, Outcome};
use apz_topology::spec::{self, Topology};

use crate::lab_observe::{self, LabVerdict};
use crate::review_graph::{By, Finding, Reviews, Verdict as FindingVerdict};
use crate::test_lab::{LabRun, Store};

/// The finding kind a failing security hit is filed as: the change tree reads it as the security axis (`sec`).
pub const KIND: &str = "security";

/// The review finding's severity for a hit's severity: a critical hit blocks the change, an unknown one is major.
pub fn severity_of(hit_severity: &str) -> &'static str {
    match hit_severity {
        "critical" => "blocker",
        "high" | "unknown" => "major",
        _ => "minor",
    }
}

/// Judges the outcomes of a run's security stages: each stage's verdict is stored with the run (a re-assessment replaces
/// it), and each hit is filed once on the run's change.
pub fn assess(
    lab: &Store,
    reviews: &Reviews,
    run: &LabRun,
    topology: &Topology,
    outcomes: &[Outcome],
) -> Vec<LabVerdict> {
    let mut out = Vec::new();
    for outcome in outcomes {
        if !outcome.ran {
            let check = Check {
                id: format!("security:{}", outcome.stage),
                kind: KIND.into(),
                passed: false,
                detail: format!("{}: not run: {}", outcome.stage, outcome.reason),
                cites: Vec::new(),
            };
            out.push(lab_observe::record(lab, run, &check, String::new));
            continue;
        }
        if outcome.hits.is_empty() {
            let check = Check {
                id: format!("security:{}", outcome.stage),
                kind: KIND.into(),
                passed: true,
                detail: format!("{}: clean ({})", outcome.stage, outcome.evidence),
                cites: Vec::new(),
            };
            out.push(lab_observe::record(lab, run, &check, String::new));
            continue;
        }
        for hit in &outcome.hits {
            let check = Check {
                id: format!("security:{}:{}:{}", outcome.stage, hit.id, hit.subject),
                kind: KIND.into(),
                passed: false,
                detail: format!(
                    "{} {} at {}: {}",
                    hit.severity, hit.id, hit.subject, hit.detail
                ),
                cites: vec![format!("lab run {} stage {}", run.id, outcome.stage)],
            };
            out.push(lab_observe::record(lab, run, &check, || {
                file_finding(reviews, run, topology, outcome, hit)
            }));
        }
    }
    out
}

/// The finding a hit becomes on the run's change: a confirmed `security` finding whose proof is the hit and the evidence
/// its stage kept. Empty when the run names no change or the change has no review run yet.
fn file_finding(
    reviews: &Reviews,
    run: &LabRun,
    topology: &Topology,
    outcome: &Outcome,
    hit: &Hit,
) -> String {
    if run.change.is_empty() {
        return String::new();
    }
    let by = By {
        seat: "test-lab".into(),
        model: "security".into(),
        session: run.id.clone(),
    };
    let finding = Finding {
        symbol: format!("{}/{}.toml", spec::DIR, topology.name),
        lineage: None,
        text: format!("{} failed: {} at {}", outcome.stage, hit.id, hit.subject),
        severity: severity_of(&hit.severity).into(),
        node: String::new(),
        round: 0,
        id: String::new(),
        scenario: format!(
            "lab run {} of topology {}: the security stage {} (declared in its [security] table)",
            run.id, topology.name, outcome.stage
        ),
        kind: KIND.into(),
        verdict: FindingVerdict::Confirmed,
        proof: format!("lab run {}\n{}\n{}", run.id, hit.detail, outcome.evidence),
        reason: String::new(),
        by: Some(by.clone()),
        verified_by: Some(by),
        contest: String::new(),
        ruling: String::new(),
        ruling_reason: String::new(),
    };
    reviews
        .attach_finding(&run.change, finding)
        .map_or_else(String::new, |(review, id)| format!("{review}/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab_observe::tests::{review_of, reviews_in};

    const TOPOLOGY: &str = "name = \"web\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\n";

    fn run_of(change: &str) -> LabRun {
        LabRun {
            id: "q7".into(),
            project: "p".into(),
            channel: "stable".into(),
            state: "done".into(),
            cpus: 1,
            memory_mib: 512,
            needs: Vec::new(),
            reservations: Vec::new(),
            enqueued_ms: 1_700_000_000_000,
            started_ms: Some(1_700_000_000_000),
            ended_ms: Some(1_700_000_060_000),
            outcome: None,
            change: change.into(),
            topology: "web".into(),
            release: String::new(),
            token_sha: crate::lab_observe::token_hash("t7"),
            observed: 0,
            dropped: 0,
        }
    }

    fn sql_hit() -> Hit {
        Hit {
            id: "sql-injection".into(),
            severity: "high".into(),
            subject: "GET /users parameter name".into(),
            detail: "error-based: a quote after the value makes the answer say \"syntax error\""
                .into(),
        }
    }

    fn ran(stage: &str, hits: Vec<Hit>) -> Outcome {
        Outcome::ran(stage, hits, "2 parameters probed".into())
    }

    #[test]
    fn a_seeded_hit_fails_its_verdict_and_is_filed_as_a_security_finding_on_the_change() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let lab = Store::default();
        let topology = spec::parse(TOPOLOGY).unwrap();
        let run = run_of("c1");
        let verdicts = assess(
            &lab,
            &reviews,
            &run,
            &topology,
            &[ran("dast:api/users", vec![sql_hit()])],
        );
        assert_eq!(verdicts.len(), 1, "{verdicts:?}");
        let verdict = &verdicts[0];
        assert!(!verdict.passed);
        assert_eq!(verdict.kind, KIND);
        assert!(
            verdict
                .id
                .starts_with("security:dast:api/users:sql-injection")
        );
        let (review, id) = verdict.finding.split_once('/').expect("the hit is filed");
        let finding = reviews
            .run(review)
            .unwrap()
            .findings()
            .find(|f| f.id == id)
            .expect("on the change's review")
            .clone();
        assert_eq!(finding.kind, KIND);
        assert_eq!(finding.verdict, FindingVerdict::Confirmed);
        assert_eq!(finding.severity, "major");
        assert!(finding.proof.contains("syntax error"), "{}", finding.proof);
        assert!(!finding.proof.contains("IOSFOD"));

        // Assessed again, the hit is not filed twice.
        let again = assess(
            &lab,
            &reviews,
            &run,
            &topology,
            &[ran("dast:api/users", vec![sql_hit()])],
        );
        assert_eq!(again[0].finding, verdict.finding);
        assert_eq!(reviews.run(review).unwrap().findings().count(), 1);
    }

    #[test]
    fn a_stage_that_did_not_run_fails_with_its_reason_and_files_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let lab = Store::default();
        let topology = spec::parse(TOPOLOGY).unwrap();
        let skipped = Outcome::skipped("image:trivy", "neither trivy nor grype is on the PATH");
        let verdicts = assess(&lab, &reviews, &run_of("c1"), &topology, &[skipped]);
        assert_eq!(verdicts.len(), 1);
        assert!(!verdicts[0].passed, "a stage that did not run never passes");
        assert!(
            verdicts[0].detail.contains("not run"),
            "{}",
            verdicts[0].detail
        );
        assert!(verdicts[0].finding.is_empty());
        assert_eq!(reviews.run("rv-c1").unwrap().findings().count(), 0);
    }

    #[test]
    fn a_clean_stage_passes_and_a_run_with_no_change_files_no_finding() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let lab = Store::default();
        let topology = spec::parse(TOPOLOGY).unwrap();
        let clean = assess(
            &lab,
            &reviews,
            &run_of("c1"),
            &topology,
            &[ran("secrets", Vec::new())],
        );
        assert!(clean[0].passed, "{clean:?}");
        assert!(clean[0].finding.is_empty());
        let no_change = assess(
            &lab,
            &reviews,
            &run_of(""),
            &topology,
            &[ran("dast:api/users", vec![sql_hit()])],
        );
        assert!(!no_change[0].passed);
        assert!(
            no_change[0].finding.is_empty(),
            "no change, so nothing to file on"
        );
    }

    #[test]
    fn a_critical_hit_blocks_and_an_unknown_one_is_major() {
        assert_eq!(severity_of("critical"), "blocker");
        assert_eq!(severity_of("unknown"), "major");
        assert_eq!(severity_of("medium"), "minor");
    }
}
