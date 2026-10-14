//! PTY terminals: a shell on a pseudo-terminal, served through open, write, read, resize, close and list.
//!
//! A terminal's shell is a login shell (`/bin/bash -l`, else `/bin/sh`) on the slave of a new PTY, in a new
//! session, so the slave is its controlling terminal. A reader thread per terminal drains the master into a
//! ring of the last [`RING_CAP`] bytes. Every byte has an offset counted from the terminal's start, so a
//! reader that polls from its last `next` sees each byte once; an offset the ring has dropped moves on to
//! the oldest byte kept, and the reply says how many bytes were lost.
//!
//! A terminal lives until it is closed or the table is dropped. One with no read, write or resize for
//! [`IDLE`] is closed in the background. Closing hangs the master up, gives the shell a grace period to
//! exit, then kills the session's process group.
//!
//! Unix uses a PTY and Windows a ConPTY (`powershell.exe -NoLogo`, else `cmd.exe`, in a job object). The
//! ring, the table, the idle janitor and the argument rules are shared in the private `table` module. Elsewhere [`Terminals`]
//! refuses every call as unavailable.

use serde::Serialize;

/// What an open returned: the terminal's name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermOpened {
    pub term: String,
}

/// What a write took. `ok` is false when not everything went in before the write's wait ran out; the
/// caller sends the rest from `written` on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermWritten {
    pub ok: bool,
    pub written: usize,
}

/// One read of a terminal. `exit_code` is `None` while the shell runs, and 128 plus the signal when a
/// signal ended it. `lost` counts the bytes the ring dropped before `offset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermRead {
    pub data_b64: String,
    pub exit_code: Option<i32>,
    pub exited: bool,
    pub lost: u64,
    pub next: u64,
    pub offset: u64,
    pub term: String,
}

/// What a resize set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermResized {
    pub cols: u16,
    pub rows: u16,
    pub term: String,
}

/// What a close did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermClosed {
    pub closed: bool,
    pub term: String,
}

/// One open terminal, as a list shows it. `started_ms` is the Unix time the shell started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermInfo {
    pub cols: u16,
    pub exited: bool,
    pub rows: u16,
    pub started_ms: u64,
    pub term: String,
}

/// The open terminals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermList {
    pub terminals: Vec<TermInfo>,
}

#[cfg(any(unix, windows))]
mod table;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::Terminals;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Terminals;

#[cfg(not(any(unix, windows)))]
mod other;
#[cfg(not(any(unix, windows)))]
pub use other::Terminals;
