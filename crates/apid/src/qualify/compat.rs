//! The compatibility and install-and-upgrade matrices, as stage 9's compatibility part reads them (PLAN 22.9, 22.10).
//!
//! The candidate's matrix is read from the release's own commit in Weft (`.plaza/compatibility.toml` at `release.from`),
//! expanded into cells, and each cell is placed on a pool that has its capabilities (`releases::compat::assess`). The
//! hub runs the cells it can run itself: a shell cell runs its declared command in its shell, in a scratch directory
//! holding the release's declared load file, and an API cell runs `cargo semver-checks` against the previous qualified
//! release's revision when that tool is installed. Every other placed cell is reported `not run` with its reason, and a
//! cell that no pool can run is `not run` too. The judgement (passed, failed or not run) is the matrix's verdict.

use std::path::{Component, Path, PathBuf};

use apz_topology::compat::{self, FILE, Kind, Outcome, Placement, Pool};

use crate::releases::entity::Release;
use crate::releases::flow::Deps;

use super::plan::Stage;
use super::result::{Evidence, Finding, StageResult, State};

/// The compatibility part of stage 9 for a candidate: what the qualification asks of the matrix.
pub trait CompatPort {
    /// The part's result for `release` at `now_ms`.
    fn judge(&self, release: &Release, now_ms: u64) -> StageResult;
}

/// Why no matrix is judged: the project does not declare one in the release's commit.
const UNDECLARED: &str = "the project declares no .plaza/compatibility.toml";

/// The compatibility and install-and-upgrade part of stage 9 for `release` (10.10). `pools` are the pools the cells may be
/// placed on, and `local` names the pool this hub runs its cells on; `baseline` is the revision of the previous qualified
/// release (the API check compares with it).
pub fn compatibility_adapter(
    d: &Deps,
    release: &Release,
    pools: &[Pool],
    local: &str,
    baseline: Option<&str>,
    now_ms: u64,
) -> StageResult {
    let rev = release.from.as_str();
    let listed = match d.weft.read(&format!("/revisions/{rev}/files")) {
        Ok(listing) => listing,
        Err(why) => {
            return not_run(
                now_ms,
                &format!("the release's files could not be read: {why}"),
            );
        }
    };
    let declared = listed["files"]
        .as_array()
        .is_some_and(|files| files.iter().any(|f| f["path"] == FILE));
    if !declared {
        return not_run(now_ms, UNDECLARED);
    }
    let text = match fetch(d, rev, FILE) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
        Err(why) => return not_run(now_ms, &format!("{FILE} could not be read: {why}")),
    };
    let matrix = match compat::parse(&text) {
        Ok(matrix) => matrix,
        Err(why) => return failed(now_ms, &format!("{FILE}: {why}")),
    };
    let cells = compat::expand(&matrix);
    let scratch = scratch_dir(&release.id);
    let outcomes = match run_here(d, rev, pools, local, baseline, &cells, &scratch) {
        Ok(outcomes) => outcomes,
        Err(why) => {
            let _ = std::fs::remove_dir_all(&scratch);
            return failed(now_ms, &why);
        }
    };
    let _ = std::fs::remove_dir_all(&scratch);
    let judged = crate::releases::compat::assess(
        d.reviews,
        release,
        &matrix,
        pools,
        &outcomes,
        &release.compatibility,
    );
    let verdict = crate::releases::compat::verdict(&judged);
    let _ = d
        .releases
        .update(&release.id, |r| r.compatibility = judged.clone());
    let mut result = match verdict {
        "failed" => {
            let failing: Vec<&str> = judged
                .iter()
                .filter(|c| c.state == "failed")
                .map(|c| c.cell.as_str())
                .collect();
            let mut r = StageResult::new(
                Stage::Ux,
                State::Failed,
                &format!(
                    "{} compatibility cell(s) failed: {}",
                    failing.len(),
                    failing.join(", ")
                ),
                now_ms,
            );
            r.findings = judged
                .iter()
                .filter(|c| c.state == "failed")
                .map(|c| Finding {
                    id: format!("compat:{}", c.cell),
                    text: format!("{} failed on {}: {}", c.cell, c.pool, c.evidence),
                    href: String::new(),
                })
                .collect();
            r
        }
        "passed" => StageResult::new(Stage::Ux, State::Passed, "", now_ms),
        _ => {
            let reasons: Vec<String> = judged
                .iter()
                .map(|c| format!("{}: {}", c.cell, c.reason))
                .collect();
            StageResult::new(
                Stage::Ux,
                State::NotRun,
                &format!("not run: {}", reasons.join("; ")),
                now_ms,
            )
        }
    };
    result.evidence = judged
        .iter()
        .filter(|c| c.state == "passed" || c.state == "failed")
        .map(|c| Evidence {
            kind: "compatibility".into(),
            label: format!("{} on {}: {}", c.cell, c.pool, c.state),
            href: String::new(),
        })
        .collect();
    result.evidence.extend(
        judged
            .iter()
            .filter(|c| c.state == "not_run")
            .map(|c| Evidence {
                kind: "not run".into(),
                label: format!("{}: {}", c.cell, c.reason),
                href: String::new(),
            }),
    );
    result
}

