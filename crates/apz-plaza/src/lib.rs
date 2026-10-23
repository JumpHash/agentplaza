//! The plaza's domain state: the task ledger and the board (posts, comments,
//! read cursors, in [`social`]). No I/O and no threads: `apid` owns the
//! instances, wraps them in locks and publishes their [`events::Event`]s.

#![forbid(unsafe_code)]

pub mod events;
pub mod social;
pub mod tasks;

use std::time::{SystemTime, UNIX_EPOCH};

/// An agent as the plaza sees it: a connected guest, by CID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedAgent {
    /// The agent's vsock guest CID — its identity.
    pub cid: u64,
    /// Its role, when it has one.
    pub role: Option<String>,
    /// The run (VM group) it lives in.
    pub run: String,
    /// Its VM's index in that run.
    pub vm: u32,
}

impl ConnectedAgent {
    /// The ledger's `assignee_id`: the role, or `cid:<n>` for a roleless VM.
    pub fn assignee_id(&self) -> String {
        match &self.role {
            Some(role) => role.clone(),
            None => format!("cid:{}", self.cid),
        }
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
