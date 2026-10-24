//! The functional tests of a qualification (PLAN 22.10): a failed test is re-run alone, up to twice; a test that both
//! passed and failed in its last ten runs is flaky; and a test that flaked in three runs within thirty days is quarantined
//! (left out of the gates, given a fix task, and listed in every report until it is fixed). Pure, with the clock given.

use serde::{Deserialize, Serialize};

/// How many re-runs a failed test gets, each on its own.
pub const RERUNS: u32 = 2;
/// How many of a test's latest runs decide whether it is flaky.
pub const WINDOW: usize = 10;
/// The window, in days, in which flaky runs count toward a quarantine.
pub const QUARANTINE_DAYS: u64 = 30;
/// How many flaky runs in the window quarantine a test.
pub const QUARANTINE_FLAKES: usize = 3;
const DAY_MS: u64 = 86_400_000;

/// One test's attempts in one run: the first run, then the re-runs it took, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempts {
    pub name: String,
    pub outcomes: Vec<bool>,
    /// Why a re-run did not happen (the runner could not take it), empty when every attempt was made.
    pub reason: String,
}

impl Attempts {
    /// The test passed in the run: some attempt passed.
    pub fn passed(&self) -> bool {
        self.outcomes.iter().any(|ok| *ok)
    }

    /// The test both passed and failed in the run (it passed on a re-run).
    pub fn flaked(&self) -> bool {
        self.passed() && self.outcomes.iter().any(|ok| !*ok)
    }
}

/// A test's first run is `first`; a failed test is re-run alone (`rerun(n)` is the n-th re-run's pass or fail), up to
/// [`RERUNS`] times, and stops at the first pass. A runner that cannot re-run stops the attempts with its reason.
pub fn run_with_reruns(
    name: &str,
    first: bool,
    rerun: &mut dyn FnMut(u32) -> Result<bool, String>,
) -> Attempts {
    let mut outcomes = vec![first];
    let mut reason = String::new();
    let mut n = 0;
    while !first && n < RERUNS && !outcomes.iter().any(|ok| *ok) {
        n += 1;
        match rerun(n) {
            Ok(ok) => outcomes.push(ok),
            Err(why) => {
                reason = why;
                break;
            }
        }
    }
    Attempts {
        name: name.to_string(),
        outcomes,
        reason,
    }
}

/// One run of a test in its history: whether it passed in the end, whether it flaked, and how many attempts it took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct HistoryRun {
    #[table(key)]
    pub run: String,
    pub at_ms: u64,
    pub passed: bool,
    pub flaked: bool,
    pub attempts: u32,
}

/// A test's history across the qualification runs it took part in, and its quarantine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct History {
    #[table(key)]
    pub project: String,
    #[table(key)]
    pub name: String,
    #[table(child = "qualify_test_run")]
    pub runs: Vec<HistoryRun>,
    /// When the test was quarantined (none when it was not).
    #[serde(default)]
    pub quarantined_ms: Option<u64>,
    /// When the owner or the fix cleared the quarantine, with the reason and the fix task.
    #[serde(default)]
    pub fixed_ms: Option<u64>,
    #[serde(default)]
    pub fix_reason: String,
    /// The task that fixes the test (filed when it was quarantined; empty until the engine files it).
    #[serde(default)]
    pub fix_task: String,
}

impl History {
    /// A new, empty history of a test.
    pub fn new(project: &str, name: &str) -> History {
        History {
            project: project.to_string(),
            name: name.to_string(),
            runs: Vec::new(),
            quarantined_ms: None,
            fixed_ms: None,
            fix_reason: String::new(),
            fix_task: String::new(),
        }
    }

    /// Records a test's attempts in run `run` at `at_ms`, replacing what the same run recorded before.
    pub fn record(&mut self, run: &str, at_ms: u64, attempts: &Attempts) {
        self.runs.retain(|r| r.run != run);
        self.runs.push(HistoryRun {
            run: run.to_string(),
            at_ms,
            passed: attempts.passed(),
            flaked: attempts.flaked(),
            attempts: attempts.outcomes.len() as u32,
        });
        self.runs.sort_by_key(|r| r.at_ms);
    }

    /// Whether the test is flaky now: among its last [`WINDOW`] runs it both passed and failed.
    pub fn flaky(&self) -> bool {
        let last: Vec<bool> = self
            .runs
            .iter()
            .rev()
            .take(WINDOW)
            .map(|r| r.passed)
            .collect();
        last.iter().any(|p| *p) && last.iter().any(|p| !*p)
    }

