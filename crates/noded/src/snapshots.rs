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

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use vmm::spawner::SnapshotKind;
use vmm::vstate::snapshot::SnapshotMeta;

/// The sleep snapshot's directory name.
pub const SLEEP: &str = "sleep";
/// The prefix of a checkpoint's directory name.
pub const CHECKPOINT_PREFIX: &str = "cp-";
/// A checkpoint's copy of the workspace volume.
pub const WORKSPACE_FILE: &str = "workspace.img";
/// A snapshot's guest memory file (`vmm::vstate::snapshot`).
pub const MEMORY_FILE: &str = "memory.img";
/// The marker of a disk-only checkpoint (no memory, no VM state): `{"taken_at_ms": n}`. A VM
/// restored or forked from one boots cold from the checkpoint's disk.
pub const DISK_MARKER: &str = "disk.json";
/// Where a new sleep snapshot is written before it takes the name [`SLEEP`].
pub const SLEEP_PARTIAL: &str = "sleep.new";
/// The diff base of a VM restored from a snapshot: a copy of that snapshot's memory (see
/// [`SnapshotStore::stage_base`]).
pub const BASE: &str = "base";
/// The checkpoints kept per VM; the oldest beyond this are removed when a new one is taken.
pub const KEEP_CHECKPOINTS: usize = 20;
/// How old a checkpoint directory with neither a VM state nor a disk marker must be before a prune
/// removes it as a half-made checkpoint.
const PARTIAL_AGE: Duration = Duration::from_secs(10 * 60);
/// The most characters of a label that make it into a checkpoint's name.
const MAX_LABEL_LEN: usize = 40;

/// One snapshot, as `GET .../checkpoints` and the sleep/checkpoint replies report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotView {
    /// The directory name: `sleep` or `cp-<ms>-<label>`.
    pub name: String,
    pub cid: u64,
    /// The label given when it was taken (`""` for the sleep snapshot). The directory name keeps
    /// the label's slug only, so a snapshot read back from disk carries the slug.
    pub label: String,
    /// `full` or `diff` (`vmm::spawner::SnapshotKind`).
    pub kind: String,
    pub taken_at_ms: u64,
    /// Guest memory bytes the snapshot wrote.
    pub memory_written: u64,
    /// Bytes the directory takes on disk (allocated blocks, so reflinked copies count as small).
    pub disk_bytes: u64,
    /// How long the VM was paused for it, in milliseconds. The snapshot's meta does not keep it, so
    /// a view read back from disk reports `0`; the reply that took the snapshot carries the real figure.
    pub paused_ms: u64,
}

/// The node's snapshot root.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    root: PathBuf,
}

/// The part of `vmstate.json` a view needs. The rest of the file is the VMM's to read.
#[derive(Deserialize)]
struct Head {
    meta: SnapshotMeta,
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

    /// Creates the root and VM `cid`'s directory, both 0700, so a snapshot can be written.
    pub fn prepare(&self, cid: u64) -> io::Result<()> {
        private_dir(&self.root)?;
        private_dir(&self.root.join(format!("cid-{cid}")))
    }

    /// A new checkpoint's name for `label` (lowercased, `[a-z0-9-]` only, at most 40 characters),
    /// taken at `now_ms`: `cp-<ms>-<slug>`, or `cp-<ms>` when the label has no usable characters.
    pub fn checkpoint_name(label: &str, now_ms: u64) -> String {
        let mut slug = String::new();
        for c in label.chars().flat_map(char::to_lowercase) {
            if c.is_ascii_alphanumeric() {
                slug.push(c);
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
        }
        slug.truncate(MAX_LABEL_LEN);
        while slug.ends_with('-') {
            slug.pop();
        }
        if slug.is_empty() {
            format!("{CHECKPOINT_PREFIX}{now_ms}")
        } else {
            format!("{CHECKPOINT_PREFIX}{now_ms}-{slug}")
        }
    }

    /// The checkpoints of VM `cid`, newest first (the sleep snapshot is not one). A VM with no
    /// snapshot directory has none.
    pub fn checkpoints(&self, cid: u64) -> Vec<SnapshotView> {
        let Ok(entries) = fs::read_dir(self.root.join(format!("cid-{cid}"))) else {
            return Vec::new();
        };
        let mut views: Vec<SnapshotView> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| is_checkpoint_name(name))
            .filter_map(|name| self.view(cid, &name))
            .collect();
        views.sort_by(|a, b| {
            b.taken_at_ms
                .cmp(&a.taken_at_ms)
                .then_with(|| b.name.cmp(&a.name))
        });
        views
    }

