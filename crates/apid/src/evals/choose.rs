//! Which engine and which model a step with `engine: any` or `model: eval` takes (PLAN 25.4, 13.2): the best by the
//! eval records of its step type, within the step's budget and the model policy. Pure folds over the scores
//! (`evals.rs`), so the rules are tested here without a database.
//!
//! A candidate is trusted once it has the policy's minimum of runs. Among the trusted, the best has the highest success
//! rate, then the lower cost per success, then the lower time per success; a tie keeps the policy's order. A candidate
//! short of the minimum is explored instead when the run falls in the explore share: the run's number is hashed, so
//! the same run always makes the same choice. A candidate whose expected cost per run (its average over its runs) is
//! more than what the step may still spend is skipped; a candidate with no runs has no expected cost yet, so it is
//! not skipped.

use super::Score;

/// The choice a run records (`eval_run.choice`): the rule it followed.
pub const EXPLOITED: &str = "eval:exploited";
pub const EXPLORED: &str = "eval:explored";
pub const UNTRUSTED: &str = "eval:untrusted";
pub const ONLY: &str = "eval:only";
/// A model a recipe names outright, a model list's first allowed model, and the seat's own choice.
pub const FIXED: &str = "fixed";
pub const LISTED: &str = "list";
pub const SEAT: &str = "seat";

/// What one candidate's runs of a step did, summed over the scores that name it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tally {
    pub key: String,
    pub runs: u64,
    pub successes: u64,
    /// Micro-dollars over all the runs.
    pub cost_micros: u64,
    /// Milliseconds from start to end over all the runs.
    pub wall_ms: u64,
}

impl Tally {
    fn success_rate(&self) -> f64 {
        self.successes as f64 / self.runs.max(1) as f64
    }

    /// Micro-dollars per success; None without a success.
    fn cost_per_success(&self) -> Option<f64> {
        (self.successes > 0).then(|| self.cost_micros as f64 / self.successes as f64)
    }

    /// Milliseconds per success; None without a success.
    fn time_per_success(&self) -> Option<f64> {
        (self.successes > 0).then(|| self.wall_ms as f64 / self.successes as f64)
    }

    /// Micro-dollars per run: the expected cost of the candidate's next run; None without a run.
    fn cost_per_run(&self) -> Option<f64> {
        (self.runs > 0).then(|| self.cost_micros as f64 / self.runs as f64)
    }

    /// Whether the candidate's expected cost fits in `budget_left` (micro-dollars). Uncapped, or unscored: it fits.
    fn fits(&self, budget_left: Option<u64>) -> bool {
        match (budget_left, self.cost_per_run()) {
            (Some(left), Some(cost)) => cost <= left as f64,
            _ => true,
        }
    }

    /// The candidate in words, for a choice's reason.
    fn describe(&self) -> String {
        let cost = self
            .cost_per_success()
            .map_or("no success".to_string(), |c| {
                format!("${:.4} per success", c / 1e6)
            });
        let time = self
            .time_per_success()
            .map_or(String::new(), |t| format!(", {:.1}s per success", t / 1e3));
        format!(
            "{} succeeds {:.0}% of {} runs, {cost}{time}",
            self.key,
            self.success_rate() * 100.0,
            self.runs
        )
    }
}

/// The policy a choice follows: the runs a candidate needs before its score is trusted, and the share of runs (in
/// percent) that explore an under-sampled candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub min_samples: u64,
    pub explore_pct: u32,
}

/// The candidate a choice picked, the rule it followed and the reason in words (scores, samples, budget).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub key: String,
    pub rule: &'static str,
    pub reason: String,
}

/// Each candidate's tally over `scores`, in the candidates' order. `key` names a score's candidate: its model or its
/// engine. A score whose key is no candidate does not count, so a disallowed model's runs never help a choice.
pub fn tally(scores: &[Score], candidates: &[String], key: impl Fn(&Score) -> &str) -> Vec<Tally> {
    candidates
        .iter()
        .map(|candidate| {
            let mut tally = Tally {
                key: candidate.clone(),
                ..Tally::default()
            };
            for score in scores.iter().filter(|s| key(s) == candidate.as_str()) {
                tally.runs += score.runs;
                tally.successes += score.successes;
                tally.cost_micros += score.cost_micros;
                tally.wall_ms += score.wall_ms;
            }
            tally
        })
        .collect()
}

