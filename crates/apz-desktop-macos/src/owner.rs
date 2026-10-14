//! Whether the owner is using the machine right now.
//!
//! The platform reports the seconds since the last input event of any kind (the HID system state), and the seconds
//! since this backend last injected an event into the session. Input after our last injection is the owner's, as long
//! as it is recent.

/// Input within this many seconds counts as the owner using the machine.
pub const OWNER_WINDOW_SECS: f64 = 2.0;

/// Allowance for the difference between the time we posted an event and the time the system records it.
pub const SLACK_SECS: f64 = 0.25;

/// Whether the owner's input is the most recent, and recent enough to count.
///
/// `idle_secs` is the time since the last input of any kind. `since_our_input_secs` is the time since this session's
/// last injected event, or `None` when the agent has not injected anything yet. With no injection, any recent input is
/// the owner's.
pub fn owner_is_active(idle_secs: f64, since_our_input_secs: Option<f64>) -> bool {
    // A NaN, a negative time or one outside the window is not recent input (NaN is in no range).
    if !(0.0..OWNER_WINDOW_SECS).contains(&idle_secs) {
        return false;
    }
    match since_our_input_secs {
        None => true,
        Some(ours) => idle_secs + SLACK_SECS < ours,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_recent_input_means_the_owner_is_away() {
        assert!(!owner_is_active(30.0, None));
        assert!(!owner_is_active(30.0, Some(60.0)));
        assert!(!owner_is_active(f64::NAN, None));
        assert!(!owner_is_active(-1.0, None));
    }

    #[test]
    fn recent_input_with_no_injection_is_the_owners() {
        assert!(owner_is_active(0.3, None));
    }

    #[test]
    fn input_after_our_last_event_is_the_owners() {
        // We posted 10 s ago, the owner moved 0.5 s ago.
        assert!(owner_is_active(0.5, Some(10.0)));
    }

    #[test]
    fn our_own_event_as_the_last_input_is_not_the_owner() {
        // We posted 0.1 s ago, and that is the most recent input.
        assert!(!owner_is_active(0.1, Some(0.1)));
        // The input was ours even though the system recorded it a moment later.
        assert!(!owner_is_active(0.2, Some(0.3)));
    }

    #[test]
    fn owner_input_before_our_event_does_not_count() {
        // The owner moved 1.0 s ago, then we posted 0.2 s ago: our event is the latest.
        assert!(!owner_is_active(1.0, Some(0.2)));
    }
}
