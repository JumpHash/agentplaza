//! Bounded runs: a command started in a root, its output kept as a tail, polled from an offset, and
//! ended at its exit, its timeout or a stop. Unix runs `sh -c <command>`; Windows runs
//! `cmd.exe /S /C "<command>"`.
//!
//! A start returns at once, so a caller never holds a request past its own deadline. Each run is kept
//! from outliving its request: on unix it is in its own process group, and on Windows it is in a job
//! object, and either one is ended as a whole at the timeout, on a stop, and when the shell exits. At
//! most [`MAX_RUNS`] runs are kept, and a finished one stays for ten minutes to be polled.
//!
//! The run table (tail, registry, start, poll and stop rules) is shared in the private `table` module. Elsewhere [`Runs`]
//! refuses every call as unavailable: a run that could not be ended as a whole would be worse than no run.

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

#[cfg(any(unix, windows))]
mod table;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::Runs;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Runs;

#[cfg(not(any(unix, windows)))]
mod other;
#[cfg(not(any(unix, windows)))]
pub use other::Runs;
