//! The work folder's two-way sync, the daemon's half (the hub's half is apid's `stations/sync.rs`).
//!
//! The daemon keeps a mirror of the work folder: for each synced file, its content hash, size and modification time.
//! A scan walks the folder every [`SCAN_EVERY`] and whenever the hub asks for the manifest. It reuses the hash of a file
//! whose size and modification time did not change, hashes the rest, and reports the changed paths to the hub as
//! `Event::SyncChanged`. The hub answers with `Op::Sync` requests: the manifest, pulls of content in chunks, pushes that
//! write content in place, and deletes. A write is atomic: the bytes go to a partial file beside the target, which is
//! renamed over the target once complete and hashed right. A push or a delete whose base no longer matches the file is
//! refused as a conflict; the hub settles it.
//!
//! Skipped on purpose: symbolic links (never followed, never written through), names [`apz_station::ignored_name`]
//! refuses, and files over [`apz_station::SYNC_FILE_CAP`]. A file that was synced and grew past the cap keeps its last
//! entry, so the hub does not read its absence as a deletion.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, UNIX_EPOCH};

use apz_station::{
    Event, FolderState, SYNC_CHUNK, SYNC_FILE_CAP, SYNC_PART_PREFIX, SyncEntry, SyncManifest, SyncOp, SyncPull,
    SyncPushed, SyncStatus, ignored_name, valid_path,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::daemon::Shared;

/// How often the scanner walks the folder.
pub const SCAN_EVERY: Duration = Duration::from_secs(2);
/// The most paths one `SyncChanged` event names.
const EVENT_PATHS: usize = 256;

/// What the daemon knows of one synced file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub hash: String,
    pub size: u64,
    pub mtime_ms: u64,
}

/// The mirror of the work folder, shared by the scanner and the worker that serves the hub's requests.
#[derive(Default)]
pub struct Mirror {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The folder the entries describe; a different folder (the owner moved it) starts from nothing.
    root: Option<PathBuf>,
    files: BTreeMap<String, Known>,
    /// The paths changed since the hub last asked for the manifest.
    pending: BTreeSet<String>,
    /// The last scan error, so a folder that stays missing is logged once.
    last_error: Option<String>,
}

/// One walk of the folder: its files, the paths that changed against the walk before, and the files left out.
#[derive(Debug, Default)]
pub struct Walk {
    pub files: BTreeMap<String, Known>,
    pub changed: Vec<String>,
    pub skipped: Vec<String>,
    /// The files and folders that are there but could not be read this time (or whose metadata could not be had). Their
    /// entries stand: a file that cannot be read is never reported as deleted.
    pub unreadable: Vec<String>,
}

impl Walk {
    /// Whether `path` lies under a folder, or is a file, that could not be read.
    fn is_unreadable(&self, path: &str) -> bool {
        self.unreadable
            .iter()
            .any(|at| path == at || path.strip_prefix(at.as_str()).is_some_and(|rest| rest.starts_with('/')))
    }
}

impl Mirror {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Walks `root` and merges the result into the mirror. Returns the paths that changed.
    pub fn scan(&self, root: &Path) -> io::Result<Vec<String>> {
        let previous = {
            let mut state = self.lock();
            if state.root.as_deref() != Some(root) {
                state.root = Some(root.to_path_buf());
                state.files.clear();
                state.pending.clear();
            }
            state.files.clone()
        };
        let walk = walk(root, &previous, SYNC_FILE_CAP)?;
        let mut state = self.lock();
        if state.root.as_deref() != Some(root) {
            // The folder moved while this walk ran: its result describes the old one.
            return Ok(Vec::new());
        }
        state.files = walk.files;
        state.pending.extend(walk.changed.iter().cloned());
        Ok(walk.changed)
    }

    /// Notes the error of a scan, returning true the first time a new error is seen.
    pub fn note_error(&self, error: &str) -> bool {
        let mut state = self.lock();
        let new = state.last_error.as_deref() != Some(error);
        state.last_error = Some(error.to_string());
        new
    }

