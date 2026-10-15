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

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vmm::clonefile::clone_file;
use vmm::spawner::{self, SnapshotKind, VmHandle, VmSpec, VsockConfig};

use crate::snapshots::WORKSPACE_FILE;

/// The directory a template's snapshot is written to.
const SNAP_DIR: &str = "snap";
/// The file that marks a template complete: it is written last, before the build directory is renamed.
const TEMPLATE_FILE: &str = "template.json";
/// Infix of a build's scratch directory (`<key>.building-<pid>`); such a directory is never a template.
const BUILDING_INFIX: &str = ".building-";
/// The build VM's vsock base name inside its directory.
const SOCKET_NAME: &str = "v";
/// The longest socket path a build VM may bind. A unix socket address holds 108 bytes, and the muxer
/// appends `_<port>` to the path for each connection.
const MAX_SOCKET_PATH: usize = 96;
/// How often a build polls the serial capture for the marker.
const READY_POLL: Duration = Duration::from_millis(20);

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

/// A template, as `GET /v1/templates` reports it. The same fields are the template's `template.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Image digests, by path, with the size and mtime they were computed at (see [`Templates::digest`]).
    digests: Mutex<HashMap<PathBuf, CachedDigest>>,
    /// One gate per key: a build holds its key's gate, so a second caller for that key waits for it.
    gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// The keys whose last build failed, and when it failed.
    failed: Mutex<HashMap<String, Instant>>,
}

/// An image's digest, valid while the file keeps its size and mtime.
struct CachedDigest {
    len: u64,
    mtime: SystemTime,
    sha256: [u8; 32],
}

