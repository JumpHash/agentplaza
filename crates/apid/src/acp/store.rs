//! The editors' ACP sessions: which agent each one drives, its work items, and its title. Kept in
//! memory and saved with the plaza state (`db::snapshot_of` component `acp_sessions`), so an editor
//! can list, load and resume its sessions after the hub restarts.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The most sessions kept; the oldest closed ones go first, then the oldest.
pub const KEEP: usize = 500;

/// One editor session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// `acp-<16 hex>`; also the agent's thread for this session.
    pub id: String,
    pub cid: u64,
    /// The editor's project folder, as the editor named it (shown and listed, never opened).
    pub cwd: String,
    /// The editor that opened it (`clientInfo.name` and version), for the console.
    pub client: String,
    /// The first prompt's first line, at most 80 characters.
    pub title: String,
    /// The work items this session queued, oldest first.
    pub works: Vec<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub closed: bool,
}

/// All sessions.
#[derive(Default)]
pub struct Sessions {
    inner: Mutex<BTreeMap<String, Session>>,
}

impl Sessions {
    /// Opens a new session on agent `cid`.
    pub fn open(&self, cid: u64, cwd: &str, client: &str, now_ms: u64) -> Session {
        let _ = (cid, cwd, client, now_ms, &self.inner);
        unimplemented_session()
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        let _ = id;
        None
    }

    /// The sessions, newest first; only those with `cwd` when it is given.
    pub fn list(&self, cwd: Option<&str>) -> Vec<Session> {
        let _ = cwd;
        Vec::new()
    }

    /// Records work item `work` on session `id`; the first prompt's first line becomes the title.
    pub fn add_work(&self, id: &str, work: u64, prompt: &str, now_ms: u64) {
        let _ = (id, work, prompt, now_ms);
    }

    /// Marks session `id` closed; false when there is no such session.
    pub fn close(&self, id: &str, now_ms: u64) -> bool {
        let _ = (id, now_ms);
        false
    }

    /// The whole state as JSON, for Postgres; "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        String::new()
    }

    /// Replaces the state with a [`Sessions::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let _ = json;
        Ok(())
    }
}

fn unimplemented_session() -> Session {
    Session {
        id: String::new(),
        cid: 0,
        cwd: String::new(),
        client: String::new(),
        title: String::new(),
        works: Vec::new(),
        created_ms: 0,
        updated_ms: 0,
        closed: false,
    }
}