    /// Clears the note of the last scan error, after a scan that worked.
    pub fn clear_error(&self) {
        self.lock().last_error = None;
    }

    fn known(&self, path: &str) -> Option<Known> {
        self.lock().files.get(path).cloned()
    }

    fn record(&self, path: &str, known: Known) {
        self.lock().files.insert(path.to_string(), known);
    }

    fn forget(&self, path: &str) {
        self.lock().files.remove(path);
    }

    /// The changed paths the hub has not yet fetched the manifest for: the scanner reports each change once, the
    /// hub's next manifest request settles them.
    fn status(&self) -> SyncStatus {
        let state = self.lock();
        SyncStatus {
            files: state.files.len() as u64,
            pending: state.pending.len() as u64,
        }
    }

    fn manifest(&self) -> SyncManifest {
        let mut state = self.lock();
        state.pending.clear();
        let entries = state
            .files
            .iter()
            .map(|(path, known)| SyncEntry {
                path: path.clone(),
                hash: known.hash.clone(),
                size: known.size,
            })
            .collect();
        SyncManifest {
            entries,
            state: FolderState::Present,
        }
    }
}

/// Walks `root`. `previous` is the walk before (its hashes are reused for unchanged files); `cap` is the largest file
/// hashed and synced. Symbolic links and ignored names are left out. An error means the folder itself is missing or
/// cannot be listed; a folder or file below it that cannot be read is named in `unreadable` instead.
pub fn walk(root: &Path, previous: &BTreeMap<String, Known>, cap: u64) -> io::Result<Walk> {
    if !fs::metadata(root)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a folder", root.display()),
        ));
    }
    let mut out = Walk::default();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let listed = fs::read_dir(&dir).and_then(|reader| reader.collect::<io::Result<Vec<_>>>());
        let entries = match listed {
            Ok(entries) => entries,
            // The folder must list: an empty walk of it would read every file as deleted.
            Err(err) if prefix.is_empty() => return Err(err),
            Err(_) => {
                out.unreadable.push(prefix);
                continue;
            }
        };
        for entry in entries {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if ignored_name(&name) {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let Ok(file_type) = entry.file_type() else {
                out.unreadable.push(rel);
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push((entry.path(), rel));
                continue;
            }
            if !file_type.is_file() || !valid_path(&rel) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                out.unreadable.push(rel);
                continue;
            };
            if meta.len() > cap {
                out.skipped.push(rel);
                continue;
            }
            let mtime_ms = mtime_ms(&meta);
            let known = match previous.get(&rel) {
                Some(old) if old.size == meta.len() && old.mtime_ms == mtime_ms => old.clone(),
                _ => match hash_file(&entry.path()) {
                    Ok(hash) => Known {
                        hash,
                        size: meta.len(),
                        mtime_ms,
                    },
                    Err(_) => {
                        out.unreadable.push(rel);
                        continue;
                    }
                },
            };
            out.files.insert(rel, known);
        }
    }
    for (path, old) in previous {
        if out.files.contains_key(path) {
            continue;
        }
        // Gone from the walk: a deletion, unless the file is still there (too big now, or unreadable this time).
        if out.is_unreadable(path) || fs::symlink_metadata(root.join(path)).is_ok_and(|m| m.is_file()) {
            out.files.insert(path.clone(), old.clone());
        }
    }
    for (path, known) in &out.files {
        if previous.get(path).is_none_or(|old| old.hash != known.hash) {
            out.changed.push(path.clone());
        }
    }
    for path in previous.keys() {
        if !out.files.contains_key(path) {
            out.changed.push(path.clone());
        }
    }
    out.changed.sort();
    Ok(out)
}

fn mtime_ms(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// The SHA-256 of a file's content, in lower-case hex, read in pieces.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Starts the scanner: every [`SCAN_EVERY`] it walks the work folder and reports the changes to the hub.
pub fn spawn(shared: Arc<Shared>) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            thread::sleep(SCAN_EVERY);
            let root = shared.root().dir().to_path_buf();
            match shared.sync.scan(&root) {
                Ok(changed) => {
                    shared.sync.clear_error();
                    for chunk in changed.chunks(EVENT_PATHS) {
                        shared.uplink.event(Event::SyncChanged {
                            paths: chunk.to_vec(),
                        });
                    }
                }
                Err(err) => {
                    if shared.sync.note_error(&err.to_string()) {
                        shared.log.warn(format!(
                            "work folder sync: cannot scan {}: {err}",
                            root.display()
                        ));
                    }
                }
            }
        }
    })
}

