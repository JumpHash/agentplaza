//! Hugging Face imports and their routes (`docs/design/foundry.md`, "Hugging Face"): the import list
//! (`<state>/foundry/hf/imports.json`), the queue (one download at a time), resumable downloads with
//! their sha256 checks, the registration, and the token (a vault entry, `HF_TOKEN`). The HTTP client is `hf.rs`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::hf::{self, Fail, Search, TreeFile};
use super::registry::{self, ModelRecord, Source, State};
use crate::state::App;
use crate::vault::{Consumer, OnBehalf, Store as Vault};
use secrecy::{ExposeSecret, SecretString};

/// The vault entry of the owner's Hugging Face token, and the consumer that reads it.
const HF_NAME: &str = "HF_TOKEN";
const HF_BINDING: &str = "hf";

const CHUNK: usize = 64 << 10;
const PROGRESS_BYTES: u64 = 512 << 10;
const PROGRESS_EVERY: Duration = Duration::from_millis(500);
const GIB: u64 = 1 << 30;
/// Free disk an import leaves behind.
const RESERVE_BYTES: u64 = GIB;
/// The states of an import that has a thread (or waits for one).
const ACTIVE: [&str; 4] = ["queued", "downloading", "verifying", "registering"];

/// One file of an import: its size and LFS sha256 from the tree, and how far it has got.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRec {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
    pub bytes: u64,
    pub state: String,
}

/// One import, as the routes show it (see `docs/design/foundry.md`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Import {
    pub n: u64,
    pub id: String,
    pub revision: String,
    pub sha: String,
    #[serde(rename = "as")]
    pub as_: String,
    pub state: String,
    pub dir: String,
    pub files: Vec<FileRec>,
    pub bytes: u64,
    pub total: u64,
    pub model_id: Option<String>,
    pub servable: Option<bool>,
    pub reason: Option<String>,
    pub error: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    next: u64,
    imports: Vec<Import>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    imports: Vec<Import>,
    /// The cancel flag of each import with a thread.
    cancel: BTreeMap<u64, Arc<AtomicBool>>,
}

/// The import list, the queue and the token. Lives on `App`.
pub struct Imports {
    dir: PathBuf,
    inner: Mutex<Inner>,
    /// Held by the one import that downloads.
    slot: Mutex<()>,
}

/// What a download step ended with.
#[derive(Debug, PartialEq)]
enum Step {
    Finished,
    Cancelled,
}

/// What an import thread needs, owned.
struct Job {
    n: u64,
    id: String,
    sha: String,
    task: Option<String>,
    dir: PathBuf,
    token: Option<String>,
    block: Vec<String>,
    flag: Arc<AtomicBool>,
    weak: Weak<App>,
}