/// The choice among `tallies` for run `run`: None when no candidate fits the budget.
pub fn choose(
    tallies: &[Tally],
    budget_left: Option<u64>,
    policy: Policy,
    run: u64,
) -> Option<Pick> {
    let affordable: Vec<&Tally> = tallies.iter().filter(|t| t.fits(budget_left)).collect();
    if affordable.is_empty() {
        return None;
    }
    let skipped: Vec<&str> = tallies
        .iter()
        .filter(|t| !t.fits(budget_left))
        .map(|t| t.key.as_str())
        .collect();
    let note = if skipped.is_empty() {
        String::new()
    } else {
        format!("; skipped over the step's budget: {}", skipped.join(", "))
    };
    let mut trusted: Vec<&Tally> = Vec::new();
    let mut under: Vec<&Tally> = Vec::new();
    for tally in affordable {
        if tally.runs >= policy.min_samples {
            trusted.push(tally);
        } else {
            under.push(tally);
        }
    }
    // The least sampled first; a tie keeps the policy's order (the sort is stable).
    under.sort_by_key(|t| t.runs);
    if trusted.is_empty() {
        let pick = under[0];
        return Some(Pick {
            key: pick.key.clone(),
            rule: UNTRUSTED,
            reason: format!(
                "no candidate has the {} runs its score needs yet; {} has {}{note}",
                policy.min_samples, pick.key, pick.runs
            ),
        });
    }
    if let Some(pick) = under.first()
        && explored(run, policy.explore_pct)
    {
        return Some(Pick {
            key: pick.key.clone(),
            rule: EXPLORED,
            reason: format!(
                "run {run} is in the {}% explore share; {} has {} of the {} runs its score needs{note}",
                policy.explore_pct, pick.key, pick.runs, policy.min_samples
            ),
        });
    }
    let best = trusted
        .iter()
        .copied()
        .reduce(|best, t| if better(t, best) { t } else { best })?;
    let rivals = trusted.len() - 1;
    Some(Pick {
        key: best.key.clone(),
        rule: EXPLOITED,
        reason: format!(
            "best of {} trusted: {}; {rivals} other trusted{note}",
            trusted.len(),
            best.describe()
        ),
    })
}

/// The engine a step runs on when it names `engine: any`: the one of `engines` whose runs of `step` did best. A single
/// engine is the only one there is, and needs no scores.
pub fn engine(
    scores: &[Score],
    step: &str,
    engines: &[String],
    budget_left: Option<u64>,
    policy: Policy,
    run: u64,
) -> Option<Pick> {
    if let [only] = engines {
        return Some(Pick {
            key: only.clone(),
            rule: ONLY,
            reason: format!("{only} is the only engine a seat runs on"),
        });
    }
    let of_step: Vec<Score> = scores.iter().filter(|s| s.step == step).cloned().collect();
    choose(
        &tally(&of_step, engines, |s| s.engine.as_str()),
        budget_left,
        policy,
        run,
    )
}

/// The model a step takes when it names `model: eval`: the one of `models` (the policy's models for the seat's tier)
/// whose runs of `step` on `engine` did best.
pub fn model(
    scores: &[Score],
    step: &str,
    engine: &str,
    models: &[String],
    budget_left: Option<u64>,
    policy: Policy,
    run: u64,
) -> Option<Pick> {
    let of_step: Vec<Score> = scores
        .iter()
        .filter(|s| s.step == step && s.engine == engine)
        .cloned()
        .collect();
    choose(
        &tally(&of_step, models, |s| s.model.as_deref().unwrap_or("")),
        budget_left,
        policy,
        run,
    )
}

/// Whether run `run` is explored: its number, hashed, falls in the first `pct` of a hundred.
pub fn explored(run: u64, pct: u32) -> bool {
    key_of_u64(run) % 100 < u64::from(pct.min(100))
}

