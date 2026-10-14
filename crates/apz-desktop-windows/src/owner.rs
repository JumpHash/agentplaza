//! Whether the owner is using the desktop, from the system's last-input time.

/// How long after the owner's last input the desktop still counts as theirs.
pub const OWNER_QUIET_MS: u32 = 3_000;

/// Whether the owner is at the desktop now.
///
/// `last_input` is the tick of the newest input of any kind (GetLastInputInfo's `dwTime`). Our own injected
/// input counts as input too, so `last_injected` is the tick this backend read right after its newest injection.
/// The owner is active when their input is newer than ours and within [`OWNER_QUIET_MS`] of `now` (GetTickCount).
/// Ticks wrap every 49.7 days, so every comparison takes a difference.
///
/// Check this before injecting: after an injection the system's last input is ours, and an earlier owner input
/// no longer shows.
pub fn owner_is_active(last_input: u32, last_injected: Option<u32>, now: u32) -> bool {
    if now.wrapping_sub(last_input) > OWNER_QUIET_MS {
        return false;
    }
    match last_injected {
        None => true,
        Some(injected) => (last_input.wrapping_sub(injected) as i32) > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_input_with_no_injection_is_the_owners() {
        assert!(owner_is_active(1_000, None, 1_500));
    }

    #[test]
    fn old_input_is_not_the_owners_any_more() {
        assert!(!owner_is_active(1_000, None, 1_000 + OWNER_QUIET_MS + 1));
        assert!(owner_is_active(1_000, None, 1_000 + OWNER_QUIET_MS));
    }

    #[test]
    fn our_own_injection_is_not_the_owner() {
        assert!(!owner_is_active(5_000, Some(5_000), 5_100));
    }

    #[test]
    fn owner_input_after_our_injection_is_the_owner() {
        assert!(owner_is_active(5_200, Some(5_000), 5_300));
    }

    #[test]
    fn owner_input_after_our_injection_goes_quiet_after_the_window() {
        assert!(!owner_is_active(
            5_100,
            Some(5_000),
            5_100 + OWNER_QUIET_MS + 1
        ));
    }

    #[test]
    fn ticks_wrap_around() {
        let injected = u32::MAX - 5;
        let input = 3;
        assert!(owner_is_active(input, Some(injected), 10));
        assert!(!owner_is_active(injected, Some(injected), 10));
    }
}