impl Imports {
    /// Opens `<state>/foundry/hf`. Imports that were running when apid stopped are `interrupted`.
    pub fn open(state_dir: &Path) -> Imports {
        let dir = state_dir.join("foundry").join("hf");
        let mut saved: Saved = fs::read(dir.join("imports.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let now = apz_plaza::now_ms();
        for import in &mut saved.imports {
            if ACTIVE.contains(&import.state.as_str()) {
                import.state = "interrupted".into();
                import.updated_ms = now;
            }
        }
        let inner = Inner {
            next: saved.next.max(1),
            imports: saved.imports,
            cancel: BTreeMap::new(),
        };
        Imports {
            dir,
            inner: Mutex::new(inner),
            slot: Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn save(&self, inner: &Inner) -> Result<(), String> {
        let saved = Saved {
            next: inner.next,
            imports: inner.imports.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&saved).map_err(|e| e.to_string())?;
        fs::create_dir_all(&self.dir)
            .map_err(|e| format!("creating {}: {e}", self.dir.display()))?;
        crate::providers::write_atomic(&self.dir.join("imports.json"), &bytes, 0o600)
            .map_err(|e| format!("saving the imports: {e}"))
    }

    /// Applies `change` to import `n` (none when it is gone) and saves when `persist`.
    fn update<T>(&self, n: u64, persist: bool, change: impl FnOnce(&mut Import) -> T) -> Option<T> {
        let mut inner = self.lock();
        let import = inner.imports.iter_mut().find(|i| i.n == n)?;
        let value = change(import);
        import.updated_ms = apz_plaza::now_ms();
        if persist {
            let _ = self.save(&inner);
        }
        Some(value)
    }

    /// Sets an import's state, unless it was cancelled (a cancelled import stays cancelled).
    fn set_state(&self, n: u64, state: &str) -> bool {
        self.update(n, true, |import| {
            if import.state == "cancelled" {
                return false;
            }
            import.state = state.to_string();
            true
        })
        .unwrap_or(false)
    }

    pub fn list(&self) -> Vec<Import> {
        let mut imports = self.lock().imports.clone();
        imports.sort_by(|a, b| b.n.cmp(&a.n));
        imports
    }

    pub fn get(&self, n: u64) -> Option<Import> {
        self.lock().imports.iter().find(|i| i.n == n).cloned()
    }

    /// The owner's Hugging Face token: the vault entry `HF_TOKEN` that `hf` reads. Never leaves the hub. A read
    /// that fails (the vault sealed, or no entry) is no token.
    pub fn token(&self, vault: &Vault) -> Option<String> {
        let consumer = Consumer {
            binding: HF_BINDING.into(),
            on_behalf: OnBehalf::Hub,
        };
        vault
            .resolve(HF_NAME, &consumer, apz_plaza::now_ms())
            .ok()
            .map(|t| t.expose_secret().trim().to_string())
            .filter(|t| !t.is_empty())
    }

    /// Stores the token as the plaza entry `HF_TOKEN` (a new version of it when there is one). One line, no spaces.
    pub fn set_token(&self, vault: &Vault, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > 512
            || value
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err("the token must be one line with no spaces".into());
        }
        vault
            .put_plaza(
                HF_NAME,
                HF_BINDING,
                "Hugging Face token",
                &SecretString::from(value.to_string()),
                "owner",
                apz_plaza::now_ms(),
            )
            .map(|_| ())
            .map_err(|e| e.message())
    }

    /// Removes the token: the entry goes when `hf` was its only reader, else only the binding.
    pub fn clear_token(&self, vault: &Vault) -> Result<(), String> {
        let now = apz_plaza::now_ms();
        let named = vault.entries_named(HF_NAME).map_err(|e| e.message())?;
        for entry in named
            .iter()
            .filter(|e| e.bindings.iter().any(|b| b == HF_BINDING))
        {
            if entry.bindings.len() == 1 {
                vault
                    .delete(&entry.id, "owner", now)
                    .map_err(|e| e.message())?;
            } else {
                let rest: Vec<String> = entry
                    .bindings
                    .iter()
                    .filter(|b| *b != HF_BINDING)
                    .cloned()
                    .collect();
                vault
                    .patch(&entry.id, None, None, Some(rest), "owner", now)
                    .map_err(|e| e.message())?;
            }
        }
        Ok(())
    }

    /// The `/test` check: the account Hugging Face names for the stored token. The detail is the name, never the token.
    pub fn check_token(&self, vault: &Vault, block: &[String]) -> Result<String, String> {
        let token = self
            .token(vault)
            .ok_or_else(|| "no token is stored".to_string())?;
        let url = format!("{}/api/whoami-v2", hf::ORIGIN);
        let (value, _) = hf::get_json(&url, Some(&token), block, "the token").map_err(|f| {
            if f.status == 403 {
                "Hugging Face refused the token".to_string()
            } else {
                f.message
            }
        })?;
        signed_in(&value)
    }

    /// Starts an import: resolves the revision and its tree, checks the size, the disk and the duplicates,
    /// records the import as queued and starts its thread. The network part runs in the request.
    pub fn start(&self, app: &App, body: ImportBody) -> Result<Started, Fail> {
        let id = body.id.trim().to_string();
        if !hf::valid_repo_id(&id) {
            return Err(Fail::new(
                400,
                format!("{id:?} is not a Hugging Face repo id (name or org/name)"),
            ));
        }
        let revision = body
            .revision
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "main".into());
        if !hf::valid_revision(&revision) {
            return Err(Fail::new(400, format!("{revision:?} is not a revision")));
        }
        let kind = body.as_;
        if kind != "base" && kind != "servable" {
            return Err(Fail::new(
                400,
                format!("as must be base or servable, got {kind:?}"),
            ));
        }
        let requested = body.files.unwrap_or_default();
        if let Some(bad) = requested.iter().find(|p| !hf::safe_path(p)) {
            return Err(Fail::new(
                400,
                format!("{bad:?} is not a relative file path"),
            ));
        }
        let settings = app.settings.get();
        let block = settings.fetch_block;
        let max_gb = settings.foundry.hf_max_gb;
        let token = self.token(&app.vault);
        let (sha, task) = hf::resolve_revision(&id, &revision, token.as_deref(), &block)?;
        let files = hf::tree(&id, &sha, token.as_deref(), &block)?;
        let picked = pick(&id, &kind, &requested, &files)?;
        let dir = self.dir.join(hf::repo_dir(&id, &sha));
        // A repo and revision whose files are all on disk, from a finished import, is already imported: nothing
        // is queued. Otherwise only the files not on disk are downloaded (a finished file is never downloaded again).
        let missing = missing_files(&dir, &picked);
        if missing.is_empty() {
            let done = self
                .lock()
                .imports
                .iter()
                .filter(|i| i.id == id && i.sha == sha && i.state == "done")
                .max_by_key(|i| i.n)
                .cloned();
            if let Some(existing) = done {
                return Ok(Started::Already(existing));
            }
        }
        let to_download = if missing.is_empty() {
            picked.clone()
        } else {
            missing
        };
        let total: u64 = to_download.iter().map(|f| f.size).sum();
        check_cap(total, max_gb)?;

        let mut inner = self.lock();
        if inner
            .imports
            .iter()
            .any(|i| i.id == id && i.sha == sha && ACTIVE.contains(&i.state.as_str()))
        {
            return Err(Fail::new(
                409,
                format!("{id} at {sha} is already importing"),
            ));
        }
        // An interrupted import of the same commit is replaced; its partial files are reused.
        inner
            .imports
            .retain(|i| !(i.id == id && i.sha == sha && i.state == "interrupted"));
        let have: u64 = to_download
            .iter()
            .map(|f| already_have(&dir, &f.path, f.size))
            .sum();
        let remaining = total.saturating_sub(have);
        let free = free_bytes(&self.dir)
            .ok_or_else(|| Fail::new(507, "cannot read the free disk space"))?;
        if free < remaining.saturating_add(RESERVE_BYTES) {
            return Err(Fail::new(
                507,
                format!(
                    "the import needs {} GiB and {} GiB are free",
                    (remaining + RESERVE_BYTES) / GIB,
                    free / GIB
                ),
            ));
        }
        let now = apz_plaza::now_ms();
        let n = inner.next;
        inner.next += 1;
        let import = Import {
            n,
            id: id.clone(),
            revision,
            sha: sha.clone(),
            as_: kind,
            state: "queued".into(),
            dir: dir.display().to_string(),
            files: to_download
                .iter()
                .map(|f| FileRec {
                    path: f.path.clone(),
                    size: f.size,
                    sha256: f.sha256.clone(),
                    bytes: 0,
                    state: "pending".into(),
                })
                .collect(),
            bytes: 0,
            total,
            model_id: None,
            servable: None,
            reason: None,
            error: None,
            created_ms: now,
            updated_ms: now,
        };
        let flag = Arc::new(AtomicBool::new(false));
        inner.imports.push(import.clone());
        inner.cancel.insert(n, flag.clone());
        self.save(&inner).map_err(|e| Fail::new(500, e))?;
        drop(inner);
        frame(app, n, &id, "", 0, total, "queued");
        let job = Job {
            n,
            id,
            sha,
            task,
            dir,
            token,
            block,
            flag,
            weak: app.handle(),
        };
        std::thread::Builder::new()
            .name(format!("hf-import-{n}"))
            .spawn(move || run(job))
            .map_err(|e| Fail::new(500, format!("starting the import: {e}")))?;
        Ok(Started::New(import))
    }

    /// `DELETE /imports/{n}`: a queued or running import is cancelled (its partial files stay, so a new
    /// import resumes); a finished, failed, cancelled or interrupted one leaves the list and its partial
    /// files are deleted. Downloaded files stay: the registry may point at them.
    pub fn remove(&self, app: &App, n: u64) -> Result<Value, Fail> {
        let mut inner = self.lock();
        let index = inner
            .imports
            .iter()
            .position(|i| i.n == n)
            .ok_or_else(|| Fail::new(404, "no such import"))?;
        let state = inner.imports[index].state.clone();
        if state == "registering" {
            return Err(Fail::new(
                409,
                "the import is registering its model: wait for it to finish",
            ));
        }
        if matches!(state.as_str(), "queued" | "downloading" | "verifying") {
            if let Some(flag) = inner.cancel.get(&n) {
                flag.store(true, Ordering::SeqCst);
            }
            let import = &mut inner.imports[index];
            import.state = "cancelled".into();
            import.updated_ms = apz_plaza::now_ms();
            let shown = import.clone();
            let _ = self.save(&inner);
            frame(app, n, &shown.id, "", shown.bytes, shown.total, "cancelled");
            return Ok(json!({ "import": shown }));
        }
        let import = inner.imports.remove(index);
        inner.cancel.remove(&n);
        let keep: Vec<String> = inner
            .imports
            .iter()
            .filter(|i| i.state == "done" && i.id == import.id && i.sha == import.sha)
            .flat_map(|i| {
                i.files
                    .iter()
                    .filter(|f| f.state == "done")
                    .map(|f| f.path.clone())
            })
            .collect();
        clear_partials(Path::new(&import.dir), &import.files, &keep);
        let _ = self.save(&inner);
        Ok(json!({ "removed": n }))
    }
}

/// The request body of `POST /v1/foundry/hf/import`.
#[derive(Debug, Deserialize)]
pub struct ImportBody {
    pub id: String,
    pub revision: Option<String>,
    pub files: Option<Vec<String>>,
    #[serde(rename = "as")]
    pub as_: String,
}

/// The files an import takes: the named ones (each must be in the tree), or the defaults for its kind.
fn pick(
    id: &str,
    kind: &str,
    requested: &[String],
    files: &[TreeFile],
) -> Result<Vec<TreeFile>, Fail> {
    if requested.is_empty() {
        let names = hf::default_files(kind, files).map_err(|e| Fail::new(400, e))?;
        return names
            .iter()
            .map(|name| {
                files
                    .iter()
                    .find(|f| &f.path == name)
                    .cloned()
                    .ok_or_else(|| Fail::new(502, "a default file vanished"))
            })
            .collect();
    }
    let mut picked: Vec<TreeFile> = Vec::new();
    for path in requested {
        let file = files
            .iter()
            .find(|f| &f.path == path)
            .ok_or_else(|| Fail::new(400, format!("{path} is not in {id}")))?;
        if !picked.iter().any(|p| p.path == file.path) {
            picked.push(file.clone());
        }
    }
    Ok(picked)
}

/// Refuses a selection over `foundry.hf_max_gb` (GiB).
pub fn check_cap(total: u64, max_gb: u32) -> Result<(), Fail> {
    if total > u64::from(max_gb) * GIB {
        return Err(Fail::new(
            413,
            format!(
                "the files total {:.2} GiB, over foundry.hf_max_gb ({max_gb})",
                total as f64 / GIB as f64
            ),
        ));
    }
    Ok(())
}

/// The partial file of a download.
fn part_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// What `start` did: queued a new import, or found the repo and revision already imported.
#[derive(Debug)]
pub enum Started {
    New(Import),
    Already(Import),
}

/// The files of a selection that are not on disk at the size the repo lists. A file at that size is finished.
fn missing_files(dir: &Path, picked: &[TreeFile]) -> Vec<TreeFile> {
    picked
        .iter()
        .filter(|f| dir.join(&f.path).metadata().map(|m| m.len()).ok() != Some(f.size))
        .cloned()
        .collect()
}

/// After a failed check: a partial file is deleted; a finished file (which another import may hold) never is.
fn discard_failed(checked: &Path, part: &Path) {
    if checked == part {
        let _ = fs::remove_file(part);
    }
}

/// Deletes the partial files of a cancelled or removed import. Only `.part` files are deleted, and a path in
/// `keep` (one a finished import holds) is never touched.
pub fn clear_partials(dir: &Path, files: &[FileRec], keep: &[String]) {
    for file in files {
        if keep.contains(&file.path) {
            continue;
        }
        let _ = fs::remove_file(part_path(&dir.join(&file.path)));
    }
}

/// Bytes of a file already on disk from an earlier run: the whole file, or its partial prefix.
fn already_have(dir: &Path, path: &str, size: u64) -> u64 {
    let target = dir.join(path);
    if target.metadata().map(|m| m.len()).ok() == Some(size) {
        return size;
    }
    part_path(&target)
        .metadata()
        .map(|m| m.len().min(size))
        .unwrap_or(0)
}

/// Free bytes on the disk holding `dir`: the disk whose mount point is the longest prefix of the directory's
/// path. Cross-platform through `sysinfo`; `None` when no disk matches.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    fs::create_dir_all(dir).ok()?;
    let canonical = fs::canonicalize(dir).ok()?;
    // Windows canonical paths carry the verbatim prefix `\\?\`, which mount points do not.
    let text = canonical.to_string_lossy();
    let target = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|disk| target.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
}

fn frame(app: &App, n: u64, id: &str, file: &str, bytes: u64, total: u64, state: &str) {
    app.bus.publish(
        "foundry",
        &json!({ "kind": "hf_import", "import": n, "id": id, "file": file, "bytes": bytes, "total": total, "state": state }),
    );
}

/// The import's thread: takes the one download slot, downloads and checks each file, registers the model.
fn run(job: Job) {
    let Some(app) = job.weak.upgrade() else {
        return;
    };
    let imports = &app.hf;
    let _slot = imports.slot.lock().unwrap_or_else(PoisonError::into_inner);
    if job.flag.load(Ordering::SeqCst) {
        imports.lock().cancel.remove(&job.n);
        return;
    }
    imports.set_state(job.n, "downloading");
    let outcome = download_all(&app, &job);
    match outcome {
        Ok(Step::Cancelled) => {}
        Ok(Step::Finished) => register(&app, &job),
        Err(message) => fail(&app, &job, &message),
    }
    imports.lock().cancel.remove(&job.n);
}

fn download_all(app: &App, job: &Job) -> Result<Step, String> {
    let files = app.hf.get(job.n).map(|i| i.files).unwrap_or_default();
    for (index, file) in files.iter().enumerate() {
        if job.flag.load(Ordering::SeqCst) {
            return Ok(Step::Cancelled);
        }
        if fetch_file(app, job, index, file)? == Step::Cancelled {
            return Ok(Step::Cancelled);
        }
    }
    Ok(Step::Finished)
}

/// One file: complete it (download from its partial prefix), check its size and sha256, move it into place.
fn fetch_file(app: &App, job: &Job, index: usize, file: &FileRec) -> Result<Step, String> {
    let target = job.dir.join(&file.path);
    let part = part_path(&target);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    // Complete from an earlier run: check the file as it is (hashing it). Otherwise download the rest
    // of the partial file, whose digest the transfer computed.
    let (checked_path, digest) = if target.metadata().map(|m| m.len()).ok() == Some(file.size) {
        (target.clone(), None)
    } else {
        let mut offset = part.metadata().map(|m| m.len()).unwrap_or(0);
        if offset > file.size {
            let _ = fs::remove_file(&part);
            offset = 0;
        }
        match download(app, job, index, file, &part, offset)? {
            Download::Cancelled => return Ok(Step::Cancelled),
            Download::Done(digest) => (part.clone(), Some(digest)),
        }
    };
    app.hf
        .update(job.n, false, |i| i.files[index].state = "verifying".into());
    if let Err(message) = check(&checked_path, file, digest.as_deref()) {
        discard_failed(&checked_path, &part);
        return Err(message);
    }
    if checked_path == part {
        fs::rename(&part, &target).map_err(|e| format!("moving {} into place: {e}", file.path))?;
    }
    app.hf.update(job.n, true, |i| {
        i.files[index].state = "done".into();
        i.files[index].bytes = file.size;
    });
    frame(app, job.n, &job.id, &file.path, file.size, 0, "done");
    Ok(Step::Finished)
}

/// What a download ended with: cancelled, or the whole file's sha256 (hex) once it is on disk.
#[derive(Debug, PartialEq)]
enum Download {
    Cancelled,
    Done(String),
}

/// Downloads the rest of a file into `part`, from `offset` on (a `Range` request when `offset > 0`).
fn download(
    app: &App,
    job: &Job,
    index: usize,
    file: &FileRec,
    part: &Path,
    mut offset: u64,
) -> Result<Download, String> {
    let open = |from: u64| {
        hf::open_file(
            &job.id,
            &job.sha,
            &file.path,
            from,
            job.token.as_deref(),
            &job.block,
        )
    };
    let mut stream = match open(offset) {
        Ok(stream) => stream,
        Err(f) if f.status == 416 => {
            let _ = fs::remove_file(part);
            offset = 0;
            open(0).map_err(describe)?
        }
        Err(f) => return Err(describe(f)),
    };
    if stream.status == 200 {
        // The server sent the whole file, not the range: start the partial file over.
        offset = 0;
    }
    app.hf.update(job.n, false, |i| {
        i.files[index].state = "downloading".into();
        i.files[index].bytes = offset;
    });
    let mut out = if offset == 0 {
        File::create(part)
    } else {
        OpenOptions::new().append(true).open(part)
    }
    .map_err(|e| format!("writing {}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    if offset > 0 {
        hash_prefix(part, offset, &mut hasher)?;
    }
    let mut progress = Progress {
        app,
        job,
        index,
        file: &file.path,
        total_file: file.size,
        last: (Instant::now(), offset),
    };
    let step = transfer(
        &mut stream.reader,
        &mut out,
        &mut hasher,
        offset,
        file.size,
        &job.flag,
        &mut |bytes| progress.report(bytes),
    )?;
    out.flush()
        .map_err(|e| format!("writing {}: {e}", part.display()))?;
    if step == Step::Cancelled {
        return Ok(Download::Cancelled);
    }
    let written = part.metadata().map(|m| m.len()).unwrap_or(0);
    if written < file.size {
        return Err(format!(
            "the download of {} ended at {written} of {} bytes; import again to resume",
            file.path, file.size
        ));
    }
    Ok(Download::Done(hex(&hasher.finalize())))
}

/// Reports a file's bytes, at most every 512 KiB or 500 ms.
struct Progress<'a> {
    app: &'a App,
    job: &'a Job,
    index: usize,
    file: &'a str,
    total_file: u64,
    last: (Instant, u64),
}

impl Progress<'_> {
    fn report(&mut self, bytes: u64) {
        if bytes - self.last.1 < PROGRESS_BYTES
            && self.last.0.elapsed() < PROGRESS_EVERY
            && bytes < self.total_file
        {
            return;
        }
        self.last = (Instant::now(), bytes);
        let index = self.index;
        let Some((all, total)) = self.app.hf.update(self.job.n, false, |i| {
            i.files[index].bytes = bytes;
            i.bytes = i.files.iter().map(|f| f.bytes).sum();
            (i.bytes, i.total)
        }) else {
            return;
        };
        frame(
            self.app,
            self.job.n,
            &self.job.id,
            self.file,
            all,
            total,
            "downloading",
        );
    }
}

