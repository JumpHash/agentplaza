//! The workspace operations the IDE and the agents share.
//!
//! The agent harness serves them to the owner inside an agent VM (the `ide_*` tools), and the
//! workstation daemon serves the same ones on a machine. Everything here works on a [`Root`], the
//! directory a caller is confined to, and returns plain serializable structs; the callers own the
//! transport and the JSON argument shapes.
//!
//! # What is here
//!
//! * [`files`]: `list`, `read`, `write` and `search` inside a root. A path goes through
//!   [`apz_ide::path`] first, then the real filesystem: the deepest existing ancestor is
//!   canonicalized and must land inside the canonical root, and a write never follows a symlink.
//!   A write takes [`WriteHooks`], so the caller can checkpoint before anything changes.
//! * [`runs`]: bounded shell runs (`start`, `poll`, `stop`), each kept to its own process group (unix)
//!   or job object (Windows). Elsewhere every call returns [`OpError::Unavailable`].
//! * [`terminal`]: terminals (`open`, `write`, `read`, `resize`, `close`, `list`) with a ring of output
//!   and an idle janitor: a PTY on unix, a ConPTY on Windows. Elsewhere every call is unavailable.
//! * [`page`]: the reply budget and the chunk boundaries every paged reply uses.
//!
//! No bound is clamped silently: a request over a cap is refused with [`OpError::LimitExceeded`],
//! and a reply that is cut short says so.
//!
//! The environment a started process gets is the caller's choice, passed as an [`Env`]. Nothing
//! here knows about a guest's proxy, PATH or uid policy.

// `unsafe` is confined to the platform modules that call libc or the Win32 API (`runs::unix`,
// `terminal::unix`, `runs::windows`, `terminal::windows` and the shared `win` helpers), each of which
// says what its calls rely on.
#![deny(unsafe_code)]

#[cfg(windows)]
mod win;

pub mod env;
pub mod error;
pub mod files;
pub mod page;
pub mod root;
pub mod runs;
pub mod terminal;

pub use apz_ide::tree::{Entry, EntryKind, PAGE_SIZE, TreePage};
pub use env::Env;
pub use error::OpError;
pub use files::{ReadReply, SearchMatch, SearchReply, WriteHooks, WriteReply};
pub use root::Root;
pub use runs::{RunPoll, RunStarted, RunStopped, Runs};
pub use terminal::{
    TermClosed, TermInfo, TermList, TermOpened, TermRead, TermResized, TermWritten, Terminals,
};
