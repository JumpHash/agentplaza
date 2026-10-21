//! Moving a sleeping VM from one node to another: the source node exports it, the target imports it.
//!
//! A sleeping VM is its sleep snapshot (`cid-<cid>/sleep/{memory.img, vmstate.json}`), its workspace
//! image and its shape ([`VmMeta`]). Nothing here runs the VM: an export reads the stopped VM's
//! files (none for a VM with no workspace volume), and an import stages them and makes the VM asleep on the target, where `wake` restores it.
//!
//! The hub relays the bytes through each node's command queue, whose replies are bounded JSON, so
//! every transfer is chunked and the hub never holds a whole image: an export is read in pieces of
//! at most [`MAX_READ`] bytes, and an import is written in pieces of at most [`MAX_WRITE`] bytes
//! (kept under the node's request body limit, base64 included).
//!
//! Exports and imports live in memory and expire after [`TTL`]; an expired import's staging
//! directory goes with it. An export holds its files open, so a sleep snapshot replaced while the
//! export is being read still yields the bytes the export hashed. A workspace image the VM wrote
//! after a wake would not match the hash, and the target refuses the import rather than keep a
//! torn image.
//!
//! An import declares each file's length before anything is staged: a declaration over the maximum
//! for its file is refused, and at most [`MAX_IMPORTS`] imports are in flight on a node at once; the
//! rest are answered 503 until one finishes, aborts or expires.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::registry::{ImportedVm, Registry, Run, VmOpError, check_import_shape};
use crate::snapshots::{self, MEMORY_FILE, SLEEP, WORKSPACE_FILE};

/// How long an export or an import stays in memory after it was made.
pub const TTL: Duration = Duration::from_secs(60 * 60);
/// The most bytes one export read returns, before base64.
pub const MAX_READ: u64 = 4 << 20;
/// The most bytes one import write takes, before base64.
pub const MAX_WRITE: usize = 512 << 10;
/// The sleep snapshot's VM state file.
const STATE_FILE: &str = "vmstate.json";
/// The files of a sleeping VM with a workspace volume, in the order an export lists them.
const FILES: [&str; 3] = [MEMORY_FILE, STATE_FILE, WORKSPACE_FILE];
/// The files of a sleeping VM with no workspace volume: its sleep snapshot alone, since its
/// `/workspace` lives in guest RAM and the memory snapshot holds it.
const SLEEP_FILES: [&str; 2] = [MEMORY_FILE, STATE_FILE];
/// The most bytes one read of a file hashes at a time.
const HASH_BUF: usize = 1 << 20;
/// The most imports that may be in flight on this node at once. An import holds its staging files
/// until it finishes, aborts or expires; one more is answered 503.
pub const MAX_IMPORTS: usize = 4;
/// The most bytes a state file may declare: a VM's registers, its devices' transports and a serial
/// buffer.
const MAX_STATE_FILE: u64 = 16 << 20;
/// How far past the VM's configured memory a memory file may declare.
const MEMORY_SLACK: u64 = 1 << 20;
/// The most bytes a workspace image may declare. An import does not know its volume's configured
/// size, so this bounds every image.
const MAX_WORKSPACE_FILE: u64 = 1 << 40;

/// The shape of the VM a transfer carries: what the target needs to make it asleep there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmMeta {
    /// The guest CID the VM keeps.
    pub cid: u64,
    /// The role the VM runs as, when it has one.
    #[serde(default)]
    pub role: Option<String>,
    pub vcpus: u32,
    pub memory_mib: u32,
    /// The usable guest memory at boot, when the VM has a balloon.
    #[serde(default)]
    pub balloon_usable_mib: Option<u32>,
    /// The VM's kernel command line.
    pub cmdline: String,
}

/// One file of a transfer: its name, its length and the SHA-256 of its bytes, in lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub len: u64,
    pub sha256: String,
}

/// The `POST /v1/imports` body: the export's `meta` and `files`, as the export replied them.
#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub meta: VmMeta,
    pub files: Vec<FileEntry>,
}

/// An export's reply: its id, its files and the VM's shape.
#[derive(Debug, Clone, Serialize)]
pub struct ExportView {
    pub export: String,
    pub files: Vec<FileEntry>,
    pub meta: VmMeta,
}

/// One chunk of an export's file: base64 data, and whether the chunk reaches the file's end.
#[derive(Debug, Serialize)]
pub struct Chunk {
    pub data: String,
    pub eof: bool,
    /// When the whole chunk is zeros (a hole of a sparse image, most of a VM's memory): how many
    /// bytes, with `data` empty. The target leaves them unwritten, so its copy stays sparse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zeros: Option<u64>,
}

/// The reply to one import chunk: how many bytes of the file are now written.
#[derive(Debug, Serialize)]
pub struct WriteView {
    pub import: String,
    pub name: String,
    pub written: u64,
}

/// The reply to a finished import: the run that holds the VM, and its CID.
#[derive(Debug, Serialize)]
pub struct FinishView {
    pub run: String,
    pub cid: u64,
}

/// Why a transfer step did not happen.
#[derive(Debug)]
pub enum MigrateError {
    /// The registry refused: an unknown run or VM, a VM in the wrong state, a CID in use.
    Vm(VmOpError),
    /// No export or import has that id or file (or it expired).
    Unknown(String),
    /// The request is malformed or out of range.
    Invalid(String),
    /// An import's file is short, or its bytes are not the sha256 the export named.
    Mismatch(String),
    /// The node could not do its part (a file could not be read or written).
    Failed(String),
    /// The node is at its limit (too many imports in flight); retry once one has finished.
    Busy(String),
}