    /// The view of snapshot `name` of VM `cid`, `None` when the name is not a snapshot name, the
    /// snapshot does not exist, or its meta is unreadable.
    pub fn view(&self, cid: u64, name: &str) -> Option<SnapshotView> {
        if !is_snapshot_name(name) {
            return None;
        }
        let dir = self.dir(cid, name);
        let Ok(meta) = read_meta(&dir) else {
            let taken_at_ms = disk_marker(&dir)?;
            return Some(SnapshotView {
                name: name.to_string(),
                cid,
                label: label_of(name),
                kind: "disk".to_string(),
                taken_at_ms,
                memory_written: 0,
                disk_bytes: disk_bytes(&dir),
                paused_ms: 0,
            });
        };
        Some(SnapshotView {
            name: name.to_string(),
            cid,
            label: label_of(name),
            kind: kind_name(meta.kind).to_string(),
            taken_at_ms: meta.taken_at_ms,
            memory_written: meta.memory_written,
            disk_bytes: disk_bytes(&dir),
            paused_ms: 0,
        })
    }

    /// Removes snapshot `name` of VM `cid` (a missing one is not an error). A name that is neither
    /// `sleep` nor a checkpoint's is refused, so nothing outside the snapshot tree is touched.
    pub fn remove(&self, cid: u64, name: &str) -> io::Result<()> {
        if !is_snapshot_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name:?} is not a snapshot name"),
            ));
        }
        remove_dir(&self.dir(cid, name))
    }

    /// Moves the finished snapshot at `partial` to take `name` for VM `cid`, replacing what `name`
    /// was. `partial` is removed on failure.
    pub fn install(&self, cid: u64, partial: &Path, name: &str) -> io::Result<()> {
        let result = remove_dir(&self.dir(cid, name))
            .and_then(|()| fs::rename(partial, self.dir(cid, name)));
        if result.is_err() {
            let _ = fs::remove_dir_all(partial);
        }
        result
    }

    /// Stages a copy of `memory` (a snapshot's memory file) as VM `cid`'s diff base, and returns the
    /// directory holding it. A VM restored from a snapshot writes its next snapshot as a diff over
    /// this copy, so the copy is what keeps that working when the snapshot it came from is removed
    /// (a checkpoint the user deletes, or a run that ends). The copy is replaced on each restore.
    pub fn stage_base(&self, cid: u64, memory: &Path) -> io::Result<PathBuf> {
        self.prepare(cid)?;
        let dir = self.dir(cid, BASE);
        remove_dir(&dir)?;
        private_dir(&dir)?;
        vmm::clonefile::clone_file(memory, &dir.join(MEMORY_FILE))?;
        Ok(dir)
    }

    /// Removes every snapshot of VM `cid` (the VM's run was deleted).
    pub fn remove_all(&self, cid: u64) -> io::Result<()> {
        remove_dir(&self.root.join(format!("cid-{cid}")))
    }

    /// Removes the oldest checkpoints of VM `cid` beyond [`KEEP_CHECKPOINTS`], and the half-made ones
    /// (see [`SnapshotStore::sweep_partial`]). A checkpoint that cannot be removed is reported and
    /// left for the next prune.
    pub fn prune(&self, cid: u64) {
        self.sweep_partial(cid);
        for old in self.checkpoints(cid).into_iter().skip(KEEP_CHECKPOINTS) {
            if let Err(err) = self.remove(cid, &old.name) {
                eprintln!(
                    "noded: VM CID {cid}: cannot prune checkpoint {}: {err}",
                    old.name
                );
            }
        }
    }

    /// Removes the checkpoint directories of VM `cid` that hold neither a VM state nor a disk marker
    /// and were last modified more than [`PARTIAL_AGE`] ago: a crash mid-checkpoint left them. A
    /// younger one may still be in progress, so it is left alone.
    fn sweep_partial(&self, cid: u64) {
        let Ok(entries) = fs::read_dir(self.root.join(format!("cid-{cid}"))) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let dir = entry.path();
            if !is_checkpoint_name(&name)
                || dir.join("vmstate.json").exists()
                || dir.join(DISK_MARKER).exists()
            {
                continue;
            }
            let aged = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|taken| taken.elapsed().ok())
                .is_some_and(|age| age > PARTIAL_AGE);
            if aged {
                if let Err(err) = remove_dir(&dir) {
                    eprintln!("noded: VM CID {cid}: cannot sweep checkpoint {name}: {err}");
                }
            }
        }
    }
}

