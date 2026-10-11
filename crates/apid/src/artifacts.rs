//! Files agents share with each other and the operator: a guest's `share_file`
//! publishes its file here, `plaza_get_file` reads the latest one back, and the
//! routes list them for the console.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::{Response, Router};
use serde::Serialize;

use crate::events::Bus;
use crate::state::App;

/// Newest artifacts kept; older ones are dropped.
const CAP: usize = 500;
/// Largest content kept per artifact.
const MAX_CONTENT: usize = 256 * 1024;

/// One shared file, content included.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub id: u64,
    pub cid: u64,
    pub work: u64,
    pub path: String,
    pub size: usize,
    pub at_ms: u64,
    pub content: String,
}

/// An artifact without its content: what is listed and published on the bus.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: u64,
    pub cid: u64,
    pub work: u64,
    pub path: String,
    pub size: usize,
    pub at_ms: u64,
}

impl Artifact {
    fn summary(&self) -> Summary {
        Summary {
            id: self.id,
            cid: self.cid,
            work: self.work,
            path: self.path.clone(),
            size: self.size,
            at_ms: self.at_ms,
        }
    }
}

/// The shared files, oldest first.
#[derive(Default)]
pub struct Artifacts {
    items: Mutex<Vec<Artifact>>,
    next: AtomicU64,
}

impl Artifacts {
    /// Stores a shared file (content capped) and announces its summary on the bus.
    pub fn add(&self, bus: &Bus, cid: u64, work: u64, path: &str, mut content: String) -> Artifact {
        let mut end = content.len().min(MAX_CONTENT);
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        content.truncate(end);
        let artifact = Artifact {
            id: self.next.fetch_add(1, Ordering::Relaxed) + 1,
            cid,
            work,
            path: path.to_string(),
            size: content.len(),
            at_ms: now_ms(),
            content,
        };
        bus.publish("artifact", &artifact.summary());
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.push(artifact.clone());
        if items.len() > CAP {
            items.remove(0);
        }
        artifact
    }

    /// Summaries of the shared files, newest first.
    pub fn list(&self) -> Vec<Summary> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.iter().rev().map(Artifact::summary).collect()
    }

    pub fn get(&self, id: u64) -> Option<Artifact> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.iter().find(|a| a.id == id).cloned()
    }

    /// The newest file `cid` shared at `path`.
    pub fn latest(&self, cid: u64, path: &str) -> Option<Artifact> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.iter().rev().find(|a| a.cid == cid && a.path == path).cloned()
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `GET /v1/artifacts` (summaries) and `GET /v1/artifacts/{id}` (with content).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/artifacts", |app, _, _| Response::json(200, &app.artifacts.list()))
        .get("/v1/artifacts/{id}", |app, _, p| {
            match p.get("id").parse::<u64>().ok().and_then(|id| app.artifacts.get(id)) {
                Some(artifact) => Response::json(200, &artifact),
                None => Response::error(404, "not found"),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_and_latest_track_the_newest_file_per_agent_and_path() {
        let bus = Bus::default();
        let shared = Artifacts::default();
        let first = shared.add(&bus, 7, 1, "notes.md", "one".into());
        let other = shared.add(&bus, 8, 1, "notes.md", "other agent".into());
        let second = shared.add(&bus, 7, 2, "notes.md", "two".into());

        let listed = shared.list();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].id, second.id, "newest first");
        assert_eq!(listed[2].id, first.id);
        assert_eq!(listed[1].cid, other.cid);
        assert_eq!(shared.latest(7, "notes.md").map(|a| a.content), Some("two".to_string()));
        assert!(shared.latest(7, "missing.md").is_none());
        assert_eq!(shared.get(first.id).map(|a| a.content), Some("one".to_string()));
        assert!(shared.get(999).is_none());
    }

    #[test]
    fn list_keeps_the_newest_cap_and_content_is_capped() {
        let bus = Bus::default();
        let shared = Artifacts::default();
        // Two-byte characters: the cut must land on a boundary.
        let big = shared.add(&bus, 1, 1, "big.txt", "é".repeat(200 * 1024));
        assert!(big.content.len() <= MAX_CONTENT);
        assert_eq!(big.size, big.content.len());

        for n in 0..CAP + 5 {
            shared.add(&bus, 1, 1, "f.txt", n.to_string());
        }
        assert_eq!(shared.list().len(), CAP);
        assert!(shared.get(big.id).is_none(), "the oldest artifact is dropped");
    }
}
