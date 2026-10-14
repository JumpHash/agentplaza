//! First-run setup for **Start locally** and **Host a hub**: the checks, the fixes and their
//! containers live in the `apz-setup` crate (shared with the CLI). The chooser (`boot.rs`) uses
//! them through this module, which keeps the `console::setup` path it has always had.

pub use apz_setup::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_waits_for_every_required_check_and_not_for_the_optional_ones() {
        let row = |need, state, required| Check {
            need,
            label: "row",
            state,
            detail: String::new(),
            fix: None,
            required,
        };
        assert!(ready(&[]));
        assert!(
            ready(&[row(Need::Garage, State::Missing, false)]),
            "Garage is optional"
        );
        assert!(!ready(&[row(Need::Docker, State::Missing, true)]));
    }
}
