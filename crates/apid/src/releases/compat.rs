//! A release's compatibility and install-and-upgrade matrices (PLAN 22.9, 10.10). The project's declaration
//! (`.plaza/compatibility.toml`, read by `apz_topology::compat`) expands into cells; each cell is placed on a pool of the
//! lab that has its capabilities, and what the pool reports for it is judged here. A cell no pool can run is `not_run`
//! with its reason; it never counts as passed. A cell that failed is a finding of kind `compatibility` on the release's
//! first change, its proof the evidence the run kept, and it is filed once however often the matrix is assessed.
//!
//! Routes:
//!   POST /v1/releases/{id}/compatibility   {matrix, outcomes?, pools?} judges the matrix on the lab's pools (or the given
//!                                          ones): the release's `compatibility` cells and the matrix's verdict

use apz_topology::compat::{self, Cell, Kind, Matrix, Outcome, Placement, Pool};
use serde::{Deserialize, Serialize};

use super::entity::Release;
use crate::review_graph::{By, Finding, Reviews, Verdict as FindingVerdict};

/// The finding kind a failed cell is filed as.
pub const KIND: &str = "compatibility";

/// One cell's result on the release (the release's `compatibility` child table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct CompatCell {
    #[table(key)]
    pub cell: String,
    /// web | cli | desktop | mobile | library.
    pub product: String,
    /// passed | failed | not_run.
    pub state: String,
    /// The pool the cell was placed on (empty when no pool can run it).
    #[serde(default)]
    pub pool: String,
    /// Why it did not run (empty for a run cell).
    #[serde(default)]
    pub reason: String,
    /// What the run printed or reported.
    #[serde(default)]
    pub evidence: String,
    /// The finding filed for a failure: `<review>/<finding>`.
    #[serde(default)]
    pub finding: String,
}

/// The matrix's verdict: failed when a run cell failed, passed when a cell ran and passed, not_run otherwise. A matrix
/// with some cells not run still passes when every cell that ran passed: the not-run ones are listed beside it.
pub fn verdict(cells: &[CompatCell]) -> &'static str {
    if cells.iter().any(|c| c.state == "failed") {
        "failed"
    } else if cells.iter().any(|c| c.state == "passed") {
        "passed"
    } else {
        "not_run"
    }
}

/// Judges a release's matrix over the pools (pure apart from filing new findings). `previous` are the release's cells
/// from an earlier assessment: a failure that is already filed keeps its finding.
pub fn assess(
    reviews: &Reviews,
    release: &Release,
    matrix: &Matrix,
    pools: &[Pool],
    outcomes: &[Outcome],
    previous: &[CompatCell],
) -> Vec<CompatCell> {
    compat::expand(matrix)
        .iter()
        .map(|cell| judge(reviews, release, cell, pools, outcomes, previous))
        .collect()
}

fn judge(
    reviews: &Reviews,
    release: &Release,
    cell: &Cell,
    pools: &[Pool],
    outcomes: &[Outcome],
    previous: &[CompatCell],
) -> CompatCell {
    let row =
        |state: &str, pool: &str, reason: String, evidence: String, finding: String| CompatCell {
            cell: cell.id.clone(),
            product: cell.product.clone(),
            state: state.into(),
            pool: pool.into(),
            reason,
            evidence,
            finding,
        };
    let pool = match compat::place(cell, pools) {
        Placement::NotRun(reason) => {
            return row("not_run", "", reason, String::new(), String::new());
        }
        Placement::On(pool) => pool,
    };
    if let Kind::Planned { reason } = &cell.kind {
        return row(
            "not_run",
            &pool,
            reason.clone(),
            String::new(),
            String::new(),
        );
    }
    let Some(outcome) = outcomes.iter().find(|o| o.cell == cell.id) else {
        return row(
            "not_run",
            &pool,
            format!("{pool} reported no result for the cell"),
            String::new(),
            String::new(),
        );
    };
    if !outcome.ran {
        return row(
            "not_run",
            &pool,
            outcome.reason.clone(),
            String::new(),
            String::new(),
        );
    }
    if outcome.passed {
        return row(
            "passed",
            &pool,
            String::new(),
            outcome.evidence.clone(),
            String::new(),
        );
    }
    let filed = previous
        .iter()
        .find(|p| p.cell == cell.id && p.state == "failed" && !p.finding.is_empty())
        .map(|p| p.finding.clone())
        .unwrap_or_else(|| file_finding(reviews, release, cell, &pool, outcome));
    row(
        "failed",
        &pool,
        String::new(),
        outcome.evidence.clone(),
        filed,
    )
}