impl MigrateError {
    /// The HTTP status this error is reported as.
    pub fn status(&self) -> u16 {
        match self {
            MigrateError::Vm(err) => err.status(),
            MigrateError::Unknown(_) => 404,
            MigrateError::Invalid(_) => 400,
            MigrateError::Mismatch(_) => 422,
            MigrateError::Failed(_) => 500,
            MigrateError::Busy(_) => 503,
        }
    }
}

impl fmt::Display for MigrateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrateError::Vm(err) => write!(formatter, "{err}"),
            MigrateError::Unknown(reason)
            | MigrateError::Invalid(reason)
            | MigrateError::Mismatch(reason)
            | MigrateError::Busy(reason)
            | MigrateError::Failed(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl From<VmOpError> for MigrateError {
    fn from(err: VmOpError) -> Self {
        MigrateError::Vm(err)
    }
}

/// The node's exports and imports, in memory.
#[derive(Default)]
pub struct Migrations {
    state: Mutex<State>,
    /// How many imports hold an [`ImportPermit`] from this count.
    in_flight: Arc<AtomicUsize>,
}

#[derive(Default)]
struct State {
    /// The count the export and import ids are numbered from.
    seq: u64,
    exports: HashMap<String, Export>,
    imports: HashMap<String, Import>,
}

/// An export: the sleeping VM's files, open and hashed when the export was made.
struct Export {
    made: Instant,
    files: Vec<Arc<ExportFile>>,
}

/// One of an export's files: its entry and the open file the reads come from.
struct ExportFile {
    entry: FileEntry,
    file: Mutex<File>,
}

/// An import: the files it will receive, staged under `staging`.
struct Import {
    made: Instant,
    meta: VmMeta,
    files: Vec<FileEntry>,
    staging: PathBuf,
    /// The import's place in [`Migrations`]' count, given back when the import is dropped.
    _permit: ImportPermit,
}

/// A place for one import on this node. The import holds it until it leaves its [`Migrations`]
/// (finished, aborted or expired), and dropping it gives the place back.
struct ImportPermit(Arc<AtomicUsize>);

impl ImportPermit {
    /// A place for one more import, or `None` when [`MAX_IMPORTS`] are taken.
    fn take(in_flight: &Arc<AtomicUsize>) -> Option<Self> {
        in_flight
            .try_update(Ordering::AcqRel, Ordering::Acquire, |taken| {
                (taken < MAX_IMPORTS).then_some(taken + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(in_flight)))
    }
}

impl Drop for ImportPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Migrations {
    pub fn new() -> Self {
        Self::default()
    }

    /// The state, with the entries that expired by now removed.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.lock_at(Instant::now())
    }

    /// The state with the entries that expired by `now` removed.
    fn lock_at(&self, now: Instant) -> MutexGuard<'_, State> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.purge(now);
        state
    }
}

impl State {
    /// Removes the exports and imports that are older than [`TTL`] at `now`. An expired import's
    /// staging directory is removed with it.
    fn purge(&mut self, now: Instant) {
        self.exports
            .retain(|_, export| now.saturating_duration_since(export.made) < TTL);
        let expired: Vec<String> = self
            .imports
            .iter()
            .filter(|(_, import)| now.saturating_duration_since(import.made) >= TTL)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(import) = self.imports.remove(&id) {
                let _ = fs::remove_dir_all(&import.staging);
            }
        }
    }
}

/// `POST /v1/runs/{id}/vms/{index}/export`: records the files of asleep VM `vm` of run `run` for a
/// transfer, and replies with their names, lengths and hashes. A VM that is not asleep is refused.
pub fn export_vm(
    migrations: &Migrations,
    registry: &Registry,
    run: &str,
    vm: u32,
) -> Result<ExportView, MigrateError> {
    let asleep = registry.asleep_vm(run, vm)?;
    let spec = &asleep.spec;
    let mut sources = vec![
        (MEMORY_FILE, asleep.sleep_dir.join(MEMORY_FILE)),
        (STATE_FILE, asleep.sleep_dir.join(STATE_FILE)),
    ];
    // A VM with no workspace volume exports its sleep snapshot alone.
    if let Some(workspace) = &spec.workspace {
        sources.push((WORKSPACE_FILE, workspace.clone()));
    }
    let files = sources
        .iter()
        .map(|(name, path)| open_export_file(name, path).map(Arc::new))
        .collect::<Result<Vec<_>, _>>()?;
    let meta = VmMeta {
        cid: asleep.cid,
        role: asleep.role,
        vcpus: u32::from(spec.vcpus),
        memory_mib: u32::try_from(spec.memory_mib)
            .map_err(|_| MigrateError::Failed("the VM's memory is too large to export".into()))?,
        balloon_usable_mib: spec.balloon,
        cmdline: spec.cmdline.clone(),
    };
    let entries = files.iter().map(|file| file.entry.clone()).collect();
    let mut state = migrations.lock();
    state.seq += 1;
    let export = format!("x{}", state.seq);
    state.exports.insert(
        export.clone(),
        Export {
            made: Instant::now(),
            files,
        },
    );
    Ok(ExportView {
        export,
        files: entries,
        meta,
    })
}