/// Serves one `Op::Sync` request on the work folder.
pub fn serve(shared: &Shared, op: SyncOp) -> Result<Option<Value>, String> {
    let root = shared.root().dir().to_path_buf();
    handle(&shared.sync, &root, op)
}

/// Serves one sync operation against the mirror of `root`. `serve` is this with the daemon's own state.
pub fn handle(mirror: &Mirror, root: &Path, op: SyncOp) -> Result<Option<Value>, String> {
    match op {
        SyncOp::Status => to_value(&mirror.status()).map(Some),
        SyncOp::Manifest => {
            // A folder that is gone or cannot be listed is reported as missing, never as an empty manifest: the hub
            // reads an empty manifest as every file deleted. The mirror keeps its entries for when the folder is back.
            let manifest = match mirror.scan(root) {
                Ok(_) => mirror.manifest(),
                Err(_) => SyncManifest {
                    entries: Vec::new(),
                    state: FolderState::Missing,
                },
            };
            to_value(&manifest).map(Some)
        }
        SyncOp::Pull { path, hash, offset } => {
            let chunk = pull(mirror, root, &path, &hash, offset.unwrap_or(0))?;
            to_value(&chunk).map(Some)
        }
        SyncOp::Push {
            path,
            hash,
            data_b64,
            base_hash,
            offset,
            total,
        } => {
            let piece = Piece {
                path: &path,
                hash: &hash,
                base: base_hash.as_deref(),
                offset: offset.unwrap_or(0),
                total,
            };
            let reply = push(mirror, root, &piece, &data_b64)?;
            to_value(&reply).map(Some)
        }
        SyncOp::Delete { path, base_hash } => {
            delete(mirror, root, &path, &base_hash).map(|()| None)
        }
        SyncOp::Resolve { .. } => Err(
            "a conflict is settled by the hub, with a pull and a push; resolve is not used".into(),
        ),
    }
}

/// The path of a file under `root`, refused when it is not a plain path there or a link is in the way.
fn target(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if !valid_path(rel) {
        return Err(format!("{rel:?} is not a path the sync takes"));
    }
    let mut full = root.to_path_buf();
    for segment in rel.split('/') {
        full.push(segment);
    }
    let canonical_root =
        fs::canonicalize(root).map_err(|e| format!("the work folder is not available: {e}"))?;
    // The deepest existing ancestor must lie under the root: a link on the way is refused, never followed.
    let mut ancestor = full.parent().unwrap_or(root).to_path_buf();
    loop {
        match fs::canonicalize(&ancestor) {
            Ok(real) => {
                if !real.starts_with(&canonical_root) {
                    return Err(format!("{rel} leaves the work folder"));
                }
                break;
            }
            Err(_) => {
                if !ancestor.pop() {
                    break;
                }
            }
        }
    }
    if fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!(
            "{rel} is a symbolic link; the sync never writes through one"
        ));
    }
    Ok(full)
}

/// The hash of the file now (`None` when there is none). The mirror's hash serves while the file's size and
/// modification time still match it; otherwise the file is hashed.
fn current_hash(mirror: &Mirror, rel: &str, full: &Path) -> Result<Option<String>, String> {
    let meta = match fs::symlink_metadata(full) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot read {rel}: {err}")),
    };
    if !meta.is_file() {
        return Err(format!("{rel} is not a regular file"));
    }
    if let Some(known) = mirror.known(rel)
        && known.size == meta.len()
        && known.mtime_ms == mtime_ms(&meta)
    {
        return Ok(Some(known.hash));
    }
    let hash = hash_file(full).map_err(|e| format!("cannot read {rel}: {e}"))?;
    mirror.record(
        rel,
        Known {
            hash: hash.clone(),
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
        },
    );
    Ok(Some(hash))
}