/// A stable key for a text (FNV-1a): the same text always gives the same number, on every build.
pub fn key_of(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A number mixed well enough to spread consecutive run numbers over the hundred shares (splitmix64).
fn key_of_u64(value: u64) -> u64 {
    let mut x = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Whether `a` is a better choice than `b`: the higher success rate, then the lower cost per success, then the lower
/// time per success. An equal one is not better, so the earlier candidate keeps its place.
fn better(a: &Tally, b: &Tally) -> bool {
    let (rate_a, rate_b) = (a.successes * b.runs, b.successes * a.runs);
    if rate_a != rate_b {
        return rate_a > rate_b;
    }
    if let Some(order) = lower(a.cost_per_success(), b.cost_per_success()) {
        return order;
    }
    lower(a.time_per_success(), b.time_per_success()).unwrap_or(false)
}

/// `Some(a < b)` when the two differ (a figure without a value is the worst), else None.
fn lower(a: Option<f64>, b: Option<f64>) -> Option<bool> {
    match (a, b) {
        (Some(x), Some(y)) if x != y => Some(x < y),
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: Policy = Policy {
        min_samples: 5,
        explore_pct: 10,
    };
    const NO_EXPLORE: Policy = Policy {
        min_samples: 5,
        explore_pct: 0,
    };

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// A score of `model` (on `engine`) for step `step`.
    fn score(step: &str, engine: &str, model: &str, runs: u64, successes: u64, cost: u64) -> Score {
        Score {
            step: step.into(),
            seat: "builder".into(),
            model: Some(model.into()),
            engine: engine.into(),
            runs,
            successes,
            success_rate: successes as f64 / runs.max(1) as f64,
            cost_per_success_micros: (successes > 0).then(|| cost as f64 / successes as f64),
            time_per_success_ms: (successes > 0).then(|| 1000.0 * runs as f64 / successes as f64),
            quality: 0.0,
            cost_micros: cost,
            wall_ms: 1000 * runs,
        }
    }

    #[test]
    fn the_trusted_best_wins_and_a_tie_goes_to_the_cheaper_then_the_policy_order() {
        let models = names(&["a/fast", "b/strong", "c/other"]);
        let scores = [
            score("build", "harness", "a/fast", 10, 6, 600),
            score("build", "harness", "b/strong", 10, 9, 900),
            score("build", "harness", "c/other", 10, 9, 300),
        ];
        // Equal success rates: the cheaper per success wins.
        let pick = model(&scores, "build", "harness", &models, None, POLICY, 1).unwrap();
        assert_eq!(pick.key, "c/other");
        assert_eq!(pick.rule, EXPLOITED);
        assert!(pick.reason.contains("best of 3 trusted"), "{}", pick.reason);
        // Equal rate and cost: the policy's order keeps the first.
        let tie = [
            score("build", "harness", "a/fast", 10, 9, 300),
            score("build", "harness", "b/strong", 10, 9, 300),
        ];
        let pick = model(
            &tie,
            "build",
            "harness",
            &names(&["a/fast", "b/strong"]),
            None,
            NO_EXPLORE,
            1,
        );
        assert_eq!(pick.unwrap().key, "a/fast");
        // Another step's runs do not count.
        assert!(
            model(&scores, "review", "harness", &models, None, POLICY, 1)
                .is_none_or(|p| p.rule != EXPLOITED)
        );
    }

    #[test]
    fn a_candidate_short_of_the_minimum_is_explored_at_the_share_and_never_exploited() {
        let models = names(&["a/proven", "b/new"]);
        let scores = [
            score("build", "harness", "a/proven", 20, 18, 2000),
            score("build", "harness", "b/new", 2, 2, 20),
        ];
        let mut explored_runs = 0;
        for run in 1..=1000u64 {
            let pick = model(&scores, "build", "harness", &models, None, POLICY, run).unwrap();
            // The same run always makes the same choice.
            assert_eq!(
                model(&scores, "build", "harness", &models, None, POLICY, run),
                Some(pick.clone())
            );
            if pick.rule == EXPLORED {
                assert_eq!(pick.key, "b/new", "the under-sampled one is explored");
                explored_runs += 1;
            } else {
                assert_eq!(pick.key, "a/proven", "b/new is never exploited at 2 runs");
            }
        }
        assert!(
            (60..=140).contains(&explored_runs),
            "about 10% of 1000 runs explore, got {explored_runs}"
        );
        // Without a trusted candidate, the least sampled takes the runs.
        let fresh = [score("build", "harness", "a/proven", 1, 1, 10)];
        let pick = model(
            &fresh,
            "build",
            "harness",
            &names(&["a/proven", "z/none"]),
            None,
            POLICY,
            7,
        )
        .unwrap();
        assert_eq!(pick.rule, UNTRUSTED);
        assert_eq!(pick.key, "z/none");
        // A zero share never explores.
        let none = (1..=200u64).all(|run| {
            model(&scores, "build", "harness", &models, None, NO_EXPLORE, run)
                .unwrap()
                .rule
                == EXPLOITED
        });
        assert!(none);
    }

    #[test]
    fn a_disallowed_model_is_never_picked_however_well_it_did() {
        // The policy's models for the tier are the candidates; `c/outside` is in no list, so its runs never count.
        let models = names(&["a/fast", "b/strong"]);
        let scores = [
            score("build", "harness", "a/fast", 10, 5, 500),
            score("build", "harness", "b/strong", 10, 6, 600),
            score("build", "harness", "c/outside", 50, 50, 1),
        ];
        for run in 1..=300u64 {
            let pick = model(&scores, "build", "harness", &models, None, POLICY, run).unwrap();
            assert_ne!(pick.key, "c/outside");
        }
        let pick = model(&scores, "build", "harness", &models, None, POLICY, 3).unwrap();
        assert_eq!(pick.key, "b/strong");
        // With nothing allowed there is nothing to pick.
        assert_eq!(
            model(
                &scores,
                "build",
                "harness",
                &names(&["d/none"]),
                None,
                POLICY,
                1
            )
            .map(|p| p.key),
            Some("d/none".into())
        );
        assert!(model(&scores, "build", "harness", &[], None, POLICY, 1).is_none());
    }

    #[test]
    fn an_over_budget_candidate_is_skipped_and_nothing_affordable_picks_nothing() {
        let models = names(&["a/dear", "b/cheap"]);
        let scores = [
            // Expected cost per run: 500 micro-dollars.
            score("build", "harness", "a/dear", 10, 10, 5000),
            // 100 micro-dollars per run.
            score("build", "harness", "b/cheap", 10, 8, 1000),
        ];
        let pick = model(&scores, "build", "harness", &models, Some(300), POLICY, 1).unwrap();
        assert_eq!(
            pick.key, "b/cheap",
            "a/dear costs more than the step has left"
        );
        assert!(
            pick.reason
                .contains("skipped over the step's budget: a/dear"),
            "{}",
            pick.reason
        );
        // A budget that fits the dear one again takes it: it is the better score.
        let pick = model(&scores, "build", "harness", &models, Some(600), POLICY, 1).unwrap();
        assert_eq!(pick.key, "a/dear");
        // Nothing fits: no choice.
        assert!(model(&scores, "build", "harness", &models, Some(50), POLICY, 1).is_none());
        // A candidate with no runs has no expected cost, so it is not skipped by one.
        let fresh = model(
            &scores[..1],
            "build",
            "harness",
            &names(&["a/dear", "n/new"]),
            Some(300),
            NO_EXPLORE,
            1,
        );
        assert!(fresh.is_some_and(|p| p.key == "n/new"));
    }

    #[test]
    fn engine_any_picks_the_engine_whose_runs_did_best_the_same_way() {
        let engines = names(&["harness", "claude"]);
        let scores = [
            score("build", "harness", "a/fast", 10, 5, 500),
            score("build", "claude", "a/fast", 10, 9, 900),
            score("review", "claude", "a/fast", 10, 10, 1),
        ];
        let pick = engine(&scores, "build", &engines, None, POLICY, 4).unwrap();
        assert_eq!(pick.key, "claude");
        assert_eq!(pick.rule, EXPLOITED);
        // Its model is then chosen among its own runs.
        let on_claude = model(
            &scores,
            "build",
            "claude",
            &names(&["a/fast"]),
            None,
            POLICY,
            4,
        )
        .unwrap();
        assert_eq!(on_claude.key, "a/fast");
        // One engine needs no scores.
        let only = engine(&[], "build", &names(&["harness"]), None, POLICY, 4).unwrap();
        assert_eq!((only.key.as_str(), only.rule), ("harness", ONLY));
    }

    #[test]
    fn the_explore_share_is_a_fixed_function_of_the_run_number() {
        assert!((1..=1000u64).all(|run| !explored(run, 0)));
        assert!((1..=1000u64).all(|run| explored(run, 100)));
        let share = (1..=10_000u64).filter(|run| explored(*run, 10)).count();
        assert!(
            (900..=1100).contains(&share),
            "about 10% of 10000, got {share}"
        );
        assert_eq!(key_of("builder/1#1"), key_of("builder/1#1"));
        assert_ne!(key_of("builder/1#1"), key_of("builder/1#2"));
    }
}
