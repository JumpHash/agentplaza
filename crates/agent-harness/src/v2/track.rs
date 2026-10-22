//! Read tracking: what the agent has been shown of each file in the current work item.
//!
//! A read records the file's digest, its size and modification time (its stamp), the lines it showed and the
//! call that showed them. A later read of the same lines of an unchanged file answers "unchanged since call N"
//! without the content. A file that changes underneath the agent (not by `edit`, which records its own writes)
//! is noted in the next result of any tool, and forgotten, so the next read of it shows the content again.

use std::collections::BTreeMap;
use std::fs;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::files::Workspace;

use super::{Lane, locked};

/// The most files one work item tracks; the one read longest ago is forgotten past it.
const MAX_TRACKED: usize = 512;

/// A file's digest, as SHA-256 of its bytes.
pub(crate) type Digest32 = [u8; 32];

/// What a file looked like when the agent was last shown it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    /// The digest of the whole file as it was shown. None once the file changed and the change was noted.
    pub(crate) digest: Option<Digest32>,
    /// The file's size and modification time when the digest was taken.
    pub(crate) stamp: Stamp,
    /// The lines shown, 1-based and inclusive.
    pub(crate) window: (usize, usize),
    /// The call that showed them.
    pub(crate) call: u64,
}

/// A file's size and modification time: when both are unchanged the file is taken as unchanged, without a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    /// The stamp of a file that cannot be read: never equal to a real one, so the file counts as changed.
    pub(crate) const UNKNOWN: Stamp = Stamp {
        len: u64::MAX,
        modified: None,
    };
}

/// The stamp of `path`, or None when it cannot be read.
pub(crate) fn stamp_of(path: &std::path::Path) -> Option<Stamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(Stamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// The digest of `bytes`.
pub(crate) fn digest_of(bytes: &[u8]) -> Digest32 {
    Sha256::digest(bytes).into()
}

/// The reads of one work item, by workspace-relative path.
#[derive(Debug, Default)]
pub(crate) struct Tracking {
    /// The work item the reads belong to.
    work: Option<u64>,
    /// How many tool calls the work item has made: the number a read is "since".
    calls: u64,
    /// What each file looked like when it was last shown.
    files: BTreeMap<String, Seen>,
}

impl Tracking {
    /// The work item the reads belong to, or None before the first.
    pub(crate) fn work(&self) -> Option<u64> {
        self.work
    }

    /// Starts the tracking over for `work`.
    pub(crate) fn start(&mut self, work: u64) {
        *self = Self {
            work: Some(work),
            calls: 0,
            files: BTreeMap::new(),
        };
    }

    /// Counts a tool call and returns its number: the first call of a work item is 1.
    pub(crate) fn next_call(&mut self) -> u64 {
        self.calls += 1;
        self.calls
    }

    /// What the agent was last shown of `label`.
    pub(crate) fn seen(&self, label: &str) -> Option<&Seen> {
        self.files.get(label)
    }

    /// Records what the agent was just shown of `label`, or what an edit just wrote.
    pub(crate) fn record(&mut self, label: &str, seen: Seen) {
        self.files.insert(label.to_string(), seen);
        if self.files.len() > MAX_TRACKED {
            let oldest = self
                .files
                .iter()
                .min_by_key(|(_, seen)| seen.call)
                .map(|(label, _)| label.clone());
            if let Some(oldest) = oldest {
                self.files.remove(&oldest);
            }
        }
    }

    /// The tracked files whose digest is still held, with what they were last shown as.
    fn held(&self) -> Vec<(String, Seen)> {
        self.files
            .iter()
            .filter(|(_, seen)| seen.digest.is_some())
            .map(|(label, seen)| (label.clone(), seen.clone()))
            .collect()
    }

    /// Forgets what the agent was shown of `label`: the next read of it shows the content.
    fn forget(&mut self, label: &str) {
        if let Some(seen) = self.files.get_mut(label) {
            seen.digest = None;
        }
    }

    /// Replaces the stamp of `label` after a check found the file unchanged in content.
    fn restamp(&mut self, label: &str, stamp: Stamp) {
        if let Some(seen) = self.files.get_mut(label) {
            seen.stamp = stamp;
        }
    }
}

/// Checks the lane's tracked reads: a file that changed since it was shown gets a note, and is forgotten. A file
/// whose stamp is the same is taken as unchanged without being read.
pub(crate) fn changes(workspace: &Workspace, lane: &Lane) {
    let held = locked(&lane.tracking).held();
    for (label, seen) in held {
        let Ok((path, _)) = workspace.resolve(&label) else {
            changed(lane, &label);
            continue;
        };
        let Some(stamp) = stamp_of(&path) else {
            changed(lane, &label);
            continue;
        };
        if stamp == seen.stamp {
            continue;
        }
        let same = fs::read(&path)
            .ok()
            .is_some_and(|bytes| Some(digest_of(&bytes)) == seen.digest);
        if same {
            locked(&lane.tracking).restamp(&label, stamp);
        } else {
            changed(lane, &label);
        }
    }
}

/// Notes that `label` changed since the agent read it, and forgets what it was shown.
fn changed(lane: &Lane, label: &str) {
    locked(&lane.tracking).forget(label);
    lane.note(format!("{label} changed since you read it"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_work_item_starts_with_no_reads_and_counts_its_calls_from_one() {
        let mut tracking = Tracking::default();
        tracking.start(3);
        assert_eq!(tracking.next_call(), 1);
        assert_eq!(tracking.next_call(), 2);
        tracking.record(
            "a.rs",
            Seen {
                digest: Some([1; 32]),
                stamp: Stamp {
                    len: 1,
                    modified: None,
                },
                window: (1, 1),
                call: 2,
            },
        );
        tracking.start(4);
        assert_eq!(tracking.work(), Some(4));
        assert!(tracking.seen("a.rs").is_none());
        assert_eq!(tracking.next_call(), 1);
    }

    #[test]
    fn the_least_recent_read_is_forgotten_past_the_cap() {
        let mut tracking = Tracking::default();
        tracking.start(1);
        for call in 0..=MAX_TRACKED as u64 {
            tracking.record(
                &format!("f{call}.rs"),
                Seen {
                    digest: None,
                    stamp: Stamp {
                        len: 0,
                        modified: None,
                    },
                    window: (1, 1),
                    call,
                },
            );
        }
        assert!(tracking.seen("f0.rs").is_none());
        assert!(tracking.seen(&format!("f{}.rs", MAX_TRACKED)).is_some());
    }
}