    /// The runs in the window of `now` in which the test flaked.
    pub fn flakes_within(&self, now: u64) -> usize {
        let since = now.saturating_sub(QUARANTINE_DAYS * DAY_MS);
        self.runs
            .iter()
            .filter(|r| r.flaked && r.at_ms >= since)
            .count()
    }

    /// Whether the test is quarantined now: quarantined, and not fixed since.
    pub fn quarantined(&self) -> bool {
        self.quarantined_ms.is_some() && self.fixed_ms.is_none()
    }

    /// Quarantines the test when it has flaked in [`QUARANTINE_FLAKES`] runs within the window (once). Returns whether it
    /// was quarantined just now, so the caller files its fix task once.
    pub fn quarantine_if_due(&mut self, now: u64) -> bool {
        if self.quarantined() || self.flakes_within(now) < QUARANTINE_FLAKES {
            return false;
        }
        self.quarantined_ms = Some(now);
        self.fixed_ms = None;
        true
    }

    /// Clears the quarantine: the test is fixed, with the reason and the task that did it.
    pub fn fixed(&mut self, now: u64, reason: &str) {
        self.fixed_ms = Some(now);
        self.fix_reason = reason.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempts(name: &str, outcomes: &[bool]) -> Attempts {
        Attempts {
            name: name.into(),
            outcomes: outcomes.to_vec(),
            reason: String::new(),
        }
    }

    #[test]
    fn a_failed_test_is_rerun_alone_up_to_twice_and_stops_at_a_pass() {
        let mut calls = Vec::new();
        let passed = run_with_reruns("t", false, &mut |n| {
            calls.push(n);
            Ok(n == 2)
        });
        assert_eq!(
            calls,
            vec![1, 2],
            "the first re-run failed, the second passed"
        );
        assert_eq!(passed.outcomes, vec![false, false, true]);
        assert!(passed.flaked());

        let mut count = 0;
        let never = run_with_reruns("t", false, &mut |_| {
            count += 1;
            Ok(false)
        });
        assert_eq!(count, 2, "two re-runs at most");
        assert!(!never.passed());
        assert!(
            !never.flaked(),
            "a test that never passed is failed, not flaky"
        );
    }

    #[test]
    fn a_passing_test_is_not_rerun_and_a_runner_that_cannot_rerun_says_why() {
        let mut count = 0;
        let ok = run_with_reruns("t", true, &mut |_| {
            count += 1;
            Ok(true)
        });
        assert_eq!((count, ok.outcomes.clone()), (0, vec![true]));
        let stopped = run_with_reruns("t", false, &mut |_| Err("no lab runner here".into()));
        assert_eq!(stopped.reason, "no lab runner here");
        assert!(!stopped.passed());
    }

    #[test]
    fn a_test_that_passed_and_failed_in_its_last_ten_runs_is_flaky() {
        let mut h = History::new("app", "cart::pay");
        for i in 0..10u64 {
            h.record(&format!("q{i}"), i * 1_000, &attempts("cart::pay", &[true]));
        }
        assert!(!h.flaky());
        h.record("q10", 10_000, &attempts("cart::pay", &[false]));
        assert!(
            h.flaky(),
            "a failure within the last ten runs next to passes"
        );
    }

    #[test]
    fn three_flaky_runs_in_thirty_days_quarantine_a_test_on_a_fake_clock() {
        let mut h = History::new("app", "cart::pay");
        let day = 86_400_000u64;
        for i in 0..3u64 {
            h.record(
                &format!("q{i}"),
                i * day,
                &attempts("cart::pay", &[false, true]),
            );
            assert_eq!(
                h.quarantine_if_due(i * day),
                i == 2,
                "quarantined on the third flake only"
            );
        }
        assert!(h.quarantined());
        h.fixed(40 * day, "the retry was missing");
        assert!(!h.quarantined());
    }

    #[test]
    fn flakes_older_than_thirty_days_do_not_count() {
        let mut h = History::new("app", "t");
        let day = 86_400_000u64;
        h.record("q1", 0, &attempts("t", &[false, true]));
        h.record("q2", day, &attempts("t", &[false, true]));
        h.record("q3", 31 * day, &attempts("t", &[false, true]));
        assert_eq!(
            h.flakes_within(31 * day),
            2,
            "day 0 is out of the window; days 1 and 31 are in it"
        );
        assert!(!h.quarantine_if_due(31 * day));
    }
}
