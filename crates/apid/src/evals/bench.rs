//! The recipe benchmark (PLAN 25.3): an archived task with a known outcome is replayed in a fork of its base at the commit
//! the task started from. A session works in the fork on the task's goal, and the fork's check decides whether the
//! replay passed. The replay is scored against the known outcome: a task that landed must pass its check, a reverted one
//! must not. The session is a seam: a booted hub supplies its scripted session (the mock session of the booted tests),
//! and the fork is the test fork (`user_test/fork.rs`) where a VM can run; [`CloneFork`] is the stand-in fork when none
//! can (a clone of the repository at the commit, in a scratch folder).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use super::Archived;

/// A fork of a base at a commit: a workspace holding the base as it was at that commit.
pub trait Fork {
    fn at(&self, commit: &str) -> Result<PathBuf, String>;
}

/// What a session spent: micro-dollars and tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub cost_micros: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// A session that works in a workspace on a goal. It edits the workspace and reports what it spent.
pub trait Session {
    fn work(&self, workspace: &Path, goal: &str) -> Result<Usage, String>;
}

/// The stand-in fork: a clone of the repository at `repo` (a path to a local repository), checked out at the commit in
/// a fresh folder under `root`.
pub struct CloneFork {
    pub repo: PathBuf,
    pub root: PathBuf,
}

impl Fork for CloneFork {
    fn at(&self, commit: &str) -> Result<PathBuf, String> {
        let dir = self.root.join(format!("fork-{}", commit.replace('/', "_")));
        let _ = std::fs::remove_dir_all(&dir);
        git(
            &self.root,
            &["clone", "--quiet"],
            &[&self.repo.to_string_lossy(), &dir.to_string_lossy()],
        )?;
        git(&dir, &["checkout", "--quiet", "--detach", commit], &[])?;
        Ok(dir)
    }
}

