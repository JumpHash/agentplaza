//! Work items: per-agent FIFO queues (one item in flight per agent), sending
//! them through the hub, closing them, and the scheduler that turns open
//! tasks, open motions and agent messages into work.
//!
//! OWNER: slice "scheduler". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::sync::Arc;

use apz_proto::wire::{WorkKind, WorkStatus};
use serde::Serialize;

use crate::state::App;

/// What a work item is about, beyond its prompt.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Origin {
    pub task: Option<String>,
    pub motion: Option<String>,
    /// For `message` work: the agent who wrote.
    pub from_cid: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkItem {
    pub id: u64,
    pub cid: u64,
    pub kind: WorkKind,
    pub thread: String,
    pub prompt: String,
    pub origin: Origin,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
}

#[derive(Default)]
pub struct Queues {
    // TODO(scheduler slice)
}

impl Queues {
    /// Queues work for `cid` and sends it if the agent is idle; the work id.
    pub fn enqueue(&self, app: &App, cid: u64, kind: WorkKind, thread: String, prompt: String, origin: Origin) -> u64 {
        // contract — TODO(scheduler slice)
        let _ = (app, cid, kind, thread, prompt, origin);
        0
    }

    /// The item in flight for `cid`.
    pub fn current(&self, cid: u64) -> Option<WorkItem> {
        // contract
        let _ = cid;
        None
    }

    /// Items waiting behind the current one.
    pub fn queued(&self, cid: u64) -> Vec<WorkItem> {
        // contract
        let _ = cid;
        Vec::new()
    }

    /// The agent reported `work_done`: close the item (task/chat/message
    /// effects), then send the next one.
    pub fn complete(&self, app: &App, cid: u64, work: u64, status: WorkStatus, summary: &str) {
        // contract — TODO(scheduler slice)
        let _ = (app, cid, work, status, summary);
    }

    /// The operator cancels the current item (sends `cancel`).
    pub fn cancel(&self, app: &App, cid: u64, reason: &str) -> bool {
        // contract — TODO(scheduler slice)
        let _ = (app, cid, reason);
        false
    }

    /// The agent went offline: fail its items (tasks return to open).
    pub fn agent_gone(&self, app: &App, cid: u64) {
        // contract — TODO(scheduler slice)
        let _ = (app, cid);
    }
}

/// The scheduler thread (every 2 s).
pub fn spawn(app: Arc<App>) {
    // contract — TODO(scheduler slice)
    let _ = app;
}