/// The transfer loop: copies `reader` into `out` from `written` until `size`, hashing as it goes.
/// A cancel flag stops it; more bytes than `size` is an error.
fn transfer(
    reader: &mut dyn Read,
    out: &mut dyn Write,
    hasher: &mut Sha256,
    mut written: u64,
    size: u64,
    flag: &AtomicBool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<Step, String> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        if flag.load(Ordering::SeqCst) {
            return Ok(Step::Cancelled);
        }
        let read = reader
            .read(&mut buf)
            .map_err(|e| format!("reading the download: {e}"))?;
        if read == 0 {
            return Ok(Step::Finished);
        }
        if written + read as u64 > size {
            return Err(format!(
                "the download holds more bytes than the repo lists ({size})"
            ));
        }
        out.write_all(&buf[..read])
            .map_err(|e| format!("writing the download: {e}"))?;
        hasher.update(&buf[..read]);
        written += read as u64;
        on_bytes(written);
    }
}

/// Feeds the first `len` bytes of `path` into `hasher` (the prefix a resumed download already holds).
fn hash_prefix(path: &Path, len: u64, hasher: &mut Sha256) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut limited = (&mut file).take(len);
    let mut buf = vec![0u8; CHUNK];
    loop {
        let read = limited
            .read(&mut buf)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buf[..read]);
    }
}