/// Opens an export's file and hashes it, so the export names what its reads will return.
fn open_export_file(name: &str, path: &Path) -> Result<ExportFile, MigrateError> {
    let mut file = File::open(path)
        .map_err(|err| MigrateError::Failed(format!("cannot open {name}: {err}")))?;
    let len = file
        .metadata()
        .map_err(|err| MigrateError::Failed(format!("cannot read {name}'s size: {err}")))?
        .len();
    let sha256 = hash_file(&mut file)
        .map_err(|err| MigrateError::Failed(format!("cannot hash {name}: {err}")))?;
    Ok(ExportFile {
        entry: FileEntry {
            name: name.to_string(),
            len,
            sha256,
        },
        file: Mutex::new(file),
    })
}

/// `GET /v1/exports/{export}/{name}?offset=O&len=L`: up to `len` bytes of the export's file `name`
/// from `offset`, at most [`MAX_READ`] of them, base64 encoded.
pub fn read_export(
    migrations: &Migrations,
    export: &str,
    name: &str,
    query: &str,
) -> Result<Chunk, MigrateError> {
    let offset = query_u64(query, "offset")?.unwrap_or(0);
    let len = query_u64(query, "len")?
        .ok_or_else(|| MigrateError::Invalid("len is required".to_string()))?;
    if len == 0 {
        return Err(MigrateError::Invalid("len must be at least 1".to_string()));
    }
    let file = {
        let state = migrations.lock();
        let found = state
            .exports
            .get(export)
            .ok_or_else(|| MigrateError::Unknown(format!("no export {export}")))?;
        found
            .files
            .iter()
            .find(|file| file.entry.name == name)
            .cloned()
            .ok_or_else(|| MigrateError::Unknown(format!("export {export} has no file {name}")))?
    };
    let size = file.entry.len;
    if offset > size {
        return Err(MigrateError::Invalid(format!(
            "offset {offset} is past the end of {name} ({size} bytes)"
        )));
    }
    let want = len.min(MAX_READ).min(size - offset);
    let mut bytes = vec![0u8; want as usize];
    {
        let mut handle = file.file.lock().unwrap_or_else(PoisonError::into_inner);
        handle
            .seek(SeekFrom::Start(offset))
            .and_then(|_| handle.read_exact(&mut bytes))
            .map_err(|err| MigrateError::Failed(format!("cannot read {name}: {err}")))?;
    }
    let eof = offset + want == size;
    if bytes.iter().all(|byte| *byte == 0) {
        return Ok(Chunk {
            data: String::new(),
            eof,
            zeros: Some(want),
        });
    }
    Ok(Chunk {
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        eof,
        zeros: None,
    })
}

/// `DELETE /v1/exports/{export}`: forgets the export and closes its files.
pub fn delete_export(migrations: &Migrations, export: &str) -> Result<(), MigrateError> {
    migrations
        .lock()
        .exports
        .remove(export)
        .map(drop)
        .ok_or_else(|| MigrateError::Unknown(format!("no export {export}")))
}

/// `POST /v1/imports`: starts an import of the VM `meta` describes, whose files are `files` (the
/// export's). Refuses a CID a run of this node holds, a file over its maximum (see `allowed`), and
/// an import while [`MAX_IMPORTS`] are in flight (503). Returns the import's id.
pub fn create_import(
    migrations: &Migrations,
    registry: &Registry,
    body: &[u8],
) -> Result<String, MigrateError> {
    let request: ImportRequest = serde_json::from_slice(body)
        .map_err(|err| MigrateError::Invalid(format!("invalid import request: {err}")))?;
    let ImportRequest { meta, mut files } = request;
    check_files(&files)?;
    // Every declared length is checked before anything is staged or written.
    for file in &files {
        allowed(&file.name, file.len, meta.memory_mib).map_err(MigrateError::Invalid)?;
    }
    for file in &mut files {
        file.sha256.make_ascii_lowercase();
    }
    check_import_shape(meta.cid, meta.role.as_deref(), meta.vcpus, meta.memory_mib)?;
    if registry.cid_in_use(meta.cid) {
        return Err(VmOpError::CidInUse(meta.cid).into());
    }
    // The import's place is held until it finishes, aborts or expires; a node at its limit says so.
    let permit = ImportPermit::take(&migrations.in_flight).ok_or_else(|| {
        MigrateError::Busy(format!(
            "the node already has {MAX_IMPORTS} imports in flight"
        ))
    })?;
    let imports = registry.snapshot_root()?.join("imports");
    snapshots::private_dir(&imports).map_err(|err| {
        MigrateError::Failed(format!("cannot create {}: {err}", imports.display()))
    })?;
    let mut state = migrations.lock();
    state.seq += 1;
    let id = format!("i{}", state.seq);
    let staging = imports.join(&id);
    snapshots::private_dir(&staging).map_err(|err| {
        MigrateError::Failed(format!("cannot create {}: {err}", staging.display()))
    })?;
    // Every file is laid down at its full length first, as a hole: chunks of zeros are never sent,
    // so what no chunk writes stays a hole and reads back as the zeros it was.
    for file in &files {
        create_private(&staging.join(&file.name))
            .and_then(|handle| handle.set_len(file.len))
            .map_err(|err| MigrateError::Failed(format!("cannot stage {}: {err}", file.name)))?;
    }
    state.imports.insert(
        id.clone(),
        Import {
            made: Instant::now(),
            meta,
            files,
            staging,
            _permit: permit,
        },
    );
    Ok(id)
}

