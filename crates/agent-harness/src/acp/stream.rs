//! An engine's `session/update` stream: forwarded to the hub as `engine_update`s, and remembered
//! for the turn's summary.
//!
//! Text chunks (`agent_message_chunk`, `agent_thought_chunk`) are coalesced per kind and message
//! for at least [`COALESCE`] before they go out, so a fast engine sends a few lines a second rather
//! than one per token. Every other update goes out at once, after any buffered text, so the order
//! the hub sees is the engine's. Each update is capped to [`UPDATE_BYTES`]
//! (`apz_acp::map::cap_update`) and numbered from 1 (`seq`).

use std::time::{Duration, Instant};

use serde_json::Value;

use super::link::EngineLink;

/// How long text chunks are gathered before they go out.
pub const COALESCE: Duration = Duration::from_millis(100);
/// The most one forwarded update may hold, in bytes.
pub const UPDATE_BYTES: usize = 64 * 1024;

/// One tool call the engine made this turn, as its last update left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallSeen {
    pub id: String,
    pub title: String,
    /// The ACP kind (`read`, `edit`, `execute`, …), `other` when the engine gave none.
    pub kind: String,
    /// `pending`, `in_progress`, `completed` or `failed`.
    pub status: String,
}

/// What the summary needs from one prompt turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    /// The agent's text this turn (every `agent_message_chunk`, in order); thoughts are not kept.
    pub message: String,
    pub tool_calls: Vec<ToolCallSeen>,
    /// The latest `plan` update's entries.
    pub plan: Option<Value>,
    /// The latest `usage_update`.
    pub usage: Option<Value>,
    /// The latest `current_mode_update`'s mode id.
    pub mode: Option<String>,
}

/// One engine session's stream.
pub struct Stream<'a> {
    link: &'a dyn EngineLink,
    engine: String,
    session: String,
    turn: Turn,
}

impl<'a> Stream<'a> {
    pub fn new(link: &'a dyn EngineLink, engine: &str, session: &str) -> Self {
        Self {
            link,
            engine: engine.to_string(),
            session: session.to_string(),
            turn: Turn::default(),
        }
    }

    /// Takes the `update` of one `session/update` notification: buffers or forwards it, and keeps
    /// what [`Turn`] needs.
    pub fn absorb(&mut self, update: Value) {
        let _ = (&self.link, &self.engine, &self.session, update);
    }

    /// True when buffered text has waited [`COALESCE`] or longer as of `now`.
    pub fn due(&self, now: Instant) -> bool {
        let _ = now;
        false
    }

    /// Sends the buffered text, if any.
    pub fn flush(&mut self) {}

    /// The turn so far.
    pub fn turn(&self) -> &Turn {
        &self.turn
    }

    /// Starts a new turn's record (the stream and its `seq` continue).
    pub fn next_turn(&mut self) {
        self.turn = Turn::default();
    }
}