fn pull(
    mirror: &Mirror,
    root: &Path,
    rel: &str,
    hash: &str,
    offset: u64,
) -> Result<SyncPull, String> {
    let full = target(root, rel)?;
    let known = mirror
        .known(rel)
        .filter(|known| known.hash == hash)
        .ok_or_else(|| format!("{rel} has no content {hash} on the station now; the hub asks for the manifest again"))?;
    let mut file = File::open(&full).map_err(|e| format!("cannot read {rel}: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("cannot read {rel}: {e}"))?;
    if meta.len() != known.size || mtime_ms(&meta) != known.mtime_ms {
        return Err(format!(
            "{rel} changed since the last scan; the hub asks for the manifest again"
        ));
    }
    if offset > known.size {
        return Err(format!("the offset {offset} is past the end of {rel}"));
    }
    let len = (known.size - offset).min(SYNC_CHUNK as u64) as usize;
    let mut buf = vec![0u8; len];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut buf))
        .map_err(|e| format!("cannot read {rel}: {e}"))?;
    Ok(SyncPull {
        path: rel.to_string(),
        hash: known.hash,
        size: known.size,
        offset,
        data_b64: STANDARD.encode(&buf),
        eof: offset + len as u64 >= known.size,
    })
}

/// One piece of a push: the file's path and hash, the base the hub last saw, and where the piece goes.
struct Piece<'a> {
    path: &'a str,
    hash: &'a str,
    base: Option<&'a str>,
    offset: u64,
    total: Option<u64>,
}

/// The partial file a push writes beside its target: named by the content's hash, so two versions never share one.
fn part_path(full: &Path, hash: &str) -> PathBuf {
    let name = format!("{SYNC_PART_PREFIX}{}", &hash[..16]);
    full.with_file_name(name)
}

fn push(
    mirror: &Mirror,
    root: &Path,
    piece: &Piece<'_>,
    data_b64: &str,
) -> Result<SyncPushed, String> {
    let rel = piece.path;
    let full = target(root, rel)?;
    if !is_sha256(piece.hash) {
        return Err(format!("{} is not a sha256", piece.hash));
    }
    let data = STANDARD
        .decode(data_b64)
        .map_err(|e| format!("the piece is not base64: {e}"))?;
    let total = piece.total.unwrap_or(data.len() as u64);
    if total > SYNC_FILE_CAP {
        return Err(format!(
            "{rel} is {total} bytes; the sync takes files up to {SYNC_FILE_CAP}"
        ));
    }
    if data.len() > SYNC_CHUNK
        || piece
            .offset
            .checked_add(data.len() as u64)
            .is_none_or(|end| end > total)
    {
        return Err(format!("the piece of {rel} does not fit its file"));
    }
    let part = part_path(&full, piece.hash);
    if piece.offset == 0 {
        check_base(mirror, rel, &full, piece.base)?;
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create the folder of {rel}: {e}"))?;
        }
        // A new first piece starts the file again, whatever a previous attempt left.
        File::create(&part).map_err(|e| format!("cannot write {rel}: {e}"))?;
    }
    let have = fs::metadata(&part)
        .map(|m| m.len())
        .map_err(|_| format!("no partial copy of {rel}: send its first piece at offset 0"))?;
    if have != piece.offset {
        return Err(format!(
            "{rel}: the station has {have} bytes of it, so the piece at {} is out of order",
            piece.offset
        ));
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(&part)
        .map_err(|e| format!("cannot write {rel}: {e}"))?;
    file.write_all(&data)
        .map_err(|e| format!("cannot write {rel}: {e}"))?;
    let received = piece.offset + data.len() as u64;
    if received < total {
        return Ok(SyncPushed {
            path: rel.to_string(),
            received,
            hash: None,
        });
    }
    file.sync_all()
        .map_err(|e| format!("cannot write {rel}: {e}"))?;
    drop(file);
    let finish = |err: String| {
        let _ = fs::remove_file(&part);
        err
    };
    if hash_file(&part).map_err(|e| finish(format!("cannot read {rel}: {e}")))? != piece.hash {
        return Err(finish(format!(
            "the content of {rel} does not hash to {}",
            piece.hash
        )));
    }
    // The base is checked again: the station's copy may have moved on between the pieces.
    check_base(mirror, rel, &full, piece.base).map_err(finish)?;
    fs::rename(&part, &full).map_err(|e| finish(format!("cannot put {rel} in place: {e}")))?;
    let meta = fs::metadata(&full).map_err(|e| format!("cannot read {rel}: {e}"))?;
    mirror.record(
        rel,
        Known {
            hash: piece.hash.to_string(),
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
        },
    );
    Ok(SyncPushed {
        path: rel.to_string(),
        received,
        hash: Some(piece.hash.to_string()),
    })
}