/// The finding a failed cell becomes on the release's first change: a confirmed `compatibility` finding whose proof is
/// the run's evidence. Empty when the release has no change or the change has no review run yet.
fn file_finding(
    reviews: &Reviews,
    release: &Release,
    cell: &Cell,
    pool: &str,
    outcome: &Outcome,
) -> String {
    let Some(change) = release.changes.first() else {
        return String::new();
    };
    let by = By {
        seat: "release-compat".into(),
        model: "compatibility".into(),
        session: release.id.clone(),
    };
    let finding = Finding {
        symbol: format!("{}#{}", compat::FILE, cell.id),
        lineage: None,
        text: format!("{} failed on {pool}", cell.id),
        severity: "major".into(),
        node: String::new(),
        round: 0,
        id: String::new(),
        scenario: format!(
            "release {} ({}), cell {}: its declared matrix in {}",
            release.id,
            release.version,
            cell.id,
            compat::FILE
        ),
        kind: KIND.into(),
        verdict: FindingVerdict::Confirmed,
        proof: format!("release {} on {pool}\n{}", release.id, outcome.evidence),
        reason: String::new(),
        by: Some(by.clone()),
        verified_by: Some(by),
        contest: String::new(),
        ruling: String::new(),
        ruling_reason: String::new(),
    };
    reviews
        .attach_finding(&change.change, finding)
        .map_or_else(String::new, |(review, id)| format!("{review}/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab_observe::tests::{review_of, reviews_in};
    use crate::releases::entity::{ReleaseChange, ReleaseState};

    const GOOD: &str = "tool() {\n  case \"$1\" in\n    --version) echo \"tool 1.2.0\" ;;\n    *) return 2 ;;\n  esac\n}\n";
    /// A bashism: `${1,,}` is bash's. sh and dash refuse it.
    const BASHISM: &str =
        "tool() {\n  if [ \"${1,,}\" = \"--version\" ]; then echo \"tool 1.2.0\"; fi\n}\n";
    /// The pool this machine is, under the name the tests give it.
    const LOCAL: &str = "this-machine";

    fn release_of(change: &str) -> Release {
        Release {
            id: "rel-1".into(),
            project: "p".into(),
            repo: "r".into(),
            version: "1.2.0".into(),
            channel: "stable".into(),
            base: None,
            lane: "main".into(),
            from: "abc".into(),
            state: ReleaseState::Candidate,
            number: None,
            notes: String::new(),
            run: None,
            changes: vec![ReleaseChange {
                change: change.into(),
                title: "the change".into(),
                review_run: Some(format!("rv-{change}")),
            }],
            artifacts: Vec::new(),
            not_run: Vec::new(),
            verdicts: Vec::new(),
            promotions: Vec::new(),
            sync: Vec::new(),
            store_steps: Vec::new(),
            compatibility: Vec::new(),
            created_ms: 0,
            updated_ms: 0,
        }
    }

    const CLI: &str =
        "[cli]\ncommand = [\"tool\", \"--version\"]\nload = \"tool.sh\"\nexpect = \"tool 1.2.0\"\n";

    /// Runs the matrix's shell cells for real in `dir` with `script` as the CLI, and reports them as the pool would.
    fn run_cli(
        dir: &std::path::Path,
        matrix: &Matrix,
        pools: &[Pool],
        script: &str,
    ) -> Vec<Outcome> {
        std::fs::write(dir.join("tool.sh"), script).unwrap();
        compat::expand(matrix)
            .iter()
            .filter(|cell| matches!(compat::place(cell, pools), Placement::On(_)))
            .map(|cell| compat::run_shell(cell, dir))
            .collect()
    }

    /// This machine's pool, under the name the tests give it.
    fn local() -> Pool {
        compat::local_pool(LOCAL)
    }

    /// The other system than this machine's: its cells have no pool here.
    fn other_system() -> &'static str {
        compat::SYSTEMS
            .iter()
            .copied()
            .find(|name| *name != compat::local_system())
            .expect("the matrix names more than one system")
    }

    fn has(pool: &Pool, shell: &str) -> bool {
        pool.capabilities.contains(&format!("shell:{shell}"))
    }

    #[test]
    fn a_cli_release_passes_in_the_shells_that_run_and_names_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let matrix = compat::parse(CLI).unwrap();
        let pools = [local()];
        let outcomes = run_cli(dir.path(), &matrix, &pools, GOOD);
        let cells = assess(&reviews, &release_of("c1"), &matrix, &pools, &outcomes, &[]);
        let system = compat::local_system();
        for shell in ["bash", "zsh", "sh", "dash"] {
            if has(&pools[0], shell) {
                let cell = cells
                    .iter()
                    .find(|c| c.cell == format!("cli:{shell}:{system}"))
                    .unwrap();
                assert_eq!(cell.state, "passed", "{shell}: {}", cell.evidence);
                assert_eq!(cell.pool, LOCAL);
            }
        }
        if !has(&pools[0], "fish") {
            let fish = cells
                .iter()
                .find(|c| c.cell == format!("cli:fish:{system}"))
                .unwrap();
            assert_eq!(fish.state, "not_run");
            assert_eq!(fish.reason, "no pool has shell:fish");
        }
        let other = other_system();
        let elsewhere = cells
            .iter()
            .find(|c| c.cell == format!("cli:bash:{other}"))
            .unwrap();
        assert_eq!(elsewhere.state, "not_run");
        assert!(
            elsewhere.reason.contains(&format!("os:{other}")),
            "{}",
            elsewhere.reason
        );
        assert!(cells.iter().all(|c| c.state != "failed"), "{cells:?}");
        if has(&pools[0], "bash") {
            assert_eq!(verdict(&cells), "passed");
        }
        assert_eq!(
            reviews.run("rv-c1").unwrap().findings().count(),
            0,
            "nothing failed, nothing filed"
        );
    }

    #[test]
    fn a_bashism_fails_in_sh_and_dash_and_is_filed_once_on_the_change() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let matrix = compat::parse(CLI).unwrap();
        let pools = [local()];
        let outcomes = run_cli(dir.path(), &matrix, &pools, BASHISM);
        let release = release_of("c1");
        let cells = assess(&reviews, &release, &matrix, &pools, &outcomes, &[]);
        let system = compat::local_system();
        for shell in ["sh", "dash"] {
            if !has(&pools[0], shell) {
                continue;
            }
            let cell = cells
                .iter()
                .find(|c| c.cell == format!("cli:{shell}:{system}"))
                .unwrap();
            assert_eq!(cell.state, "failed", "{shell}: {}", cell.evidence);
            let (review, id) = cell.finding.split_once('/').expect("the failure is filed");
            let finding = reviews
                .run(review)
                .unwrap()
                .findings()
                .find(|f| f.id == id)
                .expect("on the change's review")
                .clone();
            assert_eq!(finding.kind, KIND);
            assert_eq!(finding.verdict, FindingVerdict::Confirmed);
            assert!(
                finding.proof.to_lowercase().contains("bad substitution"),
                "{}",
                finding.proof
            );
        }
        if has(&pools[0], "bash") {
            let bash = cells
                .iter()
                .find(|c| c.cell == format!("cli:bash:{system}"))
                .unwrap();
            assert_eq!(
                bash.state, "passed",
                "bash runs the bashism: {}",
                bash.evidence
            );
        }
        // Assessed again, the failure is not filed twice.
        let again = assess(&reviews, &release, &matrix, &pools, &outcomes, &cells);
        assert_eq!(again, cells);
        let filed = cells.iter().filter(|c| !c.finding.is_empty()).count();
        assert_eq!(reviews.run("rv-c1").unwrap().findings().count(), filed);
    }

    #[test]
    fn a_library_check_not_run_here_and_planned_cells_say_why_and_never_pass() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let system = compat::local_system();
        let matrix = compat::parse(&format!(
            "[library]\n[web]\nbrowsers = [\"chromium\"]\n[desktop]\nplatforms = [\"{system}\"]\n[mobile]\nplatforms = [\"ios\"]\n",
        ))
        .unwrap();
        let pools = [local()];
        // No runner reports anything: each cell is placed or not, and none passes.
        let cells = assess(&reviews, &release_of("c1"), &matrix, &pools, &[], &[]);
        let api = cells
            .iter()
            .find(|c| c.cell == "api:cargo-semver-checks")
            .unwrap();
        assert_eq!(api.state, "not_run");
        let chromium = cells.iter().find(|c| c.cell == "web:chromium").unwrap();
        assert_eq!(chromium.reason, "no pool has browser:chromium");
        let install = cells
            .iter()
            .find(|c| c.cell == format!("desktop:{}:install", compat::local_system()))
            .unwrap();
        assert_eq!(install.pool, LOCAL);
        assert!(
            install.reason.starts_with("no runner for desktop cells"),
            "{}",
            install.reason
        );
        let ios = cells
            .iter()
            .find(|c| c.cell == "mobile:ios:install")
            .unwrap();
        assert_eq!(ios.reason, "no pool has simulator");
        assert_eq!(verdict(&cells), "not_run");
    }

    #[test]
    fn a_failure_with_no_change_is_recorded_without_a_finding() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        let matrix = compat::parse(CLI).unwrap();
        let pools = [local()];
        let outcomes = run_cli(dir.path(), &matrix, &pools, BASHISM);
        let mut release = release_of("c1");
        release.changes.clear();
        let cells = assess(&reviews, &release, &matrix, &pools, &outcomes, &[]);
        assert!(cells.iter().all(|c| c.finding.is_empty()));
        if has(&pools[0], "sh") {
            assert!(cells.iter().any(|c| c.state == "failed"));
        }
    }
}