impl Templates {
    /// Templates under `root` (`<snapshot root>/templates`).
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            digests: Mutex::default(),
            gates: Mutex::default(),
            failed: Mutex::default(),
        }
    }

    /// The template for `shape`, built now when there is none (see the module docs). The build VM's
    /// vsock socket is bound in the build's own directory, or in a private directory under the temp
    /// dir when that path is too long for a unix socket.
    pub fn get_or_build(&self, shape: &Shape) -> Result<Template, String> {
        let key = self.key(shape)?;
        if let Some(template) = self.ready(&key) {
            return Ok(template);
        }
        let gate = lock(&self.gates).entry(key.clone()).or_default().clone();
        let _building = gate.lock().unwrap_or_else(PoisonError::into_inner);
        // Another caller may have built it while this one waited for the gate.
        if let Some(template) = self.ready(&key) {
            return Ok(template);
        }
        let failed_at = lock(&self.failed).get(&key).copied();
        if let Some(at) = failed_at.filter(|at| at.elapsed() < RETRY_AFTER) {
            return Err(format!(
                "template {key} failed to build {}s ago; not retried before {}s",
                at.elapsed().as_secs(),
                RETRY_AFTER.as_secs()
            ));
        }
        match self.build(&key, shape) {
            Ok(template) => {
                lock(&self.failed).remove(&key);
                Ok(template)
            }
            Err(err) => {
                lock(&self.failed).insert(key, Instant::now());
                Err(err)
            }
        }
    }

    /// Every built template, by key. A build in progress is not one.
    pub fn list(&self) -> Vec<TemplateView> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut views: Vec<TemplateView> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let key = entry.file_name().into_string().ok()?;
                if key.contains(BUILDING_INFIX) {
                    return None;
                }
                let text = fs::read(entry.path().join(TEMPLATE_FILE)).ok()?;
                let mut view: TemplateView = serde_json::from_slice(&text).ok()?;
                view.key = key;
                Some(view)
            })
            .collect();
        views.sort_by(|a, b| a.key.cmp(&b.key));
        views
    }

    /// Removes template `key` (a VM restored from it keeps running: its files stay mapped). A missing
    /// template is not an error.
    pub fn remove(&self, key: &str) -> io::Result<()> {
        if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{key:?} is not a template key"),
            ));
        }
        match fs::remove_dir_all(self.root.join(key)) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// The template at `key` when it is complete.
    fn ready(&self, key: &str) -> Option<Template> {
        self.root
            .join(key)
            .join(TEMPLATE_FILE)
            .is_file()
            .then(|| self.template(key))
    }

    fn template(&self, key: &str) -> Template {
        let dir = self.root.join(key);
        Template {
            key: key.to_string(),
            snapshot: dir.join(SNAP_DIR),
            workspace: dir.join(WORKSPACE_FILE),
        }
    }

    /// Builds the template `key` of `shape` in a scratch directory and publishes it by renaming that
    /// directory to `root/<key>`. Whatever fails, the scratch directory and the build VM are gone.
    fn build(&self, key: &str, shape: &Shape) -> Result<Template, String> {
        let started = Instant::now();
        fs::create_dir_all(&self.root)
            .map_err(|err| format!("create {}: {err}", self.root.display()))?;
        let dir = self
            .root
            .join(format!("{key}{BUILDING_INFIX}{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        spawner::create_uds_dir(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
        let _dir = Scratch(dir.clone());
        clone_file(&shape.workspace_base, &dir.join(WORKSPACE_FILE)).map_err(|err| {
            format!(
                "copy the base workspace {}: {err}",
                shape.workspace_base.display()
            )
        })?;
        let (socket, _socket_dir) = socket_path(&dir, key)?;
        boot_and_snapshot(&dir, &socket, shape)?;
        write_template(&dir, key, shape, started)?;
        fs::rename(&dir, self.root.join(key))
            .map_err(|err| format!("publish template {key}: {err}"))?;
        Ok(self.template(key))
    }

    /// The key of `shape`: a SHA-256 over the snapshot format, this crate's version, the images'
    /// digests and the shape's numbers and command line. Every field is length-prefixed.
    fn key(&self, shape: &Shape) -> Result<String, String> {
        let mut hasher = Sha256::new();
        field(
            &mut hasher,
            "format",
            &vmm::vstate::snapshot::FORMAT.to_le_bytes(),
        );
        field(&mut hasher, "noded", env!("CARGO_PKG_VERSION").as_bytes());
        field(&mut hasher, "kernel", &self.digest(&shape.kernel)?);
        let initrd = match &shape.initrd {
            Some(path) => self.digest(path)?.to_vec(),
            None => Vec::new(),
        };
        field(&mut hasher, "initrd", &initrd);
        field(
            &mut hasher,
            "workspace",
            &self.digest(&shape.workspace_base)?,
        );
        field(&mut hasher, "vcpus", &[shape.vcpus]);
        field(
            &mut hasher,
            "memory_mib",
            &u64::try_from(shape.memory_mib)
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        let mut balloon = Vec::new();
        match shape.balloon {
            Some(mib) => {
                balloon.push(1);
                balloon.extend(mib.to_le_bytes());
            }
            None => balloon.push(0),
        }
        field(&mut hasher, "balloon", &balloon);
        field(&mut hasher, "cmdline", shape.cmdline.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        Ok(hex(&digest))
    }

    /// The SHA-256 of the file at `path`. It is hashed once per (path, size, mtime): a file that keeps
    /// its size and mtime keeps its digest, and one that changes in either is hashed again.
    fn digest(&self, path: &Path) -> Result<[u8; 32], String> {
        let meta = fs::metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
        let len = meta.len();
        let mtime = meta
            .modified()
            .map_err(|err| format!("{}: {err}", path.display()))?;
        if let Some(cached) = lock(&self.digests).get(path) {
            if cached.len == len && cached.mtime == mtime {
                return Ok(cached.sha256);
            }
        }
        let sha256 = hash_file(path).map_err(|err| format!("hash {}: {err}", path.display()))?;
        lock(&self.digests).insert(path.to_path_buf(), CachedDigest { len, mtime, sha256 });
        Ok(sha256)
    }
}

/// Boots the build VM in `dir` with its workspace and vsock socket, waits for the harness's marker,
/// snapshots it into `dir/snap` and stops it. The VM is stopped whatever the snapshot's outcome.
fn boot_and_snapshot(dir: &Path, socket: &Path, shape: &Shape) -> Result<(), String> {
    let spec = VmSpec {
        kernel: shape.kernel.clone(),
        cmdline: format!("{} plaza.template=1", shape.cmdline),
        vcpus: shape.vcpus,
        memory_mib: shape.memory_mib,
        initrd: shape.initrd.clone(),
        vsock: Some(VsockConfig {
            guest_cid: TEMPLATE_CID,
            uds_path: socket.to_path_buf(),
        }),
        workspace: Some(dir.join(WORKSPACE_FILE)),
        balloon: shape.balloon,
        ..VmSpec::default()
    };
    let mut vm = spawner::spawn(spec).map_err(|err| format!("boot the template VM: {err}"))?;
    let snap = dir.join(SNAP_DIR);
    let snapshot = wait_ready(&mut vm).and_then(|()| {
        fs::create_dir_all(&snap).map_err(|err| format!("create {}: {err}", snap.display()))?;
        vm.snapshot(&snap, SnapshotKind::Full)
            .map(|_| ())
            .map_err(|err| format!("snapshot the template VM: {err}"))
    });
    let stopped = vm
        .kill()
        .map_err(|err| format!("stop the template VM: {err}"));
    snapshot?;
    stopped
}

/// Waits until the build VM prints [`READY_MARKER`]. Fails when the VM stops first, or after
/// [`BUILD_TIMEOUT`].
fn wait_ready(vm: &mut VmHandle) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if vm.serial().contains(READY_MARKER) {
            return Ok(());
        }
        if let Some(exit) = vm
            .wait_timeout(Duration::ZERO)
            .map_err(|err| format!("watch the template VM: {err}"))?
        {
            return Err(format!(
                "the template VM stopped ({exit:?}) before it printed `{READY_MARKER}`"
            ));
        }
        if started.elapsed() >= BUILD_TIMEOUT {
            return Err(format!(
                "the template VM printed no `{READY_MARKER}` within {}s",
                BUILD_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(READY_POLL);
    }
}

/// The build VM's vsock base path, and the private directory holding it when that is not `dir/v`
/// (the path would be too long for a unix socket).
fn socket_path(dir: &Path, key: &str) -> Result<(PathBuf, Option<Scratch>), String> {
    let inside = dir.join(SOCKET_NAME);
    if inside.as_os_str().len() <= MAX_SOCKET_PATH {
        return Ok((inside, None));
    }
    let short = std::env::temp_dir().join(format!("apz-tpl-{}-{}", std::process::id(), &key[..12]));
    let _ = fs::remove_dir_all(&short);
    spawner::create_uds_dir(&short).map_err(|err| format!("create {}: {err}", short.display()))?;
    let socket = short.join(SOCKET_NAME);
    Ok((socket, Some(Scratch(short))))
}

/// Writes the template's `template.json` into `dir`.
fn write_template(dir: &Path, key: &str, shape: &Shape, started: Instant) -> Result<(), String> {
    let disk_bytes = fs::metadata(dir.join(WORKSPACE_FILE))
        .map_err(|err| format!("stat the template workspace: {err}"))?
        .len();
    let view = TemplateView {
        key: key.to_string(),
        role: role_of(&shape.cmdline),
        vcpus: shape.vcpus,
        memory_mib: shape.memory_mib,
        built_at_ms: now_ms(),
        build_ms: millis(started.elapsed()),
        disk_bytes,
    };
    let json = serde_json::to_vec(&view).map_err(|err| format!("encode template {key}: {err}"))?;
    write_private(&dir.join(TEMPLATE_FILE), &json)
        .map_err(|err| format!("write template {key}: {err}"))
}

/// The `plaza.role=` of a kernel command line, when it names one.
fn role_of(cmdline: &str) -> Option<String> {
    cmdline
        .split_whitespace()
        .find_map(|word| word.strip_prefix("plaza.role="))
        .filter(|role| !role.is_empty())
        .map(str::to_string)
}

/// Feeds one named, length-prefixed field into `hasher`, so no two shapes feed the same bytes.
fn field(hasher: &mut Sha256, name: &str, value: &[u8]) {
    hasher.update(u64::try_from(name.len()).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(name);
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(value);
}

/// The SHA-256 of the file at `path`, read in chunks.
fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(hasher.finalize().into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Writes `data` to `path` (created or truncated), readable by its owner only.
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

/// A path removed when the guard drops, so a build leaves nothing behind however it ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Locks `mutex`. A poisoned lock is taken anyway: the maps behind these locks hold no state that a
/// panic could leave half-updated.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, millis)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the temp dir, removed when the returned guard drops.
    fn test_dir(name: &str) -> (PathBuf, Scratch) {
        let dir =
            std::env::temp_dir().join(format!("noded-templates-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the test directory");
        (dir.clone(), Scratch(dir))
    }

    /// A shape over fake images in `dir`: the kernel, initrd and base workspace are small files.
    fn fake_shape(dir: &Path) -> Shape {
        let kernel = dir.join("vmlinux");
        let initrd = dir.join("initrd.gz");
        let workspace_base = dir.join("workspace.ext4");
        fs::write(&kernel, b"kernel image bytes").unwrap();
        fs::write(&initrd, b"initrd bytes").unwrap();
        fs::write(&workspace_base, vec![7u8; 4096]).unwrap();
        Shape {
            kernel,
            initrd: Some(initrd),
            workspace_base,
            vcpus: 2,
            memory_mib: 1024,
            balloon: None,
            cmdline: "console=ttyS0 plaza.role=workstation".to_string(),
        }
    }

    /// Replaces a file's bytes and sets its mtime, so the digest cache sees exactly what the test says.
    fn rewrite(path: &Path, bytes: &[u8], mtime: SystemTime) {
        fs::write(path, bytes).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    fn view(key: &str, role: Option<&str>) -> TemplateView {
        TemplateView {
            key: key.to_string(),
            role: role.map(str::to_string),
            vcpus: 2,
            memory_mib: 1024,
            built_at_ms: 1_700_000_000_000,
            build_ms: 4200,
            disk_bytes: 4096,
        }
    }

    #[test]
    fn key_is_stable_for_the_same_shape() {
        let (dir, _clean) = test_dir("key-stable");
        let shape = fake_shape(&dir);
        let templates = Templates::new(dir.join("templates"));
        let key = templates.key(&shape).unwrap();
        assert_eq!(key.len(), 64);
        assert_eq!(templates.key(&shape).unwrap(), key);
        // A fresh digest cache computes the same key.
        assert_eq!(
            Templates::new(dir.join("templates")).key(&shape).unwrap(),
            key
        );
    }

    #[test]
    fn key_follows_the_shape_and_the_images() {
        let (dir, _clean) = test_dir("key-sensitive");
        let shape = fake_shape(&dir);
        let templates = Templates::new(dir.join("templates"));
        let base = templates.key(&shape).unwrap();
        let key_of = |changed: Shape| templates.key(&changed).unwrap();
        let cmdline = format!("{} plaza.vm=1", shape.cmdline);
        assert_ne!(
            key_of(Shape {
                cmdline,
                ..shape.clone()
            }),
            base
        );
        assert_ne!(
            key_of(Shape {
                memory_mib: 2048,
                ..shape.clone()
            }),
            base
        );
        assert_ne!(
            key_of(Shape {
                vcpus: 4,
                ..shape.clone()
            }),
            base
        );
        assert_ne!(
            key_of(Shape {
                balloon: Some(256),
                ..shape.clone()
            }),
            base
        );
        assert_ne!(
            key_of(Shape {
                initrd: None,
                ..shape.clone()
            }),
            base
        );
        // One byte of the kernel changes, same size, with a later mtime.
        let mut kernel = fs::read(&shape.kernel).unwrap();
        kernel[0] ^= 0xff;
        rewrite(
            &shape.kernel,
            &kernel,
            SystemTime::now() + Duration::from_secs(3600),
        );
        assert_ne!(key_of(shape.clone()), base);
    }

    #[test]
    fn digest_is_cached_by_size_and_mtime() {
        let (dir, _clean) = test_dir("digest-cache");
        let shape = fake_shape(&dir);
        let templates = Templates::new(dir.join("templates"));
        let mtime = fs::metadata(&shape.kernel).unwrap().modified().unwrap();
        let first = templates.digest(&shape.kernel).unwrap();
        // Same size and mtime: the cache answers, so the new bytes are not read.
        let mut changed = fs::read(&shape.kernel).unwrap();
        changed[0] ^= 0xff;
        rewrite(&shape.kernel, &changed, mtime);
        assert_eq!(templates.digest(&shape.kernel).unwrap(), first);
        // A new mtime: the file is hashed again.
        rewrite(&shape.kernel, &changed, mtime + Duration::from_secs(60));
        let again = templates.digest(&shape.kernel).unwrap();
        assert_ne!(again, first);
        assert_eq!(again, hash_file(&shape.kernel).unwrap());
    }

    #[test]
    fn an_existing_template_is_returned_without_a_build() {
        let (dir, _clean) = test_dir("existing");
        let shape = fake_shape(&dir);
        let templates = Templates::new(dir.join("templates"));
        let key = templates.key(&shape).unwrap();
        let built = dir.join("templates").join(&key);
        fs::create_dir_all(&built).unwrap();
        let json = serde_json::to_vec(&view(&key, Some("workstation"))).unwrap();
        write_private(&built.join(TEMPLATE_FILE), &json).unwrap();
        // The fake kernel cannot boot, so a build here would fail.
        let template = templates.get_or_build(&shape).unwrap();
        assert_eq!(template.key, key);
        assert_eq!(template.snapshot, built.join(SNAP_DIR));
        assert_eq!(template.workspace, built.join(WORKSPACE_FILE));
    }

    #[test]
    fn list_and_remove_cover_built_templates() {
        let (dir, _clean) = test_dir("list-remove");
        let root = dir.join("templates");
        let key = "ab".repeat(32);
        let built = root.join(&key);
        fs::create_dir_all(&built).unwrap();
        let json = serde_json::to_vec(&view(&key, None)).unwrap();
        write_private(&built.join(TEMPLATE_FILE), &json).unwrap();
        // A build in progress has a template.json too, and is still not a template.
        let building = root.join(format!("{key}{BUILDING_INFIX}7"));
        fs::create_dir_all(&building).unwrap();
        write_private(&building.join(TEMPLATE_FILE), &json).unwrap();

        let templates = Templates::new(root.clone());
        assert_eq!(templates.list(), vec![view(&key, None)]);
        templates.remove(&key).unwrap();
        assert!(!built.exists());
        assert!(templates.list().is_empty());
        // Removing a missing template is not an error; a key that is not a key is refused.
        templates.remove(&key).unwrap();
        let refused = templates.remove("../escape").unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_failed_build_leaves_nothing_and_is_not_retried() {
        let (dir, _clean) = test_dir("failed-build");
        let shape = fake_shape(&dir);
        let templates = Templates::new(dir.join("templates"));
        // The fake kernel does not boot: the build fails at the VM, not at the key.
        assert!(templates.get_or_build(&shape).is_err());
        let leftovers = fs::read_dir(dir.join("templates")).unwrap().count();
        assert_eq!(leftovers, 0, "a failed build leaves no directory behind");
        let again = templates.get_or_build(&shape).unwrap_err();
        assert!(again.contains("not retried"), "{again}");
    }

    /// Boots the real guest, builds its template and checks that the next call reuses it. Needs the
    /// guest images under `target/guest`; skipped (with a note) when they are not there.
    #[test]
    #[ignore = "boots the guest kernel; needs target/guest images and /dev/kvm"]
    fn builds_a_template_from_the_guest() {
        let guest = Path::new("/mnt/fast/AgentPlaza/target/guest");
        let kernel = guest.join("x86_64/vmlinux");
        let initrd = guest.join("x86_64/initrd.gz");
        let workspace = guest.join("workspace.ext4");
        if ![&kernel, &initrd, &workspace]
            .iter()
            .all(|path| path.is_file())
        {
            eprintln!("skipped: no guest images under {}", guest.display());
            return;
        }
        let (dir, _clean) = test_dir("e2e");
        let templates = Templates::new(dir.join("templates"));
        let shape = Shape {
            kernel,
            initrd: Some(initrd),
            workspace_base: workspace,
            vcpus: 1,
            memory_mib: 512,
            balloon: None,
            cmdline: "console=ttyS0 loglevel=4 plaza.role=workstation".to_string(),
        };
        let started = Instant::now();
        let template = templates.get_or_build(&shape).expect("build the template");
        eprintln!("built template {} in {:?}", template.key, started.elapsed());
        assert!(
            template
                .snapshot
                .join(vmm::vstate::snapshot::STATE_FILE)
                .is_file()
        );
        assert!(template.workspace.is_file());
        let started = Instant::now();
        assert_eq!(templates.get_or_build(&shape).expect("reuse"), template);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(templates.list().len(), 1);
    }
}
