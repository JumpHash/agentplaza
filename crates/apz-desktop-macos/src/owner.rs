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
    fn owner_is_active_only_for_recent_input_newer_than_ours() {
        // Each case: idle seconds, seconds since our last event, and whether the owner is active.
        for (case, idle, ours, want) in [
            ("no recent input means the owner is away", 30.0, None, false),
            (
                "no recent input after our event means away",
                30.0,
                Some(60.0),
                false,
            ),
            (
                "NaN is in no range, so not recent input",
                f64::NAN,
                None,
                false,
            ),
            ("a negative time is not recent input", -1.0, None, false),
            (
                "recent input with no injection is the owner's",
                0.3,
                None,
                true,
            ),
            // We posted 10 s ago, the owner moved 0.5 s ago.
            (
                "input after our last event is the owner's",
                0.5,
                Some(10.0),
                true,
            ),
            // We posted 0.1 s ago, and that is the most recent input.
            (
                "our own event as the last input is not the owner",
                0.1,
                Some(0.1),
                false,
            ),
            // The input was ours even though the system recorded it a moment later.
            (
                "our event recorded a moment later is still ours",
                0.2,
                Some(0.3),
                false,
            ),
            // The owner moved 1.0 s ago, then we posted 0.2 s ago: our event is the latest.
            (
                "owner input before our event does not count",
                1.0,
                Some(0.2),
                false,
            ),
        ] {
            assert_eq!(owner_is_active(idle, ours), want, "{case}");
        }
    }
}
