//! A session's work as ACP `session/update`s.
//!
//! Live, [`Feed::frame`] reads the hub's bus frames for one work item:
//! - `delta {cid, work, req, delta}`: the agent's model stream → `agent_message_chunk` (text) or
//!   `agent_thought_chunk` (thinking);
//! - `transcript` entries `{cid, work, kind, data}`: `tool_request` → `tool_call` (kind from the
//!   tool's law operation, `apz_acp::map::tool_kind_for_operation`, title from the tool, raw input
//!   capped), `tool_decision` → `tool_call_update` (`in_progress`, or `failed` with the reason),
//!   `tool_result`/`tool_report` → `tool_call_update` (`completed` or `failed`, output as text
//!   content, capped), a `todo_write` call → `plan`, `model_response` → nothing (its text already
//!   streamed as deltas), `engine_update` entries → nothing (the live `engine` frames carry them);
//! - `engine {cid, work, call, engine, update}`: an engine's own stream under its `code_engine`
//!   call: its `tool_call`/`tool_call_update` re-keyed `<call>/<id>` (so they never collide with the
//!   plaza's ids), its `plan` as the session's plan, its text chunks as `agent_thought_chunk`.
//!
//! [`replay`] gives the same updates from a finished work item's transcript entries, for
//! `session/load` (text from `model_response` instead of deltas).

use serde_json::Value;

use crate::transcript::Entry;

/// The most of a tool's input or output an update carries, in bytes.
pub const CONTENT_BYTES: usize = 16 * 1024;

/// How a work item ended, from its `work_end` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
    /// `done`, `failed` or `stopped`.
    pub status: String,
    pub summary: String,
}

/// The live feed of one work item.
pub struct Feed {
    cid: u64,
    work: u64,
}

impl Feed {
    pub fn new(cid: u64, work: u64) -> Self {
        Self { cid, work }
    }

    /// The updates one bus frame (`kind`, its JSON `data`) gives this work item; none for another
    /// agent's or another work item's frames.
    pub fn frame(&mut self, kind: &str, data: &Value) -> Vec<Value> {
        let _ = (self.cid, self.work, kind, data);
        Vec::new()
    }

    /// The work item's end, when this frame is its `work_end` transcript entry.
    pub fn end(&self, kind: &str, data: &Value) -> Option<End> {
        let _ = (kind, data);
        None
    }
}

/// The updates that replay one work item from its transcript entries (other works' are skipped),
/// starting with its prompt as a `user_message_chunk`.
pub fn replay(work: u64, entries: &[Entry]) -> Vec<Value> {
    let _ = (work, entries);
    Vec::new()
}
