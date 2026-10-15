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
    fn owner_activity_follows_input_age_and_our_injections() {
        // (case, last input tick, tick of our last injection, now, whether the owner is active)
        for (case, last_input, last_injected, now, want) in [
            (
                "recent input with no injection is the owner's",
                1_000,
                None,
                1_500,
                true,
            ),
            (
                "input past the quiet window is not the owner's any more",
                1_000,
                None,
                1_000 + OWNER_QUIET_MS + 1,
                false,
            ),
            (
                "input exactly at the quiet window's edge is still the owner's",
                1_000,
                None,
                1_000 + OWNER_QUIET_MS,
                true,
            ),
            (
                "our own injection is not the owner",
                5_000,
                Some(5_000),
                5_100,
                false,
            ),
            (
                "owner input after our injection is the owner",
                5_200,
                Some(5_000),
                5_300,
                true,
            ),
            (
                "owner input after our injection goes quiet after the window",
                5_100,
                Some(5_000),
                5_100 + OWNER_QUIET_MS + 1,
                false,
            ),
            (
                "ticks wrap around: owner input after our injection is the owner",
                3,
                Some(u32::MAX - 5),
                10,
                true,
            ),
            (
                "ticks wrap around: our own injection is not the owner",
                u32::MAX - 5,
                Some(u32::MAX - 5),
                10,
                false,
            ),
        ] {
            assert_eq!(
                owner_is_active(last_input, last_injected, now),
                want,
                "{case}"
            );
        }
    }
}