/// The files an import carries: exactly the files a sleeping VM has (its three, or its two when it
/// has no workspace volume), each with a sha256.
fn check_files(files: &[FileEntry]) -> Result<(), MigrateError> {
    let mut names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
    names.sort_unstable();
    let sorted = |list: &[&'static str]| {
        let mut list = list.to_vec();
        list.sort_unstable();
        list
    };
    if names != sorted(&FILES) && names != sorted(&SLEEP_FILES) {
        return Err(MigrateError::Invalid(format!(
            "an import carries exactly {} (or {} for a VM with no workspace volume), not {names:?}",
            FILES.join(", "),
            SLEEP_FILES.join(", ")
        )));
    }
    for file in files {
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(MigrateError::Invalid(format!(
                "{}: sha256 must be 64 hex digits",
                file.name
            )));
        }
    }
    Ok(())
}

/// Whether an import may declare a file `kind` (its name) of `declared` bytes, for a VM of
/// `vm_memory_mib` MiB: a memory file at most the VM's memory plus [`MEMORY_SLACK`], a state file at
/// most [`MAX_STATE_FILE`], and a workspace image at most [`MAX_WORKSPACE_FILE`].
fn allowed(kind: &str, declared: u64, vm_memory_mib: u32) -> Result<(), String> {
    let (max, what) = match kind {
        MEMORY_FILE => (
            (u64::from(vm_memory_mib) << 20) + MEMORY_SLACK,
            "the VM's memory",
        ),
        STATE_FILE => (MAX_STATE_FILE, "a state file"),
        WORKSPACE_FILE => (MAX_WORKSPACE_FILE, "a workspace image"),
        other => return Err(format!("{other} is not a file an import carries")),
    };
    if declared > max {
        return Err(format!(
            "{kind} declares {declared} bytes, over the {max} allowed for {what}"
        ));
    }
    Ok(())
}

/// `POST /v1/imports/{import}/{name}`, body `{"offset": O, "data": <base64>}`: writes the chunk into
/// the staged file `name` at `offset`. A chunk is at most [`MAX_WRITE`] bytes and stays inside the
/// file's declared length, so a stream longer than its declared size is refused at the first byte
/// past it.
pub fn write_import(
    migrations: &Migrations,
    import: &str,
    name: &str,
    body: &[u8],
) -> Result<WriteView, MigrateError> {
    #[derive(Deserialize)]
    struct Write {
        offset: u64,
        data: String,
    }
    let write: Write = serde_json::from_slice(body)
        .map_err(|err| MigrateError::Invalid(format!("invalid chunk: {err}")))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&write.data)
        .map_err(|err| MigrateError::Invalid(format!("data is not base64: {err}")))?;
    if bytes.len() > MAX_WRITE {
        return Err(MigrateError::Invalid(format!(
            "a chunk is at most {MAX_WRITE} bytes, not {}",
            bytes.len()
        )));
    }
    // The state is held while the chunk is written, so a finish or an abort cannot race it.
    let state = migrations.lock();
    let found = state
        .imports
        .get(import)
        .ok_or_else(|| MigrateError::Unknown(format!("no import {import}")))?;
    let entry = found
        .files
        .iter()
        .find(|file| file.name == name)
        .ok_or_else(|| MigrateError::Invalid(format!("import {import} carries no file {name}")))?;
    let end = write
        .offset
        .checked_add(bytes.len() as u64)
        .filter(|end| *end <= entry.len)
        .ok_or_else(|| {
            MigrateError::Invalid(format!("the chunk runs past {name}'s {} bytes", entry.len))
        })?;
    let path = found.staging.join(name);
    let mut file = create_private(&path)
        .map_err(|err| MigrateError::Failed(format!("cannot open {name}: {err}")))?;
    file.seek(SeekFrom::Start(write.offset))
        .and_then(|_| file.write_all(&bytes))
        .map_err(|err| MigrateError::Failed(format!("cannot write {name}: {err}")))?;
    Ok(WriteView {
        import: import.to_string(),
        name: name.to_string(),
        written: end,
    })
}

/// `POST /v1/imports/{import}/finish`: checks every staged file's length and sha256 against the
/// import's files, then makes the VM asleep on this node. Whatever happens, the staging directory
/// goes: a failed import leaves nothing behind, and the import's id is spent.
pub fn finish_import(
    migrations: &Migrations,
    registry: &Registry,
    import: &str,
) -> Result<FinishView, MigrateError> {
    let found = migrations
        .lock()
        .imports
        .remove(import)
        .ok_or_else(|| MigrateError::Unknown(format!("no import {import}")))?;
    let placed = place(&found, registry);
    let _ = fs::remove_dir_all(&found.staging);
    let run = placed?;
    Ok(FinishView {
        run: run.id().to_string(),
        cid: found.meta.cid,
    })
}