/// Whether `name` is a checkpoint's directory name: `cp-` and then `[a-z0-9-]`, at least one.
pub fn is_checkpoint_name(name: &str) -> bool {
    name.strip_prefix(CHECKPOINT_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// Whether `name` is a snapshot directory's name: the sleep snapshot's or a checkpoint's.
pub fn is_snapshot_name(name: &str) -> bool {
    name == SLEEP || is_checkpoint_name(name)
}

/// The label a snapshot's name carries: the slug of `cp-<ms>-<slug>`, `""` for `sleep` or a
/// checkpoint without one.
fn label_of(name: &str) -> String {
    name.strip_prefix(CHECKPOINT_PREFIX)
        .and_then(|rest| rest.split_once('-'))
        .map(|(_, slug)| slug.to_string())
        .unwrap_or_default()
}

/// The API's name for a snapshot kind.
fn kind_name(kind: SnapshotKind) -> &'static str {
    match kind {
        SnapshotKind::Full => "full",
        SnapshotKind::Diff | SnapshotKind::DiffUnsynced => "diff",
    }
}

/// When the disk-only checkpoint in `dir` was taken, if `dir` is one.
fn disk_marker(dir: &Path) -> Option<u64> {
    let text = fs::read(dir.join(DISK_MARKER)).ok()?;
    serde_json::from_slice::<serde_json::Value>(&text)
        .ok()?
        .get("taken_at_ms")?
        .as_u64()
}

/// Whether the checkpoint in `dir` holds a disk only (see [`DISK_MARKER`]).
pub fn is_disk_only(dir: &Path) -> bool {
    !dir.join("vmstate.json").exists() && dir.join(DISK_MARKER).exists()
}

/// Marks `dir` as a disk-only checkpoint taken at `taken_at_ms`. The marker is written to a temporary
/// file, synced, and renamed into place, so a crash leaves either no marker or a whole one.
pub fn write_disk_marker(dir: &Path, taken_at_ms: u64) -> io::Result<()> {
    let tmp = dir.join(format!("{DISK_MARKER}.tmp"));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(
        serde_json::json!({ "taken_at_ms": taken_at_ms })
            .to_string()
            .as_bytes(),
    )?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, dir.join(DISK_MARKER))?;
    // The rename is durable once the directory is synced; a failure here is not an error the
    // checkpoint can act on, since the marker is already in place.
    let _ = fs::File::open(dir).and_then(|dir| dir.sync_all());
    Ok(())
}