/// Runs the cells this hub can run and reports the rest: the outcome of each placed cell, the shell cells' outcomes in
/// the scratch directory the release's load files are written to.
fn run_here(
    d: &Deps,
    rev: &str,
    pools: &[Pool],
    local: &str,
    baseline: Option<&str>,
    cells: &[compat::Cell],
    scratch: &Path,
) -> Result<Vec<Outcome>, String> {
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let mut outcomes = Vec::new();
    let mut loaded: Vec<String> = Vec::new();
    let mut tree_ready = false;
    for cell in cells {
        let Placement::On(pool) = compat::place(cell, pools) else {
            continue;
        };
        if matches!(cell.kind, Kind::Planned { .. }) {
            continue;
        }
        if pool != local {
            outcomes.push(Outcome::not_run(
                &cell.id,
                format!("{pool} takes its cells on a KVM host: not run from this hub"),
            ));
            continue;
        }
        match &cell.kind {
            Kind::Shell { load, .. } => {
                if let Some(path) = load.as_deref().filter(|p| !loaded.iter().any(|l| l == p)) {
                    fetch_into(d, rev, path, scratch)?;
                    loaded.push(path.to_string());
                }
                outcomes.push(compat::run_shell(cell, scratch));
            }
            Kind::Api { method } => {
                if method != "cargo-semver-checks" || !tool_on_path(method) {
                    outcomes.push(compat::run_api(cell, scratch, ""));
                    continue;
                }
                let Some(baseline) = baseline else {
                    outcomes.push(Outcome::not_run(
                        &cell.id,
                        "no previous qualified release to compare the API with".into(),
                    ));
                    continue;
                };
                if !tree_ready {
                    materialize_tree(d, rev, scratch)?;
                    tree_ready = true;
                }
                outcomes.push(compat::run_api(cell, scratch, baseline));
            }
            Kind::Planned { .. } => {}
        }
    }
    Ok(outcomes)
}

/// The bytes of `path` at revision `rev`.
fn fetch(d: &Deps, rev: &str, path: &str) -> Result<Vec<u8>, String> {
    d.weft.bytes(&format!("/revisions/{rev}/file?path={path}"))
}

/// Writes the file `path` of revision `rev` under `dir`, at the same relative path. A path that leaves `dir` is refused.
fn fetch_into(d: &Deps, rev: &str, path: &str, dir: &Path) -> Result<(), String> {
    let relative =
        safe_relative(path).ok_or_else(|| format!("{path} is not a path in the repository"))?;
    let bytes = fetch(d, rev, path)?;
    let target = dir.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&target, bytes).map_err(|e| e.to_string())
}

/// Writes every file of revision `rev` under `dir` (an API check reads the whole tree).
fn materialize_tree(d: &Deps, rev: &str, dir: &Path) -> Result<(), String> {
    let listing = d.weft.read(&format!("/revisions/{rev}/files"))?;
    for file in listing["files"].as_array().into_iter().flatten() {
        if let Some(path) = file["path"].as_str() {
            fetch_into(d, rev, path, dir)?;
        }
    }
    Ok(())
}

/// A repository path as a relative path with no `..` or root (None when it is not one).
fn safe_relative(path: &str) -> Option<PathBuf> {
    let relative = Path::new(path);
    let clean = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)));
    (clean && !path.is_empty()).then(|| relative.to_path_buf())
}

/// A scratch directory for one assessment of a release: unique to the assessment (two may run at once).
fn scratch_dir(release: &str) -> PathBuf {
    let nonce: String = apz_trust::random::<8>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    std::env::temp_dir().join(format!(
        "apid-compat-{release}-{}-{nonce}",
        std::process::id()
    ))
}

fn tool_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

fn not_run(now_ms: u64, reason: &str) -> StageResult {
    StageResult::new(
        Stage::Ux,
        State::NotRun,
        &format!("not run: {reason}"),
        now_ms,
    )
}

