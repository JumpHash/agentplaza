//! How a VM left the running state, read off the last lines of its serial.
//!
//! The state the daemon reports (`exited`) says a VM stopped; it does not say
//! *how*. The guest's own console does: the x86_64 guest leaves through ACPI
//! S5, where the kernel prints `reboot: Power down`, and a guest-initiated
//! reboot prints `machine restart` / `Restarting system`. Everything else —
//! a halt in place, a kill, a capture that has not reached a shutdown — is
//! [`ExitKind::Unknown`], and saying so is better than guessing.
//!
//! This mirrors the web console's derivation (`apps/web/src/models/run.ts`),
//! with the three kinds the register draws.

/// How many of a capture's final lines are examined. A shutdown's last words
/// are within a breath of the end; anything older belongs to an earlier boot.
pub const EXIT_SCAN_LINES: usize = 12;

/// How a VM left the running state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExitKind {
    /// The guest powered itself off (ACPI S5).
    Slept,
    /// The guest rebooted itself and the VM was reset.
    Reset,
    /// No shutdown was observed in the tail.
    Unknown,
}

impl ExitKind {
    /// The kind as the register names it.
    pub fn label(self) -> &'static str {
        match self {
            ExitKind::Slept => "slept",
            ExitKind::Reset => "reset",
            ExitKind::Unknown => "unknown",
        }
    }

    /// The word a stamp presses for the kind.
    pub fn stamp_label(self) -> &'static str {
        match self {
            ExitKind::Slept => "SLEPT",
            ExitKind::Reset => "RESET",
            ExitKind::Unknown => "—",
        }
    }
}

/// Derives the kind from a serial capture: the newest of the scan window's
/// final lines that carries a shutdown signal decides.
pub fn exit_kind(capture: &str) -> ExitKind {
    let lines: Vec<&str> = capture.lines().collect();
    let start = lines.len().saturating_sub(EXIT_SCAN_LINES);
    for line in lines[start..].iter().rev() {
        let line = line.to_ascii_lowercase();
        if line.contains("power down") {
            return ExitKind::Slept;
        }
        if line.contains("restarting system") || line.contains("machine restart") {
            return ExitKind::Reset;
        }
    }
    ExitKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_power_down_tail_is_a_sleep() {
        assert_eq!(
            exit_kind("plaza guest: churn done\nreboot: Power down\n"),
            ExitKind::Slept
        );
        assert_eq!(exit_kind("reboot: Power down"), ExitKind::Slept);
    }

    #[test]
    fn a_restart_tail_is_a_reset() {
        assert_eq!(exit_kind("machine restart\n"), ExitKind::Reset);
        assert_eq!(exit_kind("Restarting system\n"), ExitKind::Reset);
    }

    #[test]
    fn the_signals_are_case_insensitive() {
        assert_eq!(exit_kind("REBOOT: POWER DOWN\n"), ExitKind::Slept);
        assert_eq!(exit_kind("restarting SYSTEM\n"), ExitKind::Reset);
        assert_eq!(exit_kind("Machine Restart\n"), ExitKind::Reset);
    }

    #[test]
    fn a_carriage_return_does_not_hide_a_signal() {
        assert_eq!(exit_kind("$ poweroff\r\nreboot: Power down\r\n"), ExitKind::Slept);
    }

    #[test]
    fn the_newest_signal_in_the_window_decides() {
        // A reboot, then a later poweroff: the guest slept.
        assert_eq!(
            exit_kind("Restarting system\nreboot: Power down\n"),
            ExitKind::Slept
        );
        // A poweroff, then a later reboot: the VM was reset.
        assert_eq!(
            exit_kind("reboot: Power down\nmachine restart\n"),
            ExitKind::Reset
        );
    }

    #[test]
    fn a_signal_older_than_the_window_is_not_an_exit() {
        let mut capture = String::from("reboot: Power down\n");
        for line in 0..EXIT_SCAN_LINES {
            capture.push_str(&format!("booting vm {line}\n"));
        }
        assert_eq!(
            exit_kind(&capture),
            ExitKind::Unknown,
            "a shutdown {EXIT_SCAN_LINES} lines back is an earlier boot"
        );
    }

    #[test]
    fn a_guest_that_parked_or_was_killed_is_unknown() {
        assert_eq!(exit_kind("plaza guest: parking forever\n"), ExitKind::Unknown);
        assert_eq!(exit_kind("killed by the host\n"), ExitKind::Unknown);
    }

    #[test]
    fn an_empty_or_partial_capture_is_unknown() {
        assert_eq!(exit_kind(""), ExitKind::Unknown);
        assert_eq!(exit_kind("login: plaza\n"), ExitKind::Unknown);
        assert_eq!(exit_kind("\n\n"), ExitKind::Unknown);
    }

    #[test]
    fn every_kind_has_a_label_and_a_stamp() {
        for kind in [ExitKind::Slept, ExitKind::Reset, ExitKind::Unknown] {
            assert!(!kind.label().is_empty());
            assert!(!kind.stamp_label().is_empty());
        }
        assert_eq!(ExitKind::Slept.label(), "slept");
        assert_eq!(ExitKind::Reset.stamp_label(), "RESET");
        assert_eq!(ExitKind::Unknown.label(), "unknown");
    }
}
