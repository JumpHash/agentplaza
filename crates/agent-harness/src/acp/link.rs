//! What an engine session needs from the agent's session while it runs.
//!
//! A `code_engine` call runs on the tool thread while the agent loop waits for it. The engine
//! (an ACP agent on the child's stdio) asks the harness before it acts, and the harness asks the
//! hub: every permission request and every client method (`fs/*`, `terminal/*`) becomes a nested
//! `tool_request` named `engine.<kind>` that the hub's laws and approvals decide. The
//! engine's stream goes to the hub as `engine_update` lines. [`EngineLink`] is that path; the
//! agent loop implements it over the session's shared writer and nested-call registry, and the
//! tests implement it in memory.

use std::time::Instant;

use apz_proto::chat::ToolSpec;
use serde_json::Value;

/// The hub's final answer on one engine action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The engine may go ahead, this once.
    Allow,
    /// The engine may not; the reason is the hub's (a law, the owner's denial, no
    /// decision before the deadline).
    Deny(String),
    /// The work item was cancelled or the agent is stopping while the decision was open.
    Cancelled,
}

/// The agent's session as an engine session sees it.
pub trait EngineLink: Send + Sync {
    /// Asks the hub whether the engine may do `name` (`engine.read`, `engine.edit`, `engine.exec`,
    /// `engine.fetch` or `engine.other`) with `input` (`{engine, title, locations, raw}`). Blocks
    /// until a final verdict: a `pending` answer waits for the owner's `permission_decision`, until
    /// `deadline` (then [`Verdict::Deny`]) or a cancel (then [`Verdict::Cancelled`]).
    fn decide(&self, name: &str, input: &Value, deadline: Instant) -> Verdict;

    /// Sends one ACP `SessionUpdate` (its JSON, already capped) of engine session `session` to the
    /// hub as an `engine_update`. Best effort: a failed write is the session's problem, not the
    /// engine's.
    fn update(&self, engine: &str, session: &str, update: &Value);

    /// True once the work item was cancelled, or the agent is stopping: the engine session then
    /// sends `session/cancel`, answers its open requests `cancelled` and ends the turn.
    fn cancelled(&self) -> bool;

    /// The engine's turn is over (its prompt was answered, the engine exited, or it was killed after
    /// the cancel grace). Waits still open on the hub end at once, so the turn's threads can join
    /// instead of waiting for their deadline. Default: nothing to end.
    fn end_turn(&self) {}

    /// The plaza's tools an engine may call through its MCP server (see [`super::mcp::plaza_tools`]).
    /// Empty where the link has none, so the engine is offered no MCP server.
    fn plaza_tools(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    /// The prompt parts the engine is told, as text, in order: rendered into its instruction file
    /// before it starts (see [`super::adapter`]). Empty where the link has none.
    fn instructions(&self) -> Vec<String> {
        Vec::new()
    }

    /// Calls the plaza's tool `name` with `input` through the hub's gate, as the agent's own calls
    /// are gated. A tool the hub lets run in the VM runs here. `Ok` is the tool's output, `Err` its
    /// error or the reason it did not run. Blocks until the hub's final answer, the `deadline` or a
    /// cancel, as [`EngineLink::decide`] does.
    fn call(&self, name: &str, input: &Value, deadline: Instant) -> Result<String, String> {
        let _ = (name, input, deadline);
        Err("plaza tools are not available here".into())
    }
}