/// Verifies an import's staged files, then hands them to the registry, which makes the VM asleep.
fn place(found: &Import, registry: &Registry) -> Result<Arc<Run>, MigrateError> {
    for entry in &found.files {
        let path = found.staging.join(&entry.name);
        let len = fs::metadata(&path)
            .map(|meta| meta.len())
            .map_err(|_| MigrateError::Mismatch(format!("{} was not received", entry.name)))?;
        if len != entry.len {
            return Err(MigrateError::Mismatch(format!(
                "{} is {len} bytes, not the {} its export named",
                entry.name, entry.len
            )));
        }
        let sha256 = hash_path(&path)
            .map_err(|err| MigrateError::Failed(format!("cannot hash {}: {err}", entry.name)))?;
        if sha256 != entry.sha256 {
            return Err(MigrateError::Mismatch(format!(
                "{} does not match the sha256 its export named",
                entry.name
            )));
        }
    }
    // The sleep snapshot's two files take a directory of their own, which the registry installs.
    let sleep = found.staging.join(SLEEP);
    snapshots::private_dir(&sleep)
        .map_err(|err| MigrateError::Failed(format!("cannot create {}: {err}", sleep.display())))?;
    for name in [MEMORY_FILE, STATE_FILE] {
        fs::rename(found.staging.join(name), sleep.join(name))
            .map_err(|err| MigrateError::Failed(format!("cannot stage {name}: {err}")))?;
    }
    let meta = &found.meta;
    let has_workspace = found.files.iter().any(|file| file.name == WORKSPACE_FILE);
    Ok(registry.import_run(ImportedVm {
        cid: meta.cid,
        role: meta.role.clone(),
        vcpus: meta.vcpus,
        memory_mib: meta.memory_mib,
        balloon_usable_mib: meta.balloon_usable_mib,
        cmdline: meta.cmdline.clone(),
        sleep_dir: sleep,
        workspace: has_workspace.then(|| found.staging.join(WORKSPACE_FILE)),
    })?)
}

/// `DELETE /v1/imports/{import}`: aborts an import and removes what it staged.
pub fn abort_import(migrations: &Migrations, import: &str) -> Result<(), MigrateError> {
    let found = migrations
        .lock()
        .imports
        .remove(import)
        .ok_or_else(|| MigrateError::Unknown(format!("no import {import}")))?;
    let _ = fs::remove_dir_all(&found.staging);
    Ok(())
}

/// The value of query parameter `key` as a whole number: `None` when absent.
fn query_u64(query: &str, key: &str) -> Result<Option<u64>, MigrateError> {
    for pair in query.split('&') {
        if let Some((name, value)) = pair.split_once('=')
            && name == key
        {
            return value
                .parse()
                .map(Some)
                .map_err(|_| MigrateError::Invalid(format!("{key} must be a whole number")));
        }
    }
    Ok(None)
}

/// Opens `path` for writing, created 0600: a transfer's files are guest memory and workspace data.
fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The SHA-256 of the file at `path`, in lowercase hex.
fn hash_path(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    hash_file(&mut file)
}

