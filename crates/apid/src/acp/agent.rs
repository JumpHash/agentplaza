//! The ACP agent methods the hub answers, over the plaza as [`Hub`] sees it.
//!
//! | ACP | Plaza |
//! |---|---|
//! | `initialize` | protocol 1; `loadSession`; prompts take text and embedded resources; sessions can be listed, resumed and closed |
//! | `session/new` | a new session on the connection's agent (`agentplaza acp --agent`), else `_meta.agentplaza.agent`, else the orchestrator; the editor's `mcpServers` are not connected (a notice says so); modes are the agent's approval modes |
//! | `session/prompt` | a chat work item on the session's thread; its stream as updates until its end; `stopReason` from the end (`end_turn`, `cancelled`); a failed item is a JSON-RPC error; an approval the work waits on is asked as `session/request_permission` and answered for the owner |
//! | `session/cancel` | cancels the session's work item in progress |
//! | `session/load` | replays the session's work items, then continues |
//! | `session/list`, `session/resume`, `session/close` | the sessions; resume without replay; close ends it |
//! | `session/set_mode` | the agent's approval mode: `autonomous`, `ask`, `read_only` (`current_mode_update` follows) |

use std::sync::mpsc::Receiver;

use apz_acp::rpc::Peer;
use apz_acp::types;
use serde_json::Value;

use crate::events::Frame;
use crate::transcript::Entry;

/// What the ACP methods need from the plaza. [`super::app_hub`] gives the hub's; the tests give a
/// fake.
pub trait Hub: Sync {
    /// The agent `name` names (an agent name or a CID), or the orchestrator for `None`: its CID,
    /// or why there is none (no such agent, offline, a model agent with no chat).
    fn agent(&self, name: Option<&str>) -> Result<u64, String>;
    /// Subscribes to the bus. Taken before the work is queued, so no frame is missed.
    fn subscribe(&self) -> Receiver<Frame>;
    /// Queues `text` as the owner's chat on agent `cid`, thread `thread` (origin: the owner, over
    /// ACP); the work item's id.
    fn chat(&self, cid: u64, thread: &str, text: &str) -> Result<u64, String>;
    /// Cancels work item `work` of agent `cid` if it is in progress or queued; false otherwise.
    fn cancel(&self, cid: u64, work: u64) -> bool;
    /// The transcript entries of agent `cid` for the given work items, oldest first.
    fn entries(&self, cid: u64, works: &[u64]) -> Vec<Entry>;
    /// The pending approvals of agent `cid` held on `thread` (each as the approvals API shows it).
    fn pending_approvals(&self, cid: u64, thread: &str) -> Vec<Value>;
    /// Decides approval `id` for the owner ("from the editor"), as `POST /v1/approvals/{id}` does.
    fn decide(&self, id: &str, approve: bool, reason: &str) -> Result<(), String>;
    /// The agent's approval mode: `autonomous`, `ask` or `read_only`.
    fn mode(&self, cid: u64) -> String;
    fn set_mode(&self, cid: u64, mode: &str) -> Result<(), String>;
    fn now_ms(&self) -> u64;
}

/// One editor connection: who it is and the agent it asked for.
pub struct Conn<'h> {
    pub hub: &'h dyn Hub,
    pub sessions: &'h super::store::Sessions,
    /// `agentplaza acp --agent <name|cid>`, from the socket's `?agent=` query.
    pub agent: Option<String>,
    /// The editor's `clientInfo`, once `initialize` named it. The connection owns it; each call
    /// borrows it (the socket's handlers build a `Conn` per message over the connection's state).
    pub client: &'h std::sync::Mutex<String>,
}

/// Answers one request from the editor.
pub fn request(
    conn: &Conn<'_>,
    peer: &Peer,
    method: &str,
    params: Value,
) -> Result<Value, types::Error> {
    let _ = (conn, peer, method, params);
    Err(types::Error::method_not_found())
}

/// Takes one notification from the editor (`session/cancel`).
pub fn notification(conn: &Conn<'_>, method: &str, params: Value) {
    let _ = (conn, method, params);
}
