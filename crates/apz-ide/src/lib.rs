//! The IDE's shared model: what a workspace is, what its tree and buffers look
//! like, how a unified diff becomes hunks, which paths may be touched, and the
//! tab and panel state. The desktop app and the web console run the same code
//! (the web through `apz-ide-web`).
//!
//! * [`workspace`] — `agent:<cid>` and `repo:<name>@<ref>` identities.
//! * [`tree`] — entries, one page of a directory listing, child order.
//! * [`buffer`] — an open file's text against the hash it was read at.
//! * [`diff`] — `git diff` and `diff -u` output into a [`diff::ChangeSet`], and the reverse: two texts
//!   written as a git unified diff.
//! * [`path`] — relative-path rules and the secret deny list.
//! * [`state`] — the tab, panel and base reducer.
//!
//! No I/O, no iced, no wasm-bindgen. `serde` only.

#![forbid(unsafe_code)]

pub mod buffer;
pub mod diff;
pub mod path;
pub mod state;
pub mod tree;
pub mod workspace;

pub use buffer::Buffer;
pub use diff::{
    ChangeSet, DiffError, FileChange, Hunk, Line, LineKind, Stats, Status, parse_unified, write_binary,
    write_unified,
};
pub use path::{PathError, check_dir, check_path, check_read, check_write, is_secret};
pub use state::{Action, Base, IdeState, Panel, reduce};
pub use tree::{Entry, EntryKind, PAGE_SIZE, TreePage, changed_filter, children};
pub use workspace::{IdError, Kind, Workspace, WorkspaceId};
