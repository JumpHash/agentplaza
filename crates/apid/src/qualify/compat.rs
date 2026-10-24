//! The compatibility and install-and-upgrade matrices (PLAN 22.9, 22.10): the one adapter the compatibility part of stage 9
//! reads. Until 10.10's matrices are wired in, the adapter reports the part `not run` with its reason, so stage 9 never
//! passes on it. 10.10 fills [`compatibility_adapter`]: it takes the release, expands the release's matrices into cells,
//! and returns the part's result (passed, failed with its findings, or not run with the reason). Nothing else changes.

use crate::releases::entity::Release;

use super::plan::Stage;
use super::result::{StageResult, State};

/// Why the compatibility part does not run yet.
pub const LANDS_WITH: &str = "compatibility matrices land with 10.10";

/// The compatibility and install-and-upgrade part of stage 9 for `release`, as a stage result at `now_ms`. Until 10.10
/// fills it: not run, with [`LANDS_WITH`] as the reason.
pub fn compatibility_adapter(_release: &Release, now_ms: u64) -> StageResult {
    not_run(now_ms)
}

/// The part's result while the matrices are not wired in: not run, with the reason.
fn not_run(now_ms: u64) -> StageResult {
    StageResult::new(
        Stage::Ux,
        State::NotRun,
        &format!("not run: {LANDS_WITH}"),
        now_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_10_10_the_compatibility_part_is_not_run_with_its_reason() {
        let part = not_run(7);
        assert_eq!(part.state, State::NotRun);
        assert!(part.reason.contains(LANDS_WITH), "{part:?}");
        assert!(!part.state.passes());
    }
}
