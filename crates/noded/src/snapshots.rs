//! VM snapshots on the node: where they live, and what the API says about them.
//!
//! Every snapshot of an agent VM sits under the node's snapshot root, per guest CID:
//!
//! ```text
//! <root>/cid-<cid>/sleep/                 the VM's sleep snapshot (one; replaced on each sleep)
//! <root>/cid-<cid>/cp-<ms>-<label>/       a checkpoint: vmstate.json, memory.img, workspace.img
//! <root>/templates/<key>/                 a warm template (see `crate::templates`)
//! ```
//!
//! A snapshot directory holds `vmstate.json` and `memory.img` (`vmm::vstate::snapshot`). A
//! checkpoint also holds `workspace.img`, a reflinked copy of the VM's workspace volume taken while
//! the VM was paused; a sleep snapshot does not need one, because the stopped VM's own workspace
//! image is not written until it wakes. Directories are 0700 and files 0600: guest memory is the
//! agent's data. The root is `--snapshot-dir`, or `<persist-dir>/snapshots`; a node with neither
//! refuses every snapshot operation (409).
//!
//! **A restored VM maps its snapshot's memory file privately**, so a snapshot directory is never
//! modified in place: a new sleep snapshot is written beside the old one (`sleep.new`) and renamed
//! over it, and a removed directory's files stay readable by a VM that still maps them.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// The sleep snapshot's directory name.
pub const SLEEP: &str = "sleep";
/// The prefix of a checkpoint's directory name.
pub const CHECKPOINT_PREFIX: &str = "cp-";
/// A checkpoint's copy of the workspace volume.
pub const WORKSPACE_FILE: &str = "workspace.img";
/// The checkpoints kept per VM; the oldest beyond this are removed when a new one is taken.
pub const KEEP_CHECKPOINTS: usize = 20;

/// One snapshot, as `GET .../checkpoints` and the sleep/checkpoint replies report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotView {
    /// The directory name: `sleep` or `cp-<ms>-<label>`.
    pub name: String,
    pub cid: u64,
    /// The label given when it was taken (`""` for the sleep snapshot).
    pub label: String,
    /// `full` or `diff` (`vmm::spawner::SnapshotKind`).
    pub kind: String,
    pub taken_at_ms: u64,
    /// Guest memory bytes the snapshot wrote.
    pub memory_written: u64,
    /// Bytes the directory takes on disk (allocated blocks, so reflinked copies count as small).
    pub disk_bytes: u64,
    /// How long the VM was paused for it, in milliseconds.
    pub paused_ms: u64,
}

/// The node's snapshot root.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    root: PathBuf,
}

impl SnapshotStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory of VM `cid`'s snapshot `name`.
    pub fn dir(&self, cid: u64, name: &str) -> PathBuf {
        self.root.join(format!("cid-{cid}")).join(name)
    }

    /// A new checkpoint's name for `label` (lowercased, `[a-z0-9-]` only, at most 40 characters),
    /// taken at `now_ms`.
    pub fn checkpoint_name(label: &str, now_ms: u64) -> String {
        let _ = (label, now_ms);
        todo!("checkpoint name")
    }

    /// The checkpoints of VM `cid`, newest first (the sleep snapshot is not one).
    pub fn checkpoints(&self, cid: u64) -> Vec<SnapshotView> {
        let _ = cid;
        todo!("list checkpoints")
    }

    /// The view of snapshot `name` of VM `cid`, `None` when it does not exist or is unreadable.
    pub fn view(&self, cid: u64, name: &str) -> Option<SnapshotView> {
        let _ = (cid, name);
        todo!("snapshot view")
    }

    /// Removes snapshot `name` of VM `cid` (a missing one is not an error).
    pub fn remove(&self, cid: u64, name: &str) -> std::io::Result<()> {
        let _ = (cid, name);
        todo!("remove snapshot")
    }

    /// Removes every snapshot of VM `cid` (the VM's run was deleted).
    pub fn remove_all(&self, cid: u64) -> std::io::Result<()> {
        let _ = cid;
        todo!("remove all snapshots")
    }

    /// Removes the oldest checkpoints of VM `cid` beyond [`KEEP_CHECKPOINTS`].
    pub fn prune(&self, cid: u64) {
        let _ = cid;
        todo!("prune checkpoints")
    }
}