/// The size and, for LFS files, the sha256 of a finished file. A mismatch is an error (the caller deletes the file).
fn check(path: &Path, file: &FileRec, digest: Option<&str>) -> Result<(), String> {
    let size = path
        .metadata()
        .map(|m| m.len())
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    if size != file.size {
        return Err(format!(
            "{}: {size} bytes, the repo lists {}",
            file.path, file.size
        ));
    }
    if let Some(expect) = &file.sha256 {
        let actual = match digest {
            Some(digest) => digest.to_string(),
            None => hash_file(path)?,
        };
        if &actual != expect {
            return Err(format!(
                "sha256 mismatch for {}: the repo lists {expect}, the bytes are {actual}; the file is deleted",
                file.path
            ));
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        if read == 0 {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(&buf[..read]);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn describe(fail: Fail) -> String {
    match fail.url {
        Some(url) => format!("{} ({url})", fail.message),
        None => fail.message,
    }
}

/// The registry's id for an import: `hf-<repo>-<sha8>`, the repo's characters kept when they are safe.
fn model_name(id: &str, sha: &str) -> String {
    let clean: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("hf-{clean}-{}", &sha[..sha.len().min(8)])
}

fn register(app: &App, job: &Job) {
    if !app.hf.set_state(job.n, "registering") {
        return;
    }
    let Some(import) = app.hf.get(job.n) else {
        return;
    };
    let source = Source {
        hf: job.id.clone(),
        revision: job.sha.clone(),
        as_: import.as_.clone(),
        path: job.dir.display().to_string(),
        servable_reason: None,
        task: job.task.clone(),
    };
    match app.models.register_hf(
        &job.dir,
        source,
        &model_name(&job.id, &job.sha),
        import.total,
    ) {
        Ok(record) => {
            let servable = matches!(
                record.state,
                State::Trained | State::Shadow | State::Promoted
            );
            let reason = record
                .source
                .as_ref()
                .and_then(|s| s.servable_reason.clone());
            let model_id = record.card.id.clone();
            app.hf.update(job.n, true, |i| {
                i.state = "done".into();
                i.model_id = Some(model_id.clone());
                i.servable = Some(servable);
                i.reason = reason;
            });
            frame(app, job.n, &job.id, "", import.total, import.total, "done");
        }
        Err(message) => fail(app, job, &format!("registering the model: {message}")),
    }
}

fn fail(app: &App, job: &Job, message: &str) {
    let finished = app.hf.update(job.n, true, |i| {
        if i.state == "cancelled" {
            return None;
        }
        i.state = "failed".into();
        i.error = Some(message.to_string());
        for file in i.files.iter_mut().filter(|f| f.state != "done") {
            file.state = "failed".into();
        }
        Some((i.id.clone(), i.bytes, i.total))
    });
    if let Some(Some((id, bytes, total))) = finished {
        frame(app, job.n, &id, "", bytes, total, "failed");
    }
}

// ---- routes -----------------------------------------------------------------------------------

/// The Hugging Face routes (owner only: the gate in front of `/v1` admits only the owner).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/foundry/hf/status", status)
        .route("PUT", "/v1/foundry/hf/token", put_token)
        .delete("/v1/foundry/hf/token", delete_token)
        .get("/v1/foundry/hf/models", search_route)
        .get("/v1/foundry/hf/models/{id..}", detail_route)
        .post("/v1/foundry/hf/import", import_route)
        .post("/v1/foundry/models/{id}/rederive", rederive_route)
        .get("/v1/foundry/hf/imports", list_route)
        .get("/v1/foundry/hf/imports/{n}", one_route)
        .delete("/v1/foundry/hf/imports/{n}", delete_route)
}

fn fail_response(fail: Fail) -> Response {
    match fail.url {
        Some(url) => Response::json(fail.status, &json!({ "error": fail.message, "url": url })),
        None => Response::error(fail.status, &fail.message),
    }
}

fn status(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    let dir = app.hf.dir();
    Response::json(
        200,
        &json!({
            "hf_token_set": app.vault.presence(HF_NAME, HF_BINDING) == "secret",
            "hf_max_gb": app.settings.get().foundry.hf_max_gb,
            "free_bytes": free_bytes(dir),
            "downloads_dir": dir.display().to_string(),
        }),
    )
}

#[derive(Deserialize)]
struct TokenBody {
    value: String,
}

fn put_token(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: TokenBody = match crate::agents::body(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    match app.hf.set_token(&app.vault, &body.value) {
        Ok(()) => Response::json(200, &json!({ "hf_token_set": true })),
        Err(message) => Response::error(400, &message),
    }
}

fn delete_token(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    match app.hf.clear_token(&app.vault) {
        Ok(()) => Response::json(200, &json!({ "hf_token_set": false })),
        Err(message) => Response::error(500, &message),
    }
}

fn search_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let text = |name: &str| {
        req.query_param(name)
            .map(hf::query_text)
            .unwrap_or_default()
    };
    let limit = match hf::page_limit(req.query_param("limit")) {
        Ok(limit) => limit,
        Err(message) => return Response::error(400, &message),
    };
    let query = Search {
        search: text("search"),
        task: text("task"),
        library: text("library"),
        sort: text("sort"),
        limit,
        cursor: Some(text("cursor")).filter(|c| !c.is_empty()),
    };
    let block = app.settings.get().fetch_block;
    match hf::search_page(&query, &block) {
        Ok(page) => Response::json(200, &page),
        Err(fail) => fail_response(fail),
    }
}

fn detail_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id").to_string();
    if !hf::valid_repo_id(&id) {
        return Response::error(
            400,
            &format!("{id:?} is not a Hugging Face repo id (name or org/name)"),
        );
    }
    let revision = req
        .query_param("revision")
        .map(hf::query_text)
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "main".into());
    if !hf::valid_revision(&revision) {
        return Response::error(400, &format!("{revision:?} is not a revision"));
    }
    let token = app.hf.token(&app.vault);
    let block = app.settings.get().fetch_block;
    match hf::detail(&id, &revision, token.as_deref(), &block) {
        Ok(card) => Response::json(200, &card),
        Err(fail) => fail_response(fail),
    }
}

fn import_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: ImportBody = match crate::agents::body(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    match app.hf.start(app, body) {
        Ok(Started::New(import)) => Response::json(202, &json!({ "import": import })),
        Ok(Started::Already(import)) => {
            let model = import
                .model_id
                .as_deref()
                .and_then(|id| app.models.get(id))
                .map(|record| registry::view(&record));
            Response::json(
                200,
                &json!({ "import": import, "already": true, "model": model }),
            )
        }
        Err(fail) => fail_response(fail),
    }
}

fn list_route(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    Response::json(200, &json!({ "imports": app.hf.list() }))
}

fn rederive_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    match rederive(app, p.get("id")) {
        Ok(record) => Response::json(
            200,
            &json!({ "model": registry::view(&record), "state": record.state.name() }),
        ),
        Err(fail) => fail_response(fail),
    }
}

