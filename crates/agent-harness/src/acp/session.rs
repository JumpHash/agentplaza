//! One engine session: the adapter process, the ACP handshake, one prompt turn, and the client
//! methods the engine calls back while the turn runs.
//!
//! The adapter starts as the agent (`bash::command`, `Privilege::Agent`), in its own process group,
//! with stdin and stdout as the ACP connection (`apz_acp::rpc::Peer`) and stderr kept in a small
//! ring for errors. The harness advertises `clientCapabilities {fs: {readTextFile, writeTextFile},
//! terminal: true}` and `clientInfo {name: "agentplaza", version}`, and requires protocol version 1.
//!
//! While the turn runs, the engine's requests are answered here:
//! - `session/request_permission`: [`gate::permission_check`] → [`EngineLink::decide`] →
//!   [`gate::permission_answer`];
//! - `fs/*` and `terminal/*`: [`gate::client_check`] → `decide` (a denial is a JSON-RPC error with
//!   the hub's reason) → [`fs`] / [`terminal::Terminals`]. Before the turn's first write the
//!   workspace is checkpointed (`crate::checkpoints::snapshot`, label `engine <id>`), so the owner
//!   can restore the engine's whole turn.
//!
//! A cancel (the link's `cancelled()`) or the deadline sends `session/cancel`; open permission
//! requests are answered `cancelled`; after [`CANCEL_GRACE`] the process group is killed. The process
//! group is always killed when the session ends.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::catalog::Entry;
use super::link::EngineLink;
use super::stream::Turn;
#[allow(unused_imports)]
use super::{fs, gate, terminal};
use crate::files::Workspace;

/// How long a cancelled engine gets to end its turn before its process group is killed.
pub const CANCEL_GRACE: Duration = Duration::from_secs(2);
/// How long `initialize` and `session/new` may take (the adapter starting node and its SDK).
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// The stderr kept for errors, in bytes.
pub const STDERR_BYTES: usize = 8 * 1024;

/// How to start and drive one engine session.
pub struct Options<'w> {
    pub entry: &'static Entry,
    pub workspace: &'w Workspace,
    /// The folder the engine works in (absolute, inside the workspace): the process's cwd and the
    /// ACP session's `cwd`.
    pub cwd: PathBuf,
    /// The program and its arguments, environment included (`/usr/bin/env HOME=… KEY=… binary args…`).
    pub argv: Vec<String>,
    /// The model the hub allowed, when the call named one (sent as the catalogue says).
    pub model: Option<String>,
    /// The session mode to select, when the call named one: refused when the engine does not offer
    /// it or the catalogue refuses it.
    pub mode: Option<String>,
    /// A session to continue: `session/resume` when the engine advertises it, else `session/load`
    /// when it advertises `loadSession`, else a new session.
    pub resume: Option<String>,
    /// What the engine is told first when `resume` could not be honoured and a new session was
    /// opened instead: the previous turn's summary, put before the prompt as its own text block.
    pub context: Option<String>,
    /// When the turn must be over; then it is cancelled.
    pub deadline: Instant,
}

/// How one prompt turn went.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The ACP session id (to continue with).
    pub session: String,
    /// True when `resume` was honoured (resumed or loaded); false when a new session was opened.
    pub continued: bool,
    /// `end_turn`, `max_tokens`, `max_turn_requests`, `refusal` or `cancelled`.
    pub stop_reason: String,
    pub turn: Turn,
    /// Commands run through the client's terminals, with exit codes.
    pub commands: Vec<(String, Option<i32>)>,
    /// The checkpoint taken before the turn's first write, if it wrote.
    pub checkpoint: Option<String>,
    /// The modes the engine offered (ids), for the summary.
    pub modes: Vec<String>,
}

/// Starts the engine, opens or continues a session, runs one prompt turn with `prompt`, and ends
/// the process. Errors are the engine's (it failed to start, refused the protocol version, crashed;
/// with the stderr tail) or the harness's.
pub fn prompt_turn(
    options: &Options,
    prompt: &str,
    link: &dyn EngineLink,
) -> Result<Outcome, String> {
    let _ = (options, prompt, link);
    Err("engine sessions are not built yet".to_string())
}