/// A write goes ahead only when the file is still the version the writer saw (`base`; none for a new file).
fn check_base(mirror: &Mirror, rel: &str, full: &Path, base: Option<&str>) -> Result<(), String> {
    let current = current_hash(mirror, rel, full)?;
    if current.as_deref() == base {
        Ok(())
    } else {
        Err(format!(
            "conflict: {rel} changed on the station since the hub last saw it"
        ))
    }
}

fn delete(mirror: &Mirror, root: &Path, rel: &str, base: &str) -> Result<(), String> {
    let full = target(root, rel)?;
    match current_hash(mirror, rel, &full)? {
        None => {
            mirror.forget(rel);
            Ok(())
        }
        Some(hash) if hash == base => {
            fs::remove_file(&full).map_err(|e| format!("cannot remove {rel}: {e}"))?;
            mirror.forget(rel);
            Ok(())
        }
        Some(_) => Err(format!(
            "conflict: {rel} changed on the station since the hub last saw it"
        )),
    }
}

fn to_value<T: Serialize>(item: &T) -> Result<Value, String> {
    serde_json::to_value(item).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A fresh folder under the system's temp directory (the tests never touch a real home).
    fn scratch(tag: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "apz-ws-sync-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A push of one piece: `span` is (offset, total).
    fn put(
        mirror: &Mirror,
        root: &Path,
        path: &str,
        hash: &str,
        bytes: &[u8],
        base: Option<&str>,
        span: (u64, u64),
    ) -> Result<Value, String> {
        let op = SyncOp::Push {
            path: path.into(),
            hash: hash.into(),
            data_b64: STANDARD.encode(bytes),
            base_hash: base.map(str::to_string),
            offset: Some(span.0),
            total: Some(span.1),
        };
        handle(mirror, root, op).map(|v| v.unwrap())
    }

    #[test]
    fn a_walk_lists_the_files_and_reports_only_what_changed() {
        let root = scratch("walk");
        write(&root, "a.txt", b"hello");
        write(&root, "sub/b.md", b"# b");
        write(&root, "~$lock.docx", b"x");
        write(&root, ".#note.md", b"x");
        write(&root, "old.swp", b"x");
        write(&root, ".DS_Store", b"x");
        write(&root, ".apz-part-0123456789abcdef", b"partial");
        let first = walk(&root, &BTreeMap::new(), SYNC_FILE_CAP).unwrap();
        let names: Vec<&str> = first.files.keys().map(String::as_str).collect();
        assert_eq!(names, ["a.txt", "sub/b.md"]);
        assert_eq!(first.changed, ["a.txt", "sub/b.md"]);
        assert_eq!(first.files["a.txt"].hash, sha(b"hello"));

        let again = walk(&root, &first.files, SYNC_FILE_CAP).unwrap();
        assert!(again.changed.is_empty(), "nothing changed");

        write(&root, "a.txt", b"hello, longer");
        fs::remove_file(root.join("sub/b.md")).unwrap();
        let later = walk(&root, &again.files, SYNC_FILE_CAP).unwrap();
        assert_eq!(later.changed, ["a.txt", "sub/b.md"]);
        assert_eq!(later.files["a.txt"].hash, sha(b"hello, longer"));
        assert!(!later.files.contains_key("sub/b.md"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_over_the_cap_is_left_out_but_keeps_its_entry() {
        let root = scratch("cap");
        write(&root, "small.txt", b"tiny");
        write(&root, "grows.bin", b"12345");
        let first = walk(&root, &BTreeMap::new(), 10).unwrap();
        assert_eq!(first.files.len(), 2);
        write(&root, "grows.bin", &[7u8; 64]);
        let second = walk(&root, &first.files, 10).unwrap();
        assert_eq!(second.skipped, ["grows.bin"]);
        assert_eq!(
            second.files["grows.bin"].hash, first.files["grows.bin"].hash,
            "the last entry stands"
        );
        assert!(second.changed.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// The manifest the hub gets for `root` from `mirror`.
    fn manifest_of(mirror: &Mirror, root: &Path) -> SyncManifest {
        serde_json::from_value(handle(mirror, root, SyncOp::Manifest).unwrap().unwrap()).unwrap()
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_missing_folder_is_reported_missing_not_as_an_empty_manifest() {
        let root = scratch("missing");
        write(&root, "keep.txt", b"kept");
        let mirror = Mirror::default();
        let present = manifest_of(&mirror, &root);
        assert_eq!((present.state, present.entries.len()), (FolderState::Present, 1));

        fs::remove_dir_all(&root).unwrap();
        let gone = manifest_of(&mirror, &root);
        assert_eq!((gone.state, gone.entries.len()), (FolderState::Missing, 0));
        assert_eq!(mirror.status().files, 1, "the mirror keeps its entries for when the folder is back");

        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("keep.txt"), b"kept").unwrap();
        let back = manifest_of(&mirror, &root);
        assert_eq!((back.state, back.entries.len()), (FolderState::Present, 1));
        assert!(mirror.status().pending == 0, "the same file is not a change");

        fs::remove_file(root.join("keep.txt")).unwrap();
        let wiped = manifest_of(&mirror, &root);
        assert_eq!(wiped.state, FolderState::Present, "an emptied folder is present; the hub's threshold guards it");
        assert!(wiped.entries.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_listed_is_missing_and_its_files_are_not_deletions() {
        let root = scratch("unlistable");
        write(&root, "a.txt", b"a");
        let mirror = Mirror::default();
        assert_eq!(manifest_of(&mirror, &root).entries.len(), 1);

        set_mode(&root, 0o000);
        let locked = manifest_of(&mirror, &root);
        set_mode(&root, 0o755);
        if locked.state == FolderState::Present {
            // Running with rights that ignore the mode (root): there is nothing to test here.
            fs::remove_dir_all(root).unwrap();
            return;
        }
        assert_eq!((locked.state, locked.entries.len()), (FolderState::Missing, 0));

        let back = manifest_of(&mirror, &root);
        assert_eq!((back.state, back.entries.len()), (FolderState::Present, 1));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_subfolder_that_cannot_be_read_keeps_its_files_in_the_manifest() {
        let root = scratch("unreadable");
        write(&root, "a.txt", b"a");
        write(&root, "locked/b.txt", b"bee");
        let mirror = Mirror::default();
        assert_eq!(manifest_of(&mirror, &root).entries.len(), 2);

        set_mode(&root.join("locked"), 0o000);
        let walked = walk(&root, &mirror.lock().files.clone(), SYNC_FILE_CAP);
        let second = manifest_of(&mirror, &root);
        set_mode(&root.join("locked"), 0o755);
        let walked = walked.unwrap();
        assert_eq!(walked.unreadable, ["locked"], "the folder is named as unreadable");
        assert!(walked.changed.is_empty(), "nothing in it is a change");
        assert_eq!(second.state, FolderState::Present);
        let paths: Vec<&str> = second.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "locked/b.txt"], "the unreadable folder's file stands, not deleted");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_manifest_is_the_scanned_folder_and_asking_for_it_clears_pending() {
        let root = scratch("manifest");
        write(&root, "out/render.png", b"png-bytes");
        let mirror = Mirror::default();
        assert_eq!(
            mirror.status(),
            SyncStatus {
                files: 0,
                pending: 0
            }
        );
        let manifest: SyncManifest =
            serde_json::from_value(handle(&mirror, &root, SyncOp::Manifest).unwrap().unwrap())
                .unwrap();
        assert_eq!(
            manifest.entries,
            vec![SyncEntry {
                path: "out/render.png".into(),
                hash: sha(b"png-bytes"),
                size: 9
            }]
        );
        assert_eq!(
            mirror.status(),
            SyncStatus {
                files: 1,
                pending: 0
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_large_file_is_pulled_in_chunks_that_join_back_into_it() {
        let root = scratch("pull");
        let bytes: Vec<u8> = (0..SYNC_CHUNK + 5).map(|i| (i % 251) as u8).collect();
        write(&root, "big.bin", &bytes);
        let mirror = Mirror::default();
        handle(&mirror, &root, SyncOp::Manifest).unwrap();
        let hash = sha(&bytes);
        let first: SyncPull = serde_json::from_value(
            handle(
                &mirror,
                &root,
                SyncOp::Pull {
                    path: "big.bin".into(),
                    hash: hash.clone(),
                    offset: None,
                },
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            (first.offset, first.size, first.eof),
            (0, bytes.len() as u64, false)
        );
        let second: SyncPull = serde_json::from_value(
            handle(
                &mirror,
                &root,
                SyncOp::Pull {
                    path: "big.bin".into(),
                    hash: hash.clone(),
                    offset: Some(SYNC_CHUNK as u64),
                },
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        assert!(second.eof);
        let mut joined = STANDARD.decode(&first.data_b64).unwrap();
        joined.extend(STANDARD.decode(&second.data_b64).unwrap());
        assert_eq!(joined, bytes);
        assert_eq!(sha(&joined), hash);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pull_refuses_another_hash_and_a_file_changed_since_the_scan() {
        let root = scratch("pull-refuse");
        write(&root, "a.txt", b"first");
        let mirror = Mirror::default();
        handle(&mirror, &root, SyncOp::Manifest).unwrap();
        let wrong = handle(
            &mirror,
            &root,
            SyncOp::Pull {
                path: "a.txt".into(),
                hash: sha(b"other"),
                offset: None,
            },
        );
        assert!(wrong.is_err());
        write(&root, "a.txt", b"second, longer");
        let stale = handle(
            &mirror,
            &root,
            SyncOp::Pull {
                path: "a.txt".into(),
                hash: sha(b"first"),
                offset: None,
            },
        );
        assert!(stale.unwrap_err().contains("changed since the last scan"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_push_in_pieces_lands_atomically_with_its_hash_checked() {
        let root = scratch("push");
        let mirror = Mirror::default();
        let bytes: Vec<u8> = (0..SYNC_CHUNK + 3).map(|i| (i % 13) as u8).collect();
        let hash = sha(&bytes);
        let (head, tail) = bytes.split_at(SYNC_CHUNK);
        let total = bytes.len() as u64;

        let first = put(
            &mirror,
            &root,
            "deep/dir/out.bin",
            &hash,
            head,
            None,
            (0, total),
        )
        .unwrap();
        assert_eq!(first["received"], SYNC_CHUNK as u64);
        assert!(first.get("hash").is_none());
        assert!(
            !root.join("deep/dir/out.bin").exists(),
            "nothing in place until the last piece"
        );

        let last = put(
            &mirror,
            &root,
            "deep/dir/out.bin",
            &hash,
            tail,
            None,
            (SYNC_CHUNK as u64, total),
        )
        .unwrap();
        assert_eq!(last["hash"], hash.as_str());
        assert_eq!(fs::read(root.join("deep/dir/out.bin")).unwrap(), bytes);
        let leftovers: Vec<_> = fs::read_dir(root.join("deep/dir"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            leftovers.len(),
            1,
            "the partial copy is gone: {leftovers:?}"
        );
        assert_eq!(
            mirror.known("deep/dir/out.bin").unwrap().hash,
            hash,
            "the mirror knows the new file"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_push_refuses_a_base_that_no_longer_matches_and_leaves_the_file_alone() {
        let root = scratch("conflict");
        write(&root, "notes.md", b"the station's edit");
        let mirror = Mirror::default();
        let hub = b"the hub's edit";
        let refused = put(
            &mirror,
            &root,
            "notes.md",
            &sha(hub),
            hub,
            Some(&sha(b"the old version")),
            (0, hub.len() as u64),
        );
        assert!(refused.unwrap_err().contains("conflict"));
        let refused_new = put(
            &mirror,
            &root,
            "notes.md",
            &sha(hub),
            hub,
            None,
            (0, hub.len() as u64),
        );
        assert!(
            refused_new.unwrap_err().contains("conflict"),
            "a new file over an existing one is a conflict"
        );
        assert_eq!(
            fs::read(root.join("notes.md")).unwrap(),
            b"the station's edit"
        );

        let base = sha(b"the station's edit");
        put(
            &mirror,
            &root,
            "notes.md",
            &sha(hub),
            hub,
            Some(&base),
            (0, hub.len() as u64),
        )
        .unwrap();
        assert_eq!(fs::read(root.join("notes.md")).unwrap(), hub);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_push_with_a_wrong_hash_or_an_out_of_order_piece_writes_nothing() {
        let root = scratch("bad-push");
        let mirror = Mirror::default();
        let err = put(
            &mirror,
            &root,
            "x.txt",
            &sha(b"other"),
            b"data",
            None,
            (0, 4),
        )
        .unwrap_err();
        assert!(err.contains("does not hash"));
        assert!(!root.join("x.txt").exists());
        assert!(
            !root
                .join(
                    part_path(&root.join("x.txt"), &sha(b"other"))
                        .file_name()
                        .unwrap()
                )
                .exists()
        );

        let early = put(
            &mirror,
            &root,
            "y.txt",
            &sha(b"abcdef"),
            b"def",
            None,
            (3, 6),
        )
        .unwrap_err();
        assert!(early.contains("no partial copy"), "{early}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_path_outside_the_folder_is_refused() {
        let root = scratch("escape");
        let mirror = Mirror::default();
        for bad in ["../outside.txt", "/abs.txt", "a/../../b", "~$word.docx"] {
            let op = SyncOp::Delete {
                path: bad.into(),
                base_hash: sha(b"x"),
            };
            assert!(handle(&mirror, &root, op).is_err(), "{bad}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_way_is_never_written_through() {
        let root = scratch("link");
        let outside = scratch("link-outside");
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        let mirror = Mirror::default();
        let err = put(
            &mirror,
            &root,
            "linked/file.txt",
            &sha(b"x"),
            b"x",
            None,
            (0, 1),
        )
        .unwrap_err();
        assert!(err.contains("leaves the work folder"), "{err}");
        assert!(!outside.join("file.txt").exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn a_delete_needs_the_base_it_was_asked_for() {
        let root = scratch("delete");
        write(&root, "gone.txt", b"keep me");
        let mirror = Mirror::default();
        handle(&mirror, &root, SyncOp::Manifest).unwrap();
        let mismatch = handle(
            &mirror,
            &root,
            SyncOp::Delete {
                path: "gone.txt".into(),
                base_hash: sha(b"other"),
            },
        );
        assert!(mismatch.unwrap_err().contains("conflict"));
        assert!(root.join("gone.txt").exists());
        handle(
            &mirror,
            &root,
            SyncOp::Delete {
                path: "gone.txt".into(),
                base_hash: sha(b"keep me"),
            },
        )
        .unwrap();
        assert!(!root.join("gone.txt").exists());
        assert!(
            handle(
                &mirror,
                &root,
                SyncOp::Delete {
                    path: "gone.txt".into(),
                    base_hash: sha(b"keep me")
                }
            )
            .is_ok(),
            "already gone"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolve_is_refused_because_the_hub_settles_conflicts_itself() {
        let root = scratch("resolve");
        let mirror = Mirror::default();
        let op = SyncOp::Resolve {
            path: "a".into(),
            keep: apz_station::Keep::Both,
        };
        assert!(handle(&mirror, &root, op).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