/// The meta of the snapshot in `dir`, read from its `vmstate.json`.
fn read_meta(dir: &Path) -> io::Result<SnapshotMeta> {
    let text = fs::read(dir.join("vmstate.json"))?;
    serde_json::from_slice::<Head>(&text)
        .map(|head| head.meta)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// The bytes the files of `dir` take on disk: their allocated blocks, so a reflinked copy that
/// shares its extents with another file counts only what it has written itself.
fn disk_bytes(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(fs::Metadata::is_file)
        .map(|meta| allocated_bytes(&meta))
        .sum()
}

#[cfg(unix)]
fn allocated_bytes(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn allocated_bytes(meta: &fs::Metadata) -> u64 {
    meta.len()
}

/// Creates `path` (and its parents) with mode 0700 for the leaf, if it is not there yet.
pub(crate) fn private_dir(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Removes the directory tree at `path`; a missing one is not an error.
fn remove_dir(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch root unique to this process and test.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-noded-snapshots-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// Writes a snapshot directory with a meta of `kind` taken at `taken_at_ms`, and a memory file
    /// of `memory` bytes.
    fn write_snapshot(
        store: &SnapshotStore,
        cid: u64,
        name: &str,
        kind: &str,
        taken_at_ms: u64,
        memory: usize,
    ) {
        let dir = store.dir(cid, name);
        fs::create_dir_all(&dir).expect("the snapshot directory");
        let state = serde_json::json!({
            "meta": {
                "format": 1,
                "vmm_version": "test",
                "arch": "x86_64",
                "kind": kind,
                "taken_at_ms": taken_at_ms,
                "vcpus": 1,
                "memory_mib": 128,
                "regions": [],
                "memory_written": memory,
            }
        });
        fs::write(dir.join("vmstate.json"), state.to_string()).expect("the state file");
        fs::write(dir.join("memory.img"), vec![7u8; memory]).expect("the memory file");
    }

    #[test]
    fn a_checkpoint_name_is_a_timestamp_and_a_lowercase_slug() {
        assert_eq!(
            SnapshotStore::checkpoint_name("Before Restore!", 42),
            "cp-42-before-restore"
        );
        assert_eq!(SnapshotStore::checkpoint_name("", 7), "cp-7");
        assert_eq!(SnapshotStore::checkpoint_name("!!", 7), "cp-7");
        assert_eq!(SnapshotStore::checkpoint_name("  fork ", 9), "cp-9-fork");
        let long = SnapshotStore::checkpoint_name(&"a".repeat(100), 1);
        assert_eq!(long, format!("cp-1-{}", "a".repeat(MAX_LABEL_LEN)));
        assert!(is_checkpoint_name(&long));
    }

    #[test]
    fn only_sleep_and_checkpoint_names_are_snapshot_names() {
        assert!(is_snapshot_name("sleep"));
        assert!(is_checkpoint_name("cp-1-x"));
        for bad in [
            "",
            "cp-",
            "cp-1/../x",
            "../sleep",
            "sleep.new",
            "CP-1",
            "cp-1_x",
            "vmstate",
        ] {
            assert!(!is_snapshot_name(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn checkpoints_list_newest_first_and_prune_keeps_the_newest() {
        let root = scratch("prune");
        let store = SnapshotStore::new(root.clone());
        write_snapshot(&store, 20, SLEEP, "full", 1, 4096);
        for i in 0..(KEEP_CHECKPOINTS as u64 + 3) {
            let name = format!("cp-{}-n", 1000 + i);
            write_snapshot(&store, 20, &name, "diff", 1000 + i, 4096);
        }
        let listed = store.checkpoints(20);
        assert_eq!(listed.len(), KEEP_CHECKPOINTS + 3);
        assert_eq!(
            listed[0].name,
            format!("cp-{}-n", 1000 + KEEP_CHECKPOINTS as u64 + 2)
        );
        assert!(listed.iter().all(|view| view.name != SLEEP));

        store.prune(20);
        let kept = store.checkpoints(20);
        assert_eq!(kept.len(), KEEP_CHECKPOINTS);
        assert_eq!(kept.last().map(|view| view.taken_at_ms), Some(1003));
        assert!(
            store.view(20, SLEEP).is_some(),
            "pruning never touches the sleep snapshot"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_view_reads_the_meta_and_names_the_label_from_the_slug() {
        let root = scratch("view");
        let store = SnapshotStore::new(root.clone());
        write_snapshot(&store, 21, "cp-500-before-restore", "diff", 500, 8192);
        let view = store
            .view(21, "cp-500-before-restore")
            .expect("the checkpoint");
        assert_eq!(view.cid, 21);
        assert_eq!(view.label, "before-restore");
        assert_eq!(view.kind, "diff");
        assert_eq!(view.taken_at_ms, 500);
        assert_eq!(view.memory_written, 8192);
        assert!(view.disk_bytes > 0, "a written memory file takes blocks");
        assert_eq!(view.paused_ms, 0);

        write_snapshot(&store, 21, SLEEP, "full", 900, 4096);
        let sleep = store.view(21, SLEEP).expect("the sleep snapshot");
        assert_eq!(sleep.label, "");
        assert_eq!(sleep.kind, "full");

        assert!(store.view(21, "cp-missing-x").is_none());
        assert!(
            store.view(21, "../cid-21").is_none(),
            "a non-snapshot name has no view"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_refuses_a_name_outside_the_snapshot_tree_and_accepts_a_missing_one() {
        let root = scratch("remove");
        let store = SnapshotStore::new(root.clone());
        write_snapshot(&store, 22, "cp-1-a", "full", 1, 4096);
        let refused = store
            .remove(22, "../cid-22")
            .expect_err("not a snapshot name");
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
        assert!(store.dir(22, "cp-1-a").exists());

        store.remove(22, "cp-1-a").expect("a checkpoint is removed");
        assert!(!store.dir(22, "cp-1-a").exists());
        store
            .remove(22, "cp-1-a")
            .expect("a missing checkpoint is not an error");

        write_snapshot(&store, 22, SLEEP, "full", 2, 4096);
        store.remove_all(22).expect("every snapshot of the VM goes");
        assert!(!store.dir(22, SLEEP).exists());
        assert!(store.checkpoints(22).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn a_new_store_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("private");
        let store = SnapshotStore::new(root.join("snapshots"));
        store.prepare(23).expect("the VM's directory");
        let mode = fs::metadata(store.dir(23, ""))
            .expect("the directory")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        let _ = fs::remove_dir_all(&root);
    }
}
