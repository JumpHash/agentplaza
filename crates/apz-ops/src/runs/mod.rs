//! Bounded runs: `sh -c <command>` started in a root, its output kept as a tail, polled from an
//! offset, and ended at its exit, its timeout or a stop.
//!
//! A start returns at once, so a caller never holds a request past its own deadline. Each run is in its
//! own process group, so nothing a run started outlives it: the group is killed at the timeout, on a stop,
//! and when the shell exits. At most [`MAX_RUNS`] runs are kept, and a finished one stays for
//! ten minutes to be polled.
//!
//! Unix only. Elsewhere [`Runs`] refuses every call as unavailable: a process group cannot be ended
//! there yet, and a run that outlived its request would be worse than no run.

use serde::Serialize;

/// Runs kept at once.
pub const MAX_RUNS: usize = 4;
/// A run's timeout when the caller gives none, in seconds.
pub const DEFAULT_TIMEOUT_S: u64 = 120;
/// The longest timeout a run may ask for, in seconds.
pub const MAX_TIMEOUT_S: u64 = 600;
/// The output a run keeps. Older bytes are dropped, and a poll says where it starts.
pub const OUTPUT_CAP: usize = 1024 * 1024;

/// What a start returned: the run's id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunStarted {
    pub run: String,
}

/// One poll of a run. `next` is the offset to poll from next. `exit_code` is `None` while the run is
/// running, and when a signal ended it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunPoll {
    pub done: bool,
    pub exit_code: Option<i32>,
    pub next: u64,
    pub offset: u64,
    pub output: String,
    pub run: String,
    pub timed_out: bool,
}

/// What a stop did. `stopped` is true when the run was still running when the stop came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunStopped {
    pub done: bool,
    pub run: String,
    pub stopped: bool,
}

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::Runs;

#[cfg(not(unix))]
mod other;
#[cfg(not(unix))]
pub use other::Runs;