fn failed(now_ms: u64, reason: &str) -> StageResult {
    StageResult::new(Stage::Ux, State::Failed, reason, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::releases::flow;
    use crate::releases::tests::{FakeBuilder, World, cut, lab_with};

    #[cfg(unix)]
    const GOOD: &str = "tool() {\n  case \"$1\" in\n    --version) echo \"tool 1.2.0\" ;;\n    *) return 2 ;;\n  esac\n}\n";
    /// A bashism: `${1,,}` is bash's. sh and dash refuse it.
    #[cfg(unix)]
    const BASHISM: &str =
        "tool() {\n  if [ \"${1,,}\" = \"--version\" ]; then echo \"tool 1.2.0\"; fi\n}\n";
    #[cfg(unix)]
    const CLI: &str =
        "[cli]\ncommand = [\"tool\", \"--version\"]\nload = \"tool.sh\"\nexpect = \"tool 1.2.0\"\n";
    const LOCAL: &str = "this-machine";

    /// A candidate of a Weft whose main carries `files` (the release's commit holds them).
    fn candidate_with(name: &str, files: &[(&str, &str)]) -> (World, Release, apz_http::Handle) {
        let (client, handle, _dir, _change, _agent) = lab_with(name, files);
        let world = World::new(client, name);
        let release = {
            let d = world.deps();
            let drafted = flow::draft(&d, "app", "app", &cut("v0.1.0")).unwrap();
            flow::candidate(&d, &drafted.id, &FakeBuilder, Some(1)).unwrap()
        };
        (world, release, handle)
    }

    fn pools() -> Vec<Pool> {
        vec![compat::local_pool(LOCAL)]
    }

    fn adapter(world: &World, release: &Release) -> StageResult {
        let d = world.deps();
        compatibility_adapter(&d, release, &pools(), LOCAL, None, 1)
    }

    // The cells run POSIX shells, which a Windows pool does not have.
    #[cfg(unix)]
    #[test]
    fn a_shell_cli_whose_cells_run_here_passes_the_compatibility_part() {
        let (world, release, _handle) =
            candidate_with("compat-pass", &[(FILE, CLI), ("tool.sh", GOOD)]);
        let part = adapter(&world, &release);
        assert_eq!(part.state, State::Passed, "{part:?}");
        // The shells on this machine ran for real: at least bash, sh and zsh are among the evidence.
        let ran: Vec<&str> = part.evidence.iter().map(|e| e.label.as_str()).collect();
        let system = compat::local_system();
        for shell in ["bash", "sh", "zsh"] {
            if compat::local_pool(LOCAL)
                .capabilities
                .contains(&format!("shell:{shell}"))
            {
                assert!(
                    ran.iter()
                        .any(|l| l.starts_with(&format!("cli:{shell}:{system}"))),
                    "{shell} ran: {ran:?}"
                );
            }
        }
        assert!(part.findings.is_empty(), "{part:?}");
    }

    // The cells run POSIX shells, which a Windows pool does not have.
    #[cfg(unix)]
    #[test]
    fn a_bashism_fails_the_compatibility_part_in_sh_and_names_the_cell() {
        let (world, release, _handle) =
            candidate_with("compat-fail", &[(FILE, CLI), ("tool.sh", BASHISM)]);
        let part = adapter(&world, &release);
        // `sh` is on every Unix this runs on, and it is bash on a Mac but dash on Linux: each of sh and dash that is here
        // refuses the bashism and names its own cell.
        let system = compat::local_system();
        let local = compat::local_pool(LOCAL);
        let refusing: Vec<&str> = ["sh", "dash"]
            .into_iter()
            .filter(|shell| local.capabilities.contains(&format!("shell:{shell}")))
            .collect();
        assert!(
            refusing.contains(&"sh"),
            "sh is here: {:?}",
            local.capabilities
        );
        assert_eq!(part.state, State::Failed, "{part:?}");
        for shell in refusing {
            let id = format!("compat:cli:{shell}:{system}");
            let finding = part
                .findings
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("{id} is named: {part:?}"));
            assert!(
                finding.text.to_lowercase().contains("bad substitution"),
                "the shell's own complaint is the evidence: {part:?}"
            );
        }
    }

    #[test]
    fn a_project_with_no_matrix_is_not_run_with_the_reason() {
        let (world, release, _handle) = candidate_with("compat-none", &[]);
        let part = adapter(&world, &release);
        assert_eq!(part.state, State::NotRun);
        assert!(part.reason.contains(UNDECLARED), "{part:?}");
        assert!(!part.state.passes());
    }

    #[test]
    fn a_matrix_no_pool_can_run_is_not_run_and_never_passes() {
        let (world, release, _handle) = candidate_with(
            "compat-web",
            &[(FILE, "[web]\nbrowsers = [\"chromium\"]\n")],
        );
        let part = adapter(&world, &release);
        assert_eq!(part.state, State::NotRun, "{part:?}");
        assert!(
            part.reason.contains("no pool has browser:chromium"),
            "{part:?}"
        );
    }
}
