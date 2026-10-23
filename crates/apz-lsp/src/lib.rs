//! Language servers over stdio, for the code-intelligence tools (PLAN 15.2).
//!
//! - [`frame`]: the `Content-Length` framing of JSON-RPC.
//! - [`client`]: one server's connection: requests with timeouts, notifications, diagnostics, and the server's own
//!   requests answered so that it never waits on us.
//! - [`servers`]: the W40 languages, the server each uses, and how a path picks its language.
//! - [`pool`]: one server per (server, workspace root), started on the first query for its language and kept up until
//!   the pool drops, or reached on its warm socket when a base started it ([`bridge`], PLAN 3.8).
//! - [`limits`]: a memory and CPU cap per server where the OS has one: Linux sets `RLIMIT_AS` and `RLIMIT_CPU`; on
//!   macOS the cap does not bind, so servers run uncapped there ([`limits::ENFORCED`]).
//! - [`fake`]: a scripted server in this process, for the protocol tests here and for the tools' tests.

pub mod bridge;
pub mod client;
pub mod fake;
pub mod frame;
pub mod limits;
pub mod pool;
pub mod servers;
pub mod types;

use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub use client::Transport;
pub use limits::Limits;
pub use pool::{Checked, Pool};
pub use servers::{Language, ServerSpec, installed, which};

/// Whether the language's server is installed here (its command is on `PATH`).
pub fn is_installed(language: Language) -> bool {
    installed(language)
}
pub use types::{
    Diagnostic, DocumentSymbol, Location, Position, Range, path_to_uri, position_at, uri_to_path,
};

/// Why a question could not be put to a server, or its answer did not come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspError {
    /// No server for the language here: its command is not installed. The caller answers from elsewhere.
    NoServer { language: String, command: String },
    /// The path is not absolute, has no language, or its file cannot be read as text.
    File(String),
    /// The server did not answer within the time allowed for the method.
    Timeout { method: String },
    /// The server answered with an error.
    Rpc {
        method: String,
        code: i64,
        message: String,
    },
    /// The server stopped, or its connection closed; `detail` says what it printed last, when it printed anything.
    Closed { detail: String },
    /// Anything else in the process or the protocol.
    Protocol(String),
}

impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LspError::NoServer { language, command } => {
                write!(
                    f,
                    "no server for {language} here ({command} is not installed)"
                )
            }
            LspError::File(reason) => write!(f, "{reason}"),
            LspError::Timeout { method } => write!(f, "the server did not answer {method} in time"),
            LspError::Rpc {
                method,
                code,
                message,
            } => write!(f, "the server refused {method} ({code}): {message}"),
            LspError::Closed { detail } => write!(f, "the language server stopped: {detail}"),
            LspError::Protocol(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for LspError {}

/// How long each step may take. A server that has not answered by then is an error, never a hang.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// The `initialize` handshake: a server may index before it answers.
    pub start: Duration,
    /// Each request after that.
    pub request: Duration,
    /// How long a document's diagnostics may take to arrive after a change.
    pub diagnostics: Duration,
    /// How long a server gets to leave after `exit` before it is killed.
    pub shutdown: Duration,
    /// For this long after a server starts, an empty definition or reference answer is asked again: a server that
    /// is still indexing answers empty. After it, empty answers are final.
    pub warm_up: Duration,
    /// The wait between those asks.
    pub warm_retry: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            start: Duration::from_secs(120),
            request: Duration::from_secs(60),
            diagnostics: Duration::from_secs(10),
            shutdown: Duration::from_secs(3),
            warm_up: Duration::from_secs(30),
            warm_retry: Duration::from_secs(1),
        }
    }
}

/// A lock that keeps going past a poisoned one: the state a panicked holder left is still readable.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
