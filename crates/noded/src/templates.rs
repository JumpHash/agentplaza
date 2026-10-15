//! Warm templates: new agent VMs start by restoring a snapshot instead of booting.
//!
//! A template is a VM of one exact **shape** — guest kernel and initrd, base workspace image, vCPUs,
//! memory, balloon, and the whole kernel command line (role and `plaza.vm` included) — booted once
//! with `plaza.template=1` until its harness prints `agent-harness: template ready`, then paused and
//! snapshotted (a full snapshot) together with its workspace disk as it was at that moment. A new
//! agent VM of the same shape then starts as a reflinked copy of that disk plus a restore of that
//! memory: the harness, still in its 25 ms connect loop, reaches the hub moments after the restore,
//! and the hub's welcome sets its clock and mixes fresh entropy into it (see `apz_proto::wire::HostFacts`).
//! (CubeSandbox starts every sandbox the same way: a template boot, a memory snapshot, then restores.)
//!
//! **The key** is a SHA-256 over the shape and the images' contents (their SHA-256s, computed once per
//! image file and cached by path, size and mtime), plus the VMM's version — a template is never
//! restored by a VMM that cannot read it. A changed kernel, initrd or base image makes a new key, so a
//! rebuilt guest image never restores an old template.
//!
//! **Building** happens on first use and blocks only the calls that need that key; two calls for the
//! same key build it once. A build that fails (no harness marker within [`BUILD_TIMEOUT`]) is
//! reported to the caller, which then boots its VM cold, and is not retried for [`RETRY_AFTER`].
//!
//! Layout under the snapshot root (`crate::snapshots`): `templates/<key>/{snap/, workspace.img,
//! template.json}`.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

/// How long a template boot may take to print its marker.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a key whose build failed is left alone before another build is tried.
pub const RETRY_AFTER: Duration = Duration::from_secs(600);
/// The serial line the harness prints in template mode once it is ready to be snapshotted.
pub const READY_MARKER: &str = "agent-harness: template ready";
/// The CID a template VM boots with. Its vsock socket is private to the template's directory, so it
/// never collides with an agent's.
pub const TEMPLATE_CID: u64 = 3;

/// What a template is built for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    /// The base workspace image new agents start from.
    pub workspace_base: PathBuf,
    pub vcpus: u8,
    pub memory_mib: usize,
    pub balloon: Option<u32>,
    /// The agent VM's whole kernel command line, without `plaza.template=1` (added for the build).
    pub cmdline: String,
}

/// A template, as `GET /v1/templates` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemplateView {
    pub key: String,
    pub role: Option<String>,
    pub vcpus: u8,
    pub memory_mib: usize,
    pub built_at_ms: u64,
    pub build_ms: u64,
    pub disk_bytes: u64,
}

/// A template ready to restore from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub key: String,
    /// The snapshot directory to restore from (`VmSpec::restore`).
    pub snapshot: PathBuf,
    /// The template's workspace disk: clone it for a new VM (`vmm::clonefile::clone_file`).
    pub workspace: PathBuf,
}

/// The node's templates.
pub struct Templates {
    root: PathBuf,
}

impl Templates {
    /// Templates under `root` (`<snapshot root>/templates`).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The template for `shape`, built now when there is none (see the module docs). `uds_dir` is a
    /// directory the build VM's private vsock socket may be bound in.
    pub fn get_or_build(&self, shape: &Shape) -> Result<Template, String> {
        let _ = (shape, &self.root);
        todo!("warm template")
    }

    /// Every built template.
    pub fn list(&self) -> Vec<TemplateView> {
        todo!("list templates")
    }

    /// Removes template `key` (a VM restored from it keeps running: its files stay mapped).
    pub fn remove(&self, key: &str) -> std::io::Result<()> {
        let _ = key;
        todo!("remove template")
    }
}