/// Re-registers an `imported` model from its download. An import from before the text runtime recorded no
/// task, so the repo's task is looked up first; the lookup is the only network call.
pub fn rederive(app: &App, id: &str) -> Result<ModelRecord, Fail> {
    let record = app
        .models
        .get(id)
        .ok_or_else(|| Fail::new(404, "no such model"))?;
    if record.state != State::Imported {
        return Err(Fail::new(
            409,
            format!(
                "{id} is {}: only imported models are re-derived",
                record.state.name()
            ),
        ));
    }
    let source = record
        .source
        .clone()
        .ok_or_else(|| Fail::new(409, "only a Hugging Face import can be re-derived"))?;
    let task = match source.task.clone() {
        Some(task) => Some(task),
        None => {
            let token = app.hf.token(&app.vault);
            let block = app.settings.get().fetch_block;
            hf::resolve_revision(&source.hf, &source.revision, token.as_deref(), &block)?.1
        }
    };
    app.models.rederive(id, task).map_err(|e| Fail::new(409, e))
}

fn number(p: &Params<'_>) -> Option<u64> {
    p.get("n").parse().ok()
}

fn one_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let Some(n) = number(p) else {
        return Response::error(400, "the import number must be a whole number");
    };
    match app.hf.get(n) {
        Some(import) => Response::json(200, &json!({ "import": import })),
        None => Response::error(404, "no such import"),
    }
}