/// Runs `git <args> <rest>` in `dir`, failing with its stderr.
fn git(dir: &Path, args: &[&str], rest: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .args(rest)
        .output()
        .map_err(|e| format!("git did not run: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// One replay, scored: whether the check passed (`passed` or `failed`), the known outcome (`landed` or `reverted`), and
/// whether they agree. The session's spend and the replay's wall time come with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub outcome: String,
    pub known: String,
    pub matched: bool,
    pub cost_micros: u64,
    pub wall_ms: u64,
}

/// Replays `case` (an archived task) in a fork at its start commit, through `session`, and scores the replay against the
/// case's known outcome. An error is a replay that could not run (no fork, no session); a failed check is a score.
pub fn replay_case(
    case: &Archived,
    fork: &dyn Fork,
    session: &dyn Session,
) -> Result<Replay, String> {
    let started = Instant::now();
    let workspace = fork.at(&case.start)?;
    let usage = session.work(&workspace, &case.goal)?;
    let passed = check_passes(&workspace, &case.check)?;
    let expected = case.outcome == "landed";
    Ok(Replay {
        outcome: if passed { "passed" } else { "failed" }.into(),
        known: case.outcome.clone(),
        matched: passed == expected,
        cost_micros: usage.cost_micros,
        wall_ms: started.elapsed().as_millis() as u64,
    })
}

/// Whether the check command passes in the workspace (its shell exits 0).
fn check_passes(workspace: &Path, check: &str) -> Result<bool, String> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(check)
        .current_dir(workspace)
        .status()
        .map_err(|e| format!("the check did not run: {e}"))?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// A scratch folder with a repository of two commits: the first holds a broken `value.txt`, the second fixes it.
    /// Returns (folder, the first commit, the second commit).
    fn repo() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("base");
        std::fs::create_dir_all(&repo).unwrap();
        let commit = |message: &str| {
            let out = Command::new("git")
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "bench")
                .env("GIT_AUTHOR_EMAIL", "bench@example.invalid")
                .env("GIT_COMMITTER_NAME", "bench")
                .env("GIT_COMMITTER_EMAIL", "bench@example.invalid")
                .args(["commit", "--quiet", "--allow-empty", "-m", message])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let rev = Command::new("git")
                .current_dir(&repo)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&rev.stdout).trim().to_string()
        };
        let git_init = Command::new("git")
            .current_dir(&repo)
            .args(["init", "--quiet"])
            .output()
            .unwrap();
        assert!(git_init.status.success());
        std::fs::write(repo.join("value.txt"), "value=0\n").unwrap();
        Command::new("git")
            .current_dir(&repo)
            .args(["add", "value.txt"])
            .output()
            .unwrap();
        let first = commit("the broken base");
        std::fs::write(repo.join("value.txt"), "value=1\n").unwrap();
        Command::new("git")
            .current_dir(&repo)
            .args(["add", "value.txt"])
            .output()
            .unwrap();
        let second = commit("the fix");
        (dir, first, second)
    }

    /// The archived case: the task started at `start`, its change landed, and its check is that the value is fixed.
    fn case(start: &str, outcome: &str) -> Archived {
        Archived {
            id: 1,
            project: "pr1".into(),
            task: "t1".into(),
            start: start.into(),
            goal: "set value to 1".into(),
            check: "grep -qx value=1 value.txt".into(),
            outcome: outcome.into(),
            archived_ms: 0,
        }
    }

    /// The scripted session (the mock session): it writes the fix the task's goal asks for, and spends a fixed amount.
    struct Scripted {
        seen_goal: Cell<bool>,
    }

    impl Session for Scripted {
        fn work(&self, workspace: &Path, goal: &str) -> Result<Usage, String> {
            if goal.contains("value") {
                self.seen_goal.set(true);
                std::fs::write(workspace.join("value.txt"), "value=1\n")
                    .map_err(|e| e.to_string())?;
            }
            Ok(Usage {
                cost_micros: 250,
                tokens_in: 900,
                tokens_out: 120,
            })
        }
    }

    /// A session that does nothing: the base stays broken.
    struct Idle;

    impl Session for Idle {
        fn work(&self, _workspace: &Path, _goal: &str) -> Result<Usage, String> {
            Ok(Usage::default())
        }
    }

    #[test]
    fn a_landed_task_replays_in_a_fork_at_its_start_commit_and_its_check_passes() {
        let (dir, first, _second) = repo();
        let fork = CloneFork {
            repo: dir.path().join("base"),
            root: dir.path().join("forks"),
        };
        std::fs::create_dir_all(&fork.root).unwrap();
        let session = Scripted {
            seen_goal: Cell::new(false),
        };
        let replay = replay_case(&case(&first, "landed"), &fork, &session).unwrap();
        assert!(
            session.seen_goal.get(),
            "the session worked on the goal in the fork"
        );
        assert_eq!(
            (
                replay.outcome.as_str(),
                replay.known.as_str(),
                replay.matched
            ),
            ("passed", "landed", true),
            "{replay:?}"
        );
        assert_eq!(replay.cost_micros, 250);
        // The fork is a clone at the start commit: the broken base is what the session started from.
        let workspace = fork.at(&first).unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("value.txt")).unwrap(),
            "value=0\n"
        );
    }

    #[test]
    fn a_replay_that_leaves_the_base_broken_disagrees_with_a_landed_outcome() {
        let (dir, first, _second) = repo();
        let fork = CloneFork {
            repo: dir.path().join("base"),
            root: dir.path().join("forks"),
        };
        std::fs::create_dir_all(&fork.root).unwrap();
        let replay = replay_case(&case(&first, "landed"), &fork, &Idle).unwrap();
        assert_eq!(
            (replay.outcome.as_str(), replay.matched),
            ("failed", false),
            "a task that landed must pass its check: {replay:?}"
        );
    }

    #[test]
    fn a_reverted_task_is_scored_as_matched_when_its_replay_fails_its_check() {
        let (dir, first, second) = repo();
        let fork = CloneFork {
            repo: dir.path().join("base"),
            root: dir.path().join("forks"),
        };
        std::fs::create_dir_all(&fork.root).unwrap();
        // The archived case reverted: its check must not pass, and the idle replay leaves it failing.
        let replay = replay_case(&case(&first, "reverted"), &fork, &Idle).unwrap();
        assert!(replay.matched, "{replay:?}");
        // From the fixed commit the check passes, so a reverted case's replay there disagrees.
        let fixed = replay_case(&case(&second, "reverted"), &fork, &Idle).unwrap();
        assert_eq!((fixed.outcome.as_str(), fixed.matched), ("passed", false));
    }

    #[test]
    fn a_fork_at_a_commit_that_is_not_there_is_an_error_not_a_score() {
        let (dir, _first, _second) = repo();
        let fork = CloneFork {
            repo: dir.path().join("base"),
            root: dir.path().join("forks"),
        };
        std::fs::create_dir_all(&fork.root).unwrap();
        assert!(
            replay_case(
                &case("0000000000000000000000000000000000000000", "landed"),
                &fork,
                &Idle
            )
            .is_err()
        );
    }
}