/// The SHA-256 of `file` from its current position to its end, in lowercase hex.
fn hash_file(file: &mut File) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use crate::registry::{VmImages, VmState};
    use crate::snapshots::SnapshotStore;

    /// A scratch directory unique to this process and test.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-noded-migrate-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A node rooted at `root`: its snapshots under `root/snapshots`, its sockets under `root/uds`,
    /// and its guest images at `root/vmlinux` and `root/initrd.gz`.
    fn node_at(root: &Path) -> Registry {
        let images = VmImages {
            kernel: root.join("vmlinux"),
            initrd: root.join("initrd.gz"),
            workspace: None,
        };
        Registry::new(images, root.join("uds"), EventBus::new())
            .with_snapshots(Some(SnapshotStore::new(root.join("snapshots"))))
    }

    /// `len` bytes of a pattern that differs with `seed`.
    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| ((i % 251) as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    /// A node holding asleep VM 0 of run `r1`, imported from stand-in files of `memory` and
    /// `workspace` bytes: the import makes the VM asleep directly, so no VM has to run.
    fn asleep_node(root: &Path, cid: u64, memory: usize, workspace: usize) -> Registry {
        let node = node_at(root);
        let sleep_dir = root.join("sleep-in");
        fs::create_dir_all(&sleep_dir).expect("the stand-in sleep directory");
        fs::write(sleep_dir.join(MEMORY_FILE), pattern(memory, 3)).expect("memory");
        fs::write(sleep_dir.join(STATE_FILE), br#"{"meta":{"format":1}}"#).expect("state");
        let image = root.join("workspace-in.ext4");
        fs::write(&image, pattern(workspace, 7)).expect("workspace");
        let run = node
            .import_run(ImportedVm {
                cid,
                role: Some("planner".to_string()),
                vcpus: 2,
                memory_mib: 256,
                balloon_usable_mib: Some(128),
                cmdline: "console=ttyS0 plaza.vm=0".to_string(),
                sleep_dir,
                workspace: Some(image),
            })
            .expect("the VM is imported asleep");
        assert_eq!(run.id(), "r1");
        node
    }

    /// A node holding asleep VM 0 of run `r1` with no workspace volume, imported from stand-in sleep
    /// files of `memory` bytes.
    fn asleep_node_without_workspace(root: &Path, cid: u64, memory: usize) -> Registry {
        let node = node_at(root);
        let sleep_dir = root.join("sleep-in");
        fs::create_dir_all(&sleep_dir).expect("the stand-in sleep directory");
        fs::write(sleep_dir.join(MEMORY_FILE), pattern(memory, 3)).expect("memory");
        fs::write(sleep_dir.join(STATE_FILE), br#"{"meta":{"format":1}}"#).expect("state");
        node.import_run(ImportedVm {
            cid,
            role: None,
            vcpus: 1,
            memory_mib: 256,
            balloon_usable_mib: None,
            cmdline: "console=ttyS0 plaza.vm=0".to_string(),
            sleep_dir,
            workspace: None,
        })
        .expect("the VM is imported asleep");
        node
    }

    /// The export's metadata a relay sends with the import: `{"meta": ..., "files": ...}`.
    fn import_body(export: &ExportView) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"meta": export.meta, "files": export.files}))
            .expect("the import request")
    }

    /// Relays export `export` from `source` to import `import` on `target`, as the hub does: each
    /// file is read in pieces of `piece` bytes and written at the same offset. `short` names a file
    /// whose last byte is not sent, and `flip` a file whose first byte is changed in transit.
    fn relay(
        source: &Migrations,
        export: &ExportView,
        target: &Migrations,
        import: &str,
        short: Option<&str>,
        flip: Option<&str>,
    ) {
        const PIECE: u64 = 4096;
        for file in &export.files {
            let limit = if short == Some(file.name.as_str()) {
                file.len - 1
            } else {
                file.len
            };
            let mut offset = 0u64;
            while offset < limit {
                let query = format!("offset={offset}&len={PIECE}");
                let chunk = read_export(source, &export.export, &file.name, &query)
                    .expect("a chunk of the export");
                let mut bytes = base64::engine::general_purpose::STANDARD
                    .decode(&chunk.data)
                    .expect("base64");
                bytes.truncate((limit - offset) as usize);
                if flip == Some(file.name.as_str()) && offset == 0 {
                    bytes[0] ^= 0xff;
                }
                let body = serde_json::to_vec(&serde_json::json!({
                    "offset": offset,
                    "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                }))
                .expect("the chunk");
                write_import(target, import, &file.name, &body).expect("a chunk is written");
                offset += bytes.len() as u64;
            }
        }
    }

    /// Creates an import of `export` on `target`, from the export's own metadata.
    fn start_import(target: &Migrations, node: &Registry, export: &ExportView) -> String {
        create_import(target, node, &import_body(export)).expect("the import is created")
    }

    #[test]
    fn an_asleep_vm_moves_to_another_node_byte_for_byte() {
        let (src_root, dst_root) = (scratch("roundtrip-src"), scratch("roundtrip-dst"));
        let source = asleep_node(&src_root, 40, 9000, 20_000);
        let source_migrations = Migrations::new();
        let export = export_vm(&source_migrations, &source, "r1", 0).expect("an asleep VM exports");
        assert_eq!(
            export
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            [MEMORY_FILE, STATE_FILE, WORKSPACE_FILE]
        );
        assert_eq!(
            export.meta,
            VmMeta {
                cid: 40,
                role: Some("planner".to_string()),
                vcpus: 2,
                memory_mib: 256,
                balloon_usable_mib: Some(128),
                cmdline: "console=ttyS0 plaza.vm=0".to_string(),
            }
        );

        let target = node_at(&dst_root);
        let target_migrations = Migrations::new();
        let import = start_import(&target_migrations, &target, &export);
        relay(
            &source_migrations,
            &export,
            &target_migrations,
            &import,
            None,
            None,
        );
        let done =
            finish_import(&target_migrations, &target, &import).expect("the import finishes");
        assert_eq!(done.cid, 40);
        assert!(target.cid_in_use(40), "the imported CID is held here");
        assert!(
            !dst_root
                .join("snapshots")
                .join("imports")
                .join(&import)
                .exists(),
            "the staging directory is gone"
        );

        let original = source.asleep_vm("r1", 0).expect("the source VM");
        let arrived = target
            .asleep_vm(&done.run, 0)
            .expect("the imported VM is asleep");
        assert_eq!(arrived.cid, 40);
        assert_eq!(arrived.role.as_deref(), Some("planner"));
        assert_eq!(arrived.spec.cmdline, "console=ttyS0 plaza.vm=0");
        assert_eq!(arrived.spec.vcpus, 2);
        assert_eq!(arrived.spec.memory_mib, 256);
        assert_eq!(arrived.spec.balloon, Some(128));
        assert_eq!(arrived.spec.kernel, dst_root.join("vmlinux"));
        assert_eq!(arrived.spec.initrd, Some(dst_root.join("initrd.gz")));
        assert_eq!(
            arrived
                .spec
                .vsock
                .as_ref()
                .map(|vsock| vsock.uds_path.clone()),
            Some(dst_root.join("uds").join("vm40"))
        );
        assert!(arrived.spec.restore.is_none());
        assert_eq!(
            arrived.sleep_dir,
            dst_root.join("snapshots").join("cid-40").join(SLEEP)
        );
        for name in [MEMORY_FILE, STATE_FILE] {
            assert_eq!(
                fs::read(arrived.sleep_dir.join(name)).expect("the arrived file"),
                fs::read(original.sleep_dir.join(name)).expect("the original file"),
                "{name} arrives byte for byte"
            );
        }
        assert_eq!(
            fs::read(arrived.spec.workspace.as_ref().expect("a workspace")).expect("the image"),
            fs::read(original.spec.workspace.as_ref().expect("a workspace")).expect("the image"),
        );
        let _ = fs::remove_dir_all(&src_root);
        let _ = fs::remove_dir_all(&dst_root);
    }

    #[test]
    fn a_vm_with_no_workspace_volume_moves_as_its_sleep_snapshot_alone() {
        let (src_root, dst_root) = (scratch("bare-src"), scratch("bare-dst"));
        let source = asleep_node_without_workspace(&src_root, 42, 9000);
        let source_migrations = Migrations::new();
        let export = export_vm(&source_migrations, &source, "r1", 0).expect("an asleep VM exports");
        let names: Vec<&str> = export.files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(
            names,
            [MEMORY_FILE, STATE_FILE],
            "no workspace file is exported"
        );

        let target = node_at(&dst_root);
        let target_migrations = Migrations::new();
        let import = start_import(&target_migrations, &target, &export);
        relay(
            &source_migrations,
            &export,
            &target_migrations,
            &import,
            None,
            None,
        );
        let done =
            finish_import(&target_migrations, &target, &import).expect("the import finishes");
        assert_eq!(done.cid, 42);
        let arrived = target
            .asleep_vm(&done.run, 0)
            .expect("the imported VM is asleep");
        assert_eq!(
            arrived.spec.workspace, None,
            "the VM comes with no workspace"
        );
        for name in [MEMORY_FILE, STATE_FILE] {
            assert_eq!(
                fs::read(arrived.sleep_dir.join(name)).expect("the arrived file"),
                fs::read(
                    source
                        .asleep_vm("r1", 0)
                        .expect("the source VM")
                        .sleep_dir
                        .join(name)
                )
                .expect("the original file"),
                "{name} arrives byte for byte"
            );
        }
        // An import carries the two files of a VM with no workspace, or the three of one with a
        // workspace, and no other list.
        let mut with_workspace = export.files.clone();
        with_workspace.push(FileEntry {
            name: WORKSPACE_FILE.to_string(),
            len: 0,
            sha256: "0".repeat(64),
        });
        assert!(check_files(&export.files).is_ok());
        assert!(check_files(&with_workspace).is_ok());
        assert!(check_files(&export.files[..1]).is_err());
        let _ = fs::remove_dir_all(&src_root);
        let _ = fs::remove_dir_all(&dst_root);
    }

    #[test]
    fn a_chunk_read_is_capped_at_four_mebibytes_and_reports_the_end() {
        let root = scratch("reads");
        let source = asleep_node(&root, 41, 64 << 10, (5 << 20) + 5);
        let migrations = Migrations::new();
        let export = export_vm(&migrations, &source, "r1", 0).expect("an export");
        let workspace = export
            .files
            .iter()
            .find(|file| file.name == WORKSPACE_FILE)
            .expect("the workspace file");
        let first = read_export(
            &migrations,
            &export.export,
            WORKSPACE_FILE,
            "offset=0&len=99999999",
        )
        .expect("the first chunk");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&first.data)
                .expect("base64")
                .len() as u64,
            MAX_READ
        );
        assert!(!first.eof);
        let last = read_export(
            &migrations,
            &export.export,
            WORKSPACE_FILE,
            &format!("offset={MAX_READ}&len={MAX_READ}"),
        )
        .expect("the last chunk");
        assert!(last.eof, "a read to the file's end reports eof");
        let rest = read_export(
            &migrations,
            &export.export,
            WORKSPACE_FILE,
            &format!("offset={}&len=10", workspace.len),
        )
        .expect("an empty read at the end");
        assert!(rest.eof && rest.data.is_empty());
        for bad in ["offset=1", "len=0", "len=x", "offset=99999999999&len=1"] {
            let err = read_export(&migrations, &export.export, WORKSPACE_FILE, bad)
                .expect_err("a bad query is refused");
            assert_eq!(err.status(), 400, "{bad}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_sha256_or_a_short_file_fails_the_import_and_leaves_nothing() {
        let (src_root, dst_root) = (scratch("bad-src"), scratch("bad-dst"));
        let source = asleep_node(&src_root, 42, 9000, 20_000);
        let source_migrations = Migrations::new();
        let export = export_vm(&source_migrations, &source, "r1", 0).expect("an export");
        let target = node_at(&dst_root);
        let target_migrations = Migrations::new();

        let flipped = start_import(&target_migrations, &target, &export);
        relay(
            &source_migrations,
            &export,
            &target_migrations,
            &flipped,
            None,
            Some(MEMORY_FILE),
        );
        let err = finish_import(&target_migrations, &target, &flipped)
            .expect_err("a changed byte is refused");
        assert_eq!(err.status(), 422);
        assert!(err.to_string().contains(MEMORY_FILE));

        let short = start_import(&target_migrations, &target, &export);
        relay(
            &source_migrations,
            &export,
            &target_migrations,
            &short,
            Some(WORKSPACE_FILE),
            None,
        );
        let err = finish_import(&target_migrations, &target, &short)
            .expect_err("a short file is refused");
        assert_eq!(err.status(), 422);

        for import in [&flipped, &short] {
            assert!(
                !dst_root
                    .join("snapshots")
                    .join("imports")
                    .join(import)
                    .exists(),
                "{import} leaves no staging directory"
            );
        }
        assert!(!target.cid_in_use(42), "no CID is held for a failed import");
        assert!(
            !dst_root.join("snapshots").join("cid-42").exists(),
            "no sleep snapshot is kept for a failed import"
        );
        assert!(target.list().is_empty(), "no run is registered");
        let _ = fs::remove_dir_all(&src_root);
        let _ = fs::remove_dir_all(&dst_root);
    }

    #[test]
    fn an_import_refuses_a_cid_the_node_runs_and_takes_one_outside_the_run_range() {
        let root = scratch("cids");
        let node = asleep_node(&root, 40, 4096, 4096);
        let migrations = Migrations::new();
        let export = export_vm(&migrations, &node, "r1", 0).expect("an export");
        let clash = create_import(&migrations, &node, &import_body(&export))
            .expect_err("a CID the node runs is refused");
        assert_eq!(clash.status(), 409);

        let mut outside = export.clone();
        outside.meta.cid = 150;
        create_import(&migrations, &node, &import_body(&outside))
            .expect("a CID outside the run range is taken");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_export_of_a_running_vm_is_refused_and_an_unknown_run_is_a_404() {
        let root = scratch("refused");
        let node = node_at(&root);
        let run = node.stand_in_run(43, VmState::Running);
        let migrations = Migrations::new();
        let err = export_vm(&migrations, &node, run.id(), 0).expect_err("a running VM is refused");
        assert_eq!(err.status(), 409);
        let err = export_vm(&migrations, &node, "r99", 0).expect_err("an unknown run");
        assert_eq!(err.status(), 404);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_export_is_deleted_or_expires_and_a_bad_import_request_is_a_400() {
        let root = scratch("expiry");
        let node = asleep_node(&root, 44, 4096, 4096);
        let migrations = Migrations::new();
        let export = export_vm(&migrations, &node, "r1", 0).expect("an export");
        let query = "offset=0&len=16";
        read_export(&migrations, &export.export, MEMORY_FILE, query).expect("a live export reads");
        delete_export(&migrations, &export.export).expect("the export is deleted");
        let gone = read_export(&migrations, &export.export, MEMORY_FILE, query)
            .expect_err("a deleted export is gone");
        assert_eq!(gone.status(), 404);

        let again = export_vm(&migrations, &node, "r1", 0).expect("a second export");
        let later = Instant::now() + TTL + Duration::from_secs(1);
        drop(migrations.lock_at(later));
        let expired = read_export(&migrations, &again.export, MEMORY_FILE, query)
            .expect_err("an expired export is gone");
        assert_eq!(expired.status(), 404);

        let bad_files = serde_json::json!({
            "meta": export.meta,
            "files": [{"name": MEMORY_FILE, "len": 1, "sha256": "00"}],
        });
        let err = create_import(
            &migrations,
            &node,
            &serde_json::to_vec(&bad_files).expect("the body"),
        )
        .expect_err("a file list that is not the VM's three files is refused");
        assert_eq!(err.status(), 400);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_declared_size_over_its_maximum_is_refused() {
        let mib = 1u64 << 20;
        assert!(allowed(STATE_FILE, 16 * mib, 256).is_ok());
        assert!(allowed(STATE_FILE, 16 * mib + 1, 256).is_err());
        assert!(allowed(MEMORY_FILE, 256 * mib + mib, 256).is_ok());
        assert!(allowed(MEMORY_FILE, 256 * mib + mib + 1, 256).is_err());
        assert!(allowed(WORKSPACE_FILE, MAX_WORKSPACE_FILE, 256).is_ok());
        assert!(allowed(WORKSPACE_FILE, MAX_WORKSPACE_FILE + 1, 256).is_err());
        assert!(allowed("other.bin", 0, 256).is_err(), "an unknown file");
    }

    #[test]
    fn an_import_declaring_an_oversized_file_is_refused_before_anything_is_staged() {
        let (src_root, dst_root) = (scratch("cap-src"), scratch("cap-dst"));
        let source = asleep_node(&src_root, 45, 4096, 4096);
        let export = export_vm(&Migrations::new(), &source, "r1", 0).expect("an export");
        let target = node_at(&dst_root);
        let migrations = Migrations::new();
        let mut oversized = export.clone();
        oversized
            .files
            .iter_mut()
            .find(|file| file.name == MEMORY_FILE)
            .expect("the memory file")
            .len = (256 << 20) + (2 << 20);
        let err = create_import(&migrations, &target, &import_body(&oversized))
            .expect_err("a memory file over the VM's memory is refused");
        assert_eq!(err.status(), 400);
        assert!(err.to_string().contains(MEMORY_FILE));
        let _ = fs::remove_dir_all(&src_root);
        let _ = fs::remove_dir_all(&dst_root);
    }

    #[test]
    fn at_most_four_imports_are_in_flight_and_an_aborted_one_frees_its_place() {
        let (src_root, dst_root) = (scratch("slots-src"), scratch("slots-dst"));
        let source = asleep_node(&src_root, 46, 4096, 4096);
        let export = export_vm(&Migrations::new(), &source, "r1", 0).expect("an export");
        let target = node_at(&dst_root);
        let migrations = Migrations::new();
        let ids: Vec<String> = (0..MAX_IMPORTS)
            .map(|_| start_import(&migrations, &target, &export))
            .collect();
        let full = create_import(&migrations, &target, &import_body(&export))
            .expect_err("a fifth import is refused while four are in flight");
        assert_eq!(full.status(), 503);
        abort_import(&migrations, &ids[0]).expect("an import is aborted");
        let _ = start_import(&migrations, &target, &export);
        let _ = fs::remove_dir_all(&src_root);
        let _ = fs::remove_dir_all(&dst_root);
    }
}