fn delete_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let Some(n) = number(p) else {
        return Response::error(400, "the import number must be a whole number");
    };
    match app.hf.remove(app, n) {
        Ok(value) => Response::json(200, &value),
        Err(fail) => fail_response(fail),
    }
}

/// The account name of a whoami reply, as the detail of a check.
fn signed_in(value: &Value) -> Result<String, String> {
    value
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(|name| format!("signed in as {name}"))
        .ok_or_else(|| "Hugging Face sent no account name".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apid-hf-import-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(path: &str, size: u64, sha256: Option<String>) -> FileRec {
        FileRec {
            path: path.into(),
            size,
            sha256,
            bytes: 0,
            state: "pending".into(),
        }
    }

    fn sha_hex(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    #[test]
    fn a_sha256_mismatch_is_refused_and_a_match_passes() {
        let dir = scratch("sha");
        let body = b"weights".to_vec();
        let path = dir.join("model.onnx");
        fs::write(&path, &body).unwrap();
        let good = record("model.onnx", body.len() as u64, Some(sha_hex(&body)));
        assert!(check(&path, &good, None).is_ok());
        let bad = record("model.onnx", body.len() as u64, Some("0".repeat(64)));
        let message = check(&path, &bad, None).unwrap_err();
        assert!(message.contains("sha256 mismatch"), "{message}");
        let wrong_size = record("model.onnx", 3, None);
        assert!(
            check(&path, &wrong_size, None)
                .unwrap_err()
                .contains("bytes")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_size_cap_refuses_over_foundry_hf_max_gb_and_allows_the_cap() {
        assert!(check_cap(20 * GIB, 20).is_ok());
        assert_eq!(check_cap(20 * GIB + 1, 20).unwrap_err().status, 413);
        assert!(check_cap(GIB, 1).is_ok());
    }

    #[test]
    fn a_partial_file_resumes_from_its_prefix_and_the_digest_covers_the_whole_file() {
        let dir = scratch("resume");
        let content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let target = dir.join("big.bin");
        let part = part_path(&target);
        let cut = 70_000;
        fs::write(&part, &content[..cut]).unwrap();
        assert_eq!(
            already_have(&dir, "big.bin", content.len() as u64),
            cut as u64
        );

        // The resumed transfer appends the rest; the hasher starts from the prefix already on disk.
        let mut hasher = Sha256::new();
        hash_prefix(&part, cut as u64, &mut hasher).unwrap();
        let mut out = OpenOptions::new().append(true).open(&part).unwrap();
        let flag = AtomicBool::new(false);
        let mut reader: &[u8] = &content[cut..];
        let mut reported = Vec::new();
        let step = transfer(
            &mut reader,
            &mut out,
            &mut hasher,
            cut as u64,
            content.len() as u64,
            &flag,
            &mut |b| reported.push(b),
        )
        .unwrap();
        assert_eq!(step, Step::Finished);
        assert_eq!(part.metadata().unwrap().len(), content.len() as u64);
        assert_eq!(
            hex(&hasher.finalize()),
            sha_hex(&content),
            "the digest covers the prefix too"
        );
        assert_eq!(reported.last(), Some(&(content.len() as u64)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transfer_refuses_more_bytes_than_the_repo_lists_and_stops_on_cancel() {
        let flag = AtomicBool::new(false);
        let mut out: Vec<u8> = Vec::new();
        let mut hasher = Sha256::new();
        let mut reader: &[u8] = b"0123456789";
        let error =
            transfer(&mut reader, &mut out, &mut hasher, 0, 4, &flag, &mut |_| {}).unwrap_err();
        assert!(error.contains("more bytes"), "{error}");

        let cancelled = AtomicBool::new(true);
        let mut reader: &[u8] = b"abc";
        let step = transfer(
            &mut reader,
            &mut out,
            &mut Sha256::new(),
            0,
            3,
            &cancelled,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(step, Step::Cancelled);
    }

    #[test]
    fn selections_are_checked_against_the_tree_and_the_defaults_follow_the_kind() {
        let tree = vec![
            TreeFile {
                path: "onnx/model.onnx".into(),
                size: 10,
                sha256: Some("a".repeat(64)),
            },
            TreeFile {
                path: "config.json".into(),
                size: 1,
                sha256: None,
            },
            TreeFile {
                path: "tokenizer.json".into(),
                size: 2,
                sha256: None,
            },
        ];
        let named = pick("org/x", "servable", &["config.json".into()], &tree).unwrap();
        assert_eq!(named.len(), 1);
        assert_eq!(
            pick("org/x", "servable", &["nope.bin".into()], &tree)
                .unwrap_err()
                .status,
            400
        );
        let defaults = pick("org/x", "servable", &[], &tree).unwrap();
        assert_eq!(
            defaults.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["onnx/model.onnx", "config.json", "tokenizer.json"]
        );
        assert_eq!(
            model_name("Xenova/distil.bert", "0b6928efcb76"),
            "hf-Xenova-distil-bert-0b6928ef"
        );
    }

    #[test]
    fn an_import_list_survives_a_restart_and_running_imports_become_interrupted() {
        let dir = scratch("list");
        let import = Import {
            n: 4,
            id: "org/x".into(),
            revision: "main".into(),
            sha: "a".repeat(40),
            as_: "base".into(),
            state: "downloading".into(),
            dir: dir.display().to_string(),
            files: vec![record("config.json", 1, None)],
            bytes: 0,
            total: 1,
            model_id: None,
            servable: None,
            reason: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let saved = Saved {
            next: 5,
            imports: vec![import],
        };
        fs::create_dir_all(dir.join("foundry").join("hf")).unwrap();
        fs::write(
            dir.join("foundry").join("hf").join("imports.json"),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        let opened = Imports::open(&dir);
        let listed = opened.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, "interrupted");
        assert_eq!(opened.lock().next, 5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_free_disk_of_a_directory_is_read_from_its_disk() {
        let dir = scratch("free");
        let free = free_bytes(&dir.join("nested"))
            .expect("the disk holding a temp directory has a mount point");
        assert!(free > 0, "a temp directory's disk has some free space");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_files_not_on_disk_at_their_size_are_downloaded() {
        let dir = scratch("missing");
        fs::write(dir.join("config.json"), b"{}").unwrap();
        let picked = vec![
            TreeFile {
                path: "config.json".into(),
                size: 2,
                sha256: None,
            },
            TreeFile {
                path: "tokenizer.json".into(),
                size: 3,
                sha256: None,
            },
        ];
        let missing = missing_files(&dir, &picked);
        assert_eq!(
            missing.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["tokenizer.json"]
        );
        // A file at the wrong size is not the one the repo lists: it is downloaded again.
        fs::write(dir.join("tokenizer.json"), b"toolong").unwrap();
        assert_eq!(missing_files(&dir, &picked).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_check_deletes_a_partial_file_and_never_a_finished_one() {
        let dir = scratch("discard");
        let part = dir.join("model.onnx.part");
        let finished = dir.join("model.onnx");
        fs::write(&part, b"partial").unwrap();
        fs::write(&finished, b"finished by another import").unwrap();
        discard_failed(&finished, &part);
        assert!(
            finished.exists() && part.exists(),
            "a finished file is left alone"
        );
        discard_failed(&part, &part);
        assert!(!part.exists() && finished.exists(), "the partial file goes");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_an_import_deletes_its_partial_files_and_keeps_the_finished_ones() {
        let dir = scratch("cancel");
        fs::write(dir.join("config.json"), b"{}").unwrap();
        fs::write(dir.join("model.onnx.part"), b"part").unwrap();
        let files = vec![
            record("config.json", 2, None),
            record("model.onnx", 9, None),
        ];
        clear_partials(&dir, &files, &["config.json".to_string()]);
        assert!(
            dir.join("config.json").exists(),
            "a finished file is never deleted by a cancel"
        );
        assert!(
            !dir.join("model.onnx.part").exists(),
            "the partial file is deleted"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A booted vault in `dir`, as the hub has it at start.
    fn vault_in(dir: &Path) -> crate::vault::Store {
        let vault = crate::vault::Store::open(dir, &dir.join("vault.key"), None);
        vault.boot(apz_plaza::now_ms()).expect("boot");
        vault
    }

    #[test]
    fn the_token_is_a_vault_entry_and_never_a_file_or_a_status_value() {
        let dir = scratch("token");
        let vault = vault_in(&dir);
        let imports = Imports::open(&dir);
        assert!(imports.token(&vault).is_none());
        imports.set_token(&vault, "  SENTINEL-HF-2290  ").unwrap();
        assert_eq!(imports.token(&vault).as_deref(), Some("SENTINEL-HF-2290"));
        assert!(
            !imports.dir().join("token").exists(),
            "no token file is written"
        );
        assert!(!vault.status(&[]).to_string().contains("SENTINEL-HF-2290"));
        assert_eq!(vault.presence("HF_TOKEN", "hf"), "secret");
        assert!(imports.set_token(&vault, "two words").is_err());
        assert!(imports.set_token(&vault, "").is_err());
        imports.clear_token(&vault).unwrap();
        assert!(imports.token(&vault).is_none());
        assert_eq!(vault.presence("HF_TOKEN", "hf"), "missing");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_check_names_the_account_and_never_the_token() {
        let value = serde_json::json!({ "name": "alice", "auth": { "accessToken": { "value": "SENTINEL-HF-2290" } } });
        assert_eq!(signed_in(&value).unwrap(), "signed in as alice");
        assert!(!signed_in(&value).unwrap().contains("SENTINEL-HF-2290"));
        assert!(signed_in(&serde_json::json!({})).is_err());
    }
}
