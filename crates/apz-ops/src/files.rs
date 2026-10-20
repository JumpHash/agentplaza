//! The file operations: list, read, write and search inside a [`Root`].
//!
//! # Paths
//!
//! [`apz_ide::path`] refuses the lexical cases: absolute, `..`, secrets, and writes under `.plaza/`.
//! Then the real filesystem: the deepest existing ancestor is canonicalized (which follows links) and
//! must land inside the canonical root, and the place it lands is checked by the same rules, so a link
//! into a secret or into `.plaza/` is refused too. A write also refuses a target that is itself a
//! symlink. `.plaza/` is readable, never writable, and hidden from listings and search.
//!
//! # Replies
//!
//! Each reply struct serializes with its fields in alphabetical order, which is the order a JSON
//! object built from a map has, so the bytes are the same whichever way a caller encodes them. A
//! paged reply stays under [`OUTPUT_BUDGET`]; text is paged in chunks of at most [`CHUNK`] bytes.
//!
//! # Writes and hooks
//!
//! A write is checked against the hash the caller last read, then handed to [`WriteHooks`] before
//! anything changes, so a caller can take a checkpoint of the state it is about to overwrite. The hook
//! also sees each folder and the temporary file a write creates, so a caller can set their ownership.
//! On unix the folders, the temporary file and the rename all go through handles to the root and to
//! the folder, never through a path, so a folder swapped for a link mid-write is refused.

#[cfg(unix)]
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
#[cfg(not(unix))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use apz_ide::path::{check_dir, check_read, check_write, is_secret};
use apz_ide::tree::{Entry, EntryKind, PAGE_SIZE, TreePage};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::OpError;
use crate::page::{OUTPUT_BUDGET, chunk_end};
use crate::root::{Root, Target, locate, native, shown};
#[cfg(unix)]
use crate::root::{create_new_under, make_confined_dirs, remove_under, rename_under};

/// The largest file the operations read or write as text: 1 MiB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The largest search needle, in bytes.
pub const MAX_NEEDLE_BYTES: usize = 4 * 1024;
/// The most matches one search returns.
pub const MAX_SEARCH_MATCHES: usize = 200;
/// How much of a matching line a search returns, in characters.
pub const SEARCH_LINE_CHARS: usize = 200;
/// The most bytes one search reads, across all files.
pub const MAX_SEARCH_SCAN: u64 = 20 * 1024 * 1024;
/// Bytes sniffed for a NUL when a listing says whether a file is binary.
const BINARY_PROBE: u64 = 8 * 1024;
/// The harness's state directory. Hidden from listings and search, and never writable.
const PLAZA_DIR: &str = ".plaza";

/// What a [`read`] returned. `hash` is the sha256 of the whole file, which a later [`write`] sends back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadReply {
    pub done: bool,
    pub hash: String,
    pub offset: usize,
    pub path: String,
    pub size: usize,
    pub text: String,
}

/// What a [`write`] did. `checkpoint` is the id the hook returned, if it returned one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteReply {
    pub checkpoint: Option<String>,
    pub hash: String,
    pub path: String,
}

/// One line that matched a [`search`]. `text` is cut to [`SEARCH_LINE_CHARS`] characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchMatch {
    pub line: usize,
    pub path: String,
    pub text: String,
}

/// What a [`search`] found. `truncated` says the scan or the reply stopped before the end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchReply {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

/// Hooks a [`write`] calls around its changes. Both have defaults that do nothing.
pub trait WriteHooks {
    /// Called once the hash check has passed, before anything is written, with the path relative to
    /// the root. Returns the checkpoint id to report, or `None` for none.
    fn before_write(&mut self, _path: &Path) -> Result<Option<String>, OpError> {
        Ok(None)
    }

    /// Called for each folder a write creates, and for the temporary file it writes before renaming
    /// it into place.
    fn created(&mut self, _path: &Path) {}
}

/// The hooks that do nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl WriteHooks for NoHooks {}

/// The entry a listing sorts and pages by: its path, name and kind.
struct Listed {
    rel: String,
    name: String,
    kind: EntryKind,
    size: u64,
}

/// One page of a directory, directories first, then by name. `cursor` is the `next` of the page before.
///
/// Secret paths are skipped, and `.plaza` at the root is hidden. A page holds up to [`PAGE_SIZE`]
/// entries, and fewer when the reply would not fit [`OUTPUT_BUDGET`].
pub fn list(root: &Root, path: &str, cursor: Option<&str>) -> Result<TreePage, OpError> {
    let target = locate(root, path, check_dir, check_dir)?;
    let metadata = fs::metadata(&target.abs)
        .map_err(|err| OpError::io(err, format!("cannot list {}", shown(&target.rel))))?;
    if !metadata.is_dir() {
        return Err(OpError::invalid(format!(
            "{} is not a directory",
            shown(&target.rel)
        )));
    }
    let root_listing = target.rel.is_empty();
    let items = fs::read_dir(&target.abs)
        .map_err(|err| OpError::io(err, format!("cannot list {}", shown(&target.rel))))?;
    let mut found: Vec<Listed> = Vec::new();
    for item in items.flatten() {
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        if root_listing && name == PLAZA_DIR {
            continue;
        }
        let rel = if root_listing {
            name.clone()
        } else {
            format!("{}/{name}", target.rel)
        };
        if is_secret(&rel) {
            continue;
        }
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Dir
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            continue;
        };
        let size = match kind {
            EntryKind::File => item.metadata().map_or(0, |meta| meta.len()),
            EntryKind::Dir | EntryKind::Symlink => 0,
        };
        found.push(Listed {
            rel,
            name,
            kind,
            size,
        });
    }
    found.sort_by_key(key_of);

    let from = match cursor {
        None => 0,
        Some(cursor) => {
            let key = parse_cursor(cursor)?;
            found.partition_point(|item| key_of(item) <= key)
        }
    };
    let mut entries = Vec::new();
    let mut used = 128;
    let mut at = from;
    while at < found.len() && entries.len() < PAGE_SIZE {
        let item = &found[at];
        let binary = item.kind == EntryKind::File && sniff_binary(root.dir(), &native(&item.rel));
        let entry = Entry {
            path: item.rel.clone(),
            kind: item.kind,
            size: item.size,
            hash: None,
            binary,
        };
        let cost =
            serde_json::to_string(&entry).map_or(0, |text| text.len()) + item.name.len() + 16;
        if !entries.is_empty() && used + cost > OUTPUT_BUDGET {
            break;
        }
        used += cost;
        entries.push(entry);
        at += 1;
    }
    let next = (at < found.len()).then(|| {
        let last = &found[at - 1];
        cursor_of(last.kind == EntryKind::Dir, &last.name)
    });
    Ok(TreePage { entries, next })
}

/// A chunk of a text file from byte `offset` (the start when `None`).
///
/// A file over [`MAX_FILE_BYTES`], a binary file and a file that is not UTF-8 are refused.
pub fn read(root: &Root, path: &str, offset: Option<u64>) -> Result<ReadReply, OpError> {
    let target = locate(root, path, check_read, check_read)?;
    let metadata = fs::metadata(&target.abs)
        .map_err(|err| OpError::io(err, format!("cannot read {}", target.rel)))?;
    if metadata.is_dir() {
        return Err(OpError::invalid(format!(
            "{} is a directory; a read takes one file",
            target.rel
        )));
    }
    // Only a regular file is opened: a FIFO would wait for a writer, and a device or socket is not a file.
    if !metadata.is_file() {
        return Err(OpError::invalid(format!(
            "{} is not a regular file; a read takes one file",
            target.rel
        )));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(OpError::limit(format!(
            "{} is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
            target.rel,
            metadata.len()
        )));
    }
    let bytes = read_capped(root.dir(), &native(&target.rel))
        .map_err(|err| OpError::io(err, format!("cannot read {}", target.rel)))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(OpError::limit(format!(
            "{} grew past the {MAX_FILE_BYTES}-byte cap while it was read",
            target.rel
        )));
    }
    let text = text_of(&target.rel, &bytes)?;
    let hash = sha_hex(&bytes);
    let offset = match offset {
        None => 0,
        Some(at) => usize::try_from(at)
            .map_err(|_| OpError::invalid("\"offset\" must be a whole number of bytes"))?,
    };
    if offset > text.len() {
        return Err(OpError::invalid(format!(
            "offset {offset} is past the end of {} ({} bytes)",
            target.rel,
            text.len()
        )));
    }
    if !text.is_char_boundary(offset) {
        return Err(OpError::invalid(format!(
            "offset {offset} is inside a character of {}",
            target.rel
        )));
    }
    let reply = |end: usize| ReadReply {
        done: end == text.len(),
        hash: hash.clone(),
        offset,
        path: target.rel.clone(),
        size: text.len(),
        text: text[offset..end].to_string(),
    };
    let end = chunk_end(text, offset, |end| {
        serde_json::to_string(&reply(end)).is_ok_and(|json| json.len() <= OUTPUT_BUDGET)
    });
    Ok(reply(end))
}

/// Replaces a text file, or creates it.
///
/// `expected_hash` is the hash the file had when last read, or `None` for a file that must not exist
/// yet. A mismatch is refused as [`OpError::Conflict`] (with the current hash, or `missing`), and
/// nothing is written and no hook runs. Otherwise `hooks` run, the file is written atomically, and the
/// reply carries the new hash.
pub fn write(
    root: &Root,
    path: &str,
    text: &str,
    expected_hash: Option<&str>,
    hooks: &mut dyn WriteHooks,
) -> Result<WriteReply, OpError> {
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(OpError::limit(format!(
            "the text is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
            text.len()
        )));
    }
    let target = locate(root, path, check_write, check_write)?;
    refuse_symlink_target(&target.abs, &target.rel)?;
    let current = match fs::metadata(&target.abs) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(OpError::invalid(format!(
                "{} is a directory; a write takes one file",
                target.rel
            )));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(OpError::invalid(format!(
                "{} is not a regular file; a write takes one file",
                target.rel
            )));
        }
        Ok(metadata) => {
            if metadata.len() > MAX_FILE_BYTES {
                return Err(OpError::limit(format!(
                    "{} is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
                    target.rel,
                    metadata.len()
                )));
            }
            let bytes = read_capped(root.dir(), &native(&target.rel))
                .map_err(|err| OpError::io(err, format!("cannot read {}", target.rel)))?;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(OpError::limit(format!(
                    "{} grew past the {MAX_FILE_BYTES}-byte cap while it was read",
                    target.rel
                )));
            }
            Some(bytes)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(OpError::io(err, format!("cannot read {}", target.rel))),
    };
    let current_hash = current.as_deref().map(sha_hex);
    let refusal = match (expected_hash, &current_hash) {
        (None, None) => None,
        (None, Some(now)) => Some(now.clone()),
        (Some(_), None) => Some("missing".to_string()),
        (Some(want), Some(now)) => (want != now.as_str()).then(|| now.clone()),
    };
    if let Some(reason) = refusal {
        return Err(OpError::Conflict(reason));
    }

    let checkpoint = hooks.before_write(Path::new(&target.rel))?;
    let permissions = fs::metadata(&target.abs)
        .ok()
        .map(|metadata| metadata.permissions());
    place(root, &target, text.as_bytes(), permissions, hooks)?;
    Ok(WriteReply {
        checkpoint,
        hash: sha_hex(text.as_bytes()),
        path: target.rel,
    })
}

/// A plain substring search of the text files under the root.
///
/// Case-insensitive when `q` has no uppercase letters. `glob` matches the path relative to the root.
/// `max` defaults to and is capped at [`MAX_SEARCH_MATCHES`]. Binary, secret and `.plaza/` files are
/// skipped, and the scan stops after [`MAX_SEARCH_SCAN`] bytes. Symlinks are neither followed nor
/// searched.
pub fn search(
    root: &Root,
    q: &str,
    glob: Option<&str>,
    max: Option<usize>,
) -> Result<SearchReply, OpError> {
    if q.is_empty() {
        return Err(OpError::invalid("\"q\" must not be empty"));
    }
    if q.len() > MAX_NEEDLE_BYTES {
        return Err(OpError::limit(format!(
            "\"q\" is {} bytes, over the {MAX_NEEDLE_BYTES}-byte cap",
            q.len()
        )));
    }
    let glob = match glob {
        None => None,
        Some(pattern) => Some(
            globset::Glob::new(pattern)
                .map_err(|err| OpError::invalid(format!("\"glob\" is not a glob: {err}")))?
                .compile_matcher(),
        ),
    };
    let max = max.unwrap_or(MAX_SEARCH_MATCHES);
    if max == 0 {
        return Err(OpError::invalid("\"max\" must be at least 1"));
    }
    if max > MAX_SEARCH_MATCHES {
        return Err(OpError::limit(format!(
            "\"max\" is {max}, over the {MAX_SEARCH_MATCHES}-match cap"
        )));
    }
    let case_blind = q == q.to_lowercase();
    let mut scan = Scan {
        needle: if case_blind {
            q.to_lowercase()
        } else {
            q.to_string()
        },
        case_blind,
        glob,
        max,
        matches: Vec::new(),
        used: 64,
        scanned: 0,
        truncated: false,
    };
    let canonical = root.canonical()?;
    walk_for_search(&canonical, "", &mut scan);
    Ok(SearchReply {
        matches: scan.matches,
        truncated: scan.truncated,
    })
}

// ---------------------------------------------------------------------------------------------
// Listing order and cursors
// ---------------------------------------------------------------------------------------------

/// The listing order: directories first, then by name without regard to case, then exactly.
fn display_key(is_dir: bool, name: &str) -> (u8, String, String) {
    (u8::from(!is_dir), name.to_lowercase(), name.to_string())
}

fn key_of(item: &Listed) -> (u8, String, String) {
    display_key(item.kind == EntryKind::Dir, &item.name)
}

/// A page cursor: `0` and the name for a directory, `1` and the name for anything else.
fn cursor_of(is_dir: bool, name: &str) -> String {
    format!("{}{name}", if is_dir { '0' } else { '1' })
}

fn parse_cursor(cursor: &str) -> Result<(u8, String, String), OpError> {
    let (is_dir, name) = match (cursor.strip_prefix('0'), cursor.strip_prefix('1')) {
        (Some(name), _) => (true, name),
        (None, Some(name)) => (false, name),
        (None, None) => return Err(OpError::invalid("\"cursor\" is not a cursor from ide_list")),
    };
    if name.is_empty() || name.contains('/') {
        return Err(OpError::invalid("\"cursor\" is not a cursor from ide_list"));
    }
    Ok(display_key(is_dir, name))
}

// ---------------------------------------------------------------------------------------------
// Bytes and writes
// ---------------------------------------------------------------------------------------------

/// Refuses a write whose target is itself a symlink. Resolving the deepest existing ancestor does not
/// see a dangling link inside the root, and a live link would make the write land wherever it points.
/// Reads may follow links that resolve inside; a write never follows one.
pub(crate) fn refuse_symlink_target(path: &Path, rel: &str) -> Result<(), OpError> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(OpError::denied(format!(
            "{rel} is a symlink; a write never follows one"
        )));
    }
    Ok(())
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The bytes of a file under `root`, read at most one byte past the cap so an over-cap file is seen.
/// The file is opened by [`crate::root::open_confined`], so a link or a non-file swapped in since the
/// path check is refused rather than followed.
fn read_capped(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    crate::root::open_confined(root, rel)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The bytes as text, or a refusal: the operations open text only.
fn text_of<'a>(rel: &str, bytes: &'a [u8]) -> Result<&'a str, OpError> {
    if bytes.contains(&0) {
        return Err(OpError::invalid(format!(
            "{rel} is binary (it holds a NUL byte); the IDE opens text only"
        )));
    }
    std::str::from_utf8(bytes)
        .map_err(|_| OpError::invalid(format!("{rel} is not UTF-8 text; the IDE opens text only")))
}

/// Whether the first [`BINARY_PROBE`] bytes of a file under `root` hold a NUL.
fn sniff_binary(root: &Path, rel: &Path) -> bool {
    let Ok(file) = crate::root::open_confined(root, rel) else {
        return false;
    };
    let mut head = Vec::new();
    if file.take(BINARY_PROBE).read_to_end(&mut head).is_err() {
        return false;
    }
    head.contains(&0)
}

/// Places `text` as the file `target` names: the folders it needs are made, then the file is written
/// atomically. On unix every step goes through a handle, as [`write_in`] describes.
#[cfg(unix)]
fn place(
    root: &Root,
    target: &Target,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
    hooks: &mut dyn WriteHooks,
) -> Result<(), OpError> {
    let (parent, name) = split(&target.rel);
    let (folder, made) = make_confined_dirs(root.dir(), &native(parent))
        .map_err(|err| OpError::io(err, format!("cannot create the folders of {}", target.rel)))?;
    for path in made.iter().rev() {
        hooks.created(path);
    }
    write_in(&folder, &target.abs, name, bytes, permissions, hooks)
        .map_err(|err| OpError::io(err, format!("cannot write {}", target.rel)))
}

/// The folder and the name of a `/`-separated path. A name at the root has an empty folder.
#[cfg(unix)]
fn split(rel: &str) -> (&str, &str) {
    rel.rsplit_once('/').unwrap_or(("", rel))
}

/// Writes `bytes` to the file `name` in `folder` atomically: a temp file made exclusively in the same
/// folder, flushed, handed to the hooks, and renamed over the target. The rename makes the write
/// all-or-nothing; a failed write removes the temp file. Every step is a call on `folder`, so no path is
/// followed. `abs` is the target's path, under which the hook is given the temp file's path.
#[cfg(unix)]
fn write_in(
    folder: &fs::File,
    abs: &Path,
    name: &str,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
    hooks: &mut dyn WriteHooks,
) -> io::Result<()> {
    let name = OsStr::new(name);
    let (temp, mut file) = create_temp_file(folder, name)?;
    let temp_path = abs.with_file_name(&temp);
    let result = (|| {
        file.write_all(bytes)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        drop(file);
        hooks.created(&temp_path);
        rename_under(folder, &temp, name)
    })();
    if result.is_err() {
        let _ = remove_under(folder, &temp);
    }
    result
}

/// Creates a fresh temp file in `folder` exclusively. Existing names, links included, are never opened or
/// truncated. The counter keeps concurrent writers in this process from sharing a name.
#[cfg(unix)]
fn create_temp_file(folder: &fs::File, name: &OsStr) -> io::Result<(OsString, fs::File)> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    for _ in 0..128 {
        let seq = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let mut temp = OsString::from(".");
        temp.push(name);
        temp.push(format!(".plaza-tmp-{}-{seq}", std::process::id()));
        match create_new_under(folder, &temp) {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a workspace temporary file",
    ))
}

/// Windows keeps the path-based write: the folders by `create_dir_all`, the temp file by path.
#[cfg(not(unix))]
fn place(
    _root: &Root,
    target: &Target,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
    hooks: &mut dyn WriteHooks,
) -> Result<(), OpError> {
    if let Some(parent) = target.abs.parent() {
        create_dirs(parent, hooks).map_err(|err| {
            OpError::io(err, format!("cannot create the folders of {}", target.rel))
        })?;
    }
    write_atomic(&target.abs, bytes, permissions, hooks)
        .map_err(|err| OpError::io(err, format!("cannot write {}", target.rel)))
}

/// `create_dir_all`, reporting every folder it creates to the hooks, deepest first.
#[cfg(not(unix))]
fn create_dirs(dir: &Path, hooks: &mut dyn WriteHooks) -> io::Result<()> {
    let missing: Vec<PathBuf> = dir
        .ancestors()
        .take_while(|ancestor| !ancestor.exists())
        .map(Path::to_path_buf)
        .collect();
    fs::create_dir_all(dir)?;
    for made in &missing {
        hooks.created(made);
    }
    Ok(())
}

/// Writes `bytes` to `path` atomically: a sibling temp file, flushed, handed to the hooks, renamed over
/// the target. The rename makes the write all-or-nothing; a failed rename removes the temp file.
#[cfg(not(unix))]
fn write_atomic(
    path: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
    hooks: &mut dyn WriteHooks,
) -> io::Result<()> {
    let (temp, mut file) = create_temp_file(path)?;
    let result = (|| {
        file.write_all(bytes)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        drop(file);
        hooks.created(&temp);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Creates a fresh sibling exclusively. Existing files, including symlinks, are never opened or
/// truncated. The counter keeps concurrent writers in this process from sharing a temporary file.
#[cfg(not(unix))]
fn create_temp_file(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for _ in 0..128 {
        let seq = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(".{name}.plaza-tmp-{}-{seq}", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a workspace temporary file",
    ))
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

struct Scan {
    needle: String,
    case_blind: bool,
    glob: Option<globset::GlobMatcher>,
    max: usize,
    matches: Vec<SearchMatch>,
    /// The reply's size so far, against [`OUTPUT_BUDGET`].
    used: usize,
    scanned: u64,
    truncated: bool,
}

fn walk_for_search(root: &Path, dir: &str, scan: &mut Scan) {
    let Ok(items) = fs::read_dir(root.join(native(dir))) else {
        return;
    };
    let mut names: Vec<String> = items
        .flatten()
        .filter_map(|item| item.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        if scan.truncated {
            return;
        }
        if name == PLAZA_DIR {
            continue;
        }
        let rel = if dir.is_empty() {
            name
        } else {
            format!("{dir}/{name}")
        };
        if is_secret(&rel) {
            continue;
        }
        let path = root.join(native(&rel));
        // Symlinks are neither followed nor searched.
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            walk_for_search(root, &rel, scan);
        } else if metadata.is_file() {
            scan_file(root, &rel, scan);
        }
    }
}

fn scan_file(root: &Path, rel: &str, scan: &mut Scan) {
    if scan.glob.as_ref().is_some_and(|glob| !glob.is_match(rel)) {
        return;
    }
    let path = root.join(native(rel));
    let Ok(metadata) = fs::metadata(&path) else {
        return;
    };
    if metadata.len() > MAX_FILE_BYTES {
        return;
    }
    if scan.scanned + metadata.len() > MAX_SEARCH_SCAN {
        scan.truncated = true;
        return;
    }
    let Ok(bytes) = read_capped(root, &native(rel)) else {
        return;
    };
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return;
    }
    if scan.scanned + bytes.len() as u64 > MAX_SEARCH_SCAN {
        scan.truncated = true;
        return;
    }
    scan.scanned += bytes.len() as u64;
    if bytes.contains(&0) {
        return;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return;
    };
    for (index, line) in text.lines().enumerate() {
        let hit = if scan.case_blind {
            line.to_lowercase().contains(&scan.needle)
        } else {
            line.contains(&scan.needle)
        };
        if !hit {
            continue;
        }
        if scan.matches.len() >= scan.max {
            scan.truncated = true;
            return;
        }
        let row = SearchMatch {
            line: index + 1,
            path: rel.to_string(),
            text: line.chars().take(SEARCH_LINE_CHARS).collect(),
        };
        let cost = serde_json::to_string(&row).map_or(0, |json| json.len()) + 1;
        if scan.used + cost > OUTPUT_BUDGET {
            scan.truncated = true;
            return;
        }
        scan.used += cost;
        scan.matches.push(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::CHUNK;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn scratch(name: &str) -> (PathBuf, Root) {
        let dir = std::env::temp_dir().join(format!("apz-ops-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch root");
        let root = Root::new(&dir);
        (dir, root)
    }

    #[cfg(unix)]
    fn denied<T: std::fmt::Debug>(result: Result<T, OpError>) -> String {
        match result.expect_err("the operation refuses") {
            OpError::Denied(reason) => reason,
            other => panic!("expected a denial, got {other}"),
        }
    }

    // The test makes symlinks with the unix call, so it runs on unix only.
    #[cfg(unix)]
    #[test]
    fn refused_paths_are_refused_by_the_rules_and_by_the_real_filesystem() {
        let (dir, root) = scratch("paths");
        let outside = scratch("paths-outside").0;
        fs::write(outside.join("secret.txt"), "outside\n").unwrap();
        fs::write(dir.join(".env"), "TOKEN=1\n").unwrap();
        fs::create_dir_all(dir.join(".plaza")).unwrap();
        fs::write(dir.join(".plaza").join("state"), "x\n").unwrap();
        symlink(&outside, dir.join("esc")).unwrap();
        symlink(outside.join("missing.txt"), dir.join("dangle")).unwrap();
        symlink(dir.join(".env"), dir.join("link.txt")).unwrap();

        for raw in ["../x", "/etc/passwd", "a/../../x"] {
            assert!(
                denied(read(&root, raw, None)).contains("refused"),
                "read {raw}"
            );
            assert!(
                denied(write(&root, raw, "x", None, &mut NoHooks)).contains("refused"),
                "write {raw}"
            );
        }
        assert!(denied(write(&root, ".plaza/state", "x", None, &mut NoHooks)).contains(".plaza"));
        assert!(denied(read(&root, ".env", None)).contains("secret"));
        assert!(denied(write(&root, ".env", "x", None, &mut NoHooks)).contains("secret"));
        assert!(denied(list(&root, ".ssh", None)).contains("secret"));
        assert!(denied(read(&root, "esc/secret.txt", None)).contains("outside the workspace"));
        assert!(denied(list(&root, "esc", None)).contains("outside the workspace"));
        assert!(denied(write(&root, "esc/new.txt", "x", None, &mut NoHooks)).contains("outside"));
        assert!(denied(write(&root, "dangle", "x", None, &mut NoHooks)).contains("symlink"));
        assert!(
            denied(read(&root, "link.txt", None)).contains("secret"),
            "a link to a secret is a secret"
        );
        assert!(!outside.join("new.txt").exists() && !outside.join("missing.txt").exists());
    }

    #[test]
    fn listing_pages_500_entries_with_directories_first_and_hides_secrets() {
        let (dir, root) = scratch("list");
        fs::create_dir_all(dir.join("d1")).unwrap();
        fs::create_dir_all(dir.join("d2")).unwrap();
        fs::write(dir.join(".env"), "x").unwrap();
        fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        for i in 0..1100 {
            fs::write(dir.join(format!("f{i:04}")), "x").unwrap();
        }
        let mut names = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = list(&root, "", cursor.as_deref()).expect("a page");
            pages += 1;
            assert!(page.entries.len() <= PAGE_SIZE);
            names.extend(page.entries.iter().map(|entry| entry.path.clone()));
            match page.next {
                None => break,
                Some(next) => cursor = Some(next),
            }
        }
        assert_eq!(pages, 3, "1103 entries in pages of 500");
        assert_eq!(names.len(), 1103);
        assert_eq!(&names[..2], ["d1", "d2"], "directories first");
        assert_eq!(names[2], "bin.dat");
        assert_eq!(names[3], "f0000");
        assert!(!names.iter().any(|n| n == ".env" || n == ".plaza"));
        let first = list(&root, "", None).expect("the first page");
        assert!(first.entries[2].binary);
        assert!(!first.entries[3].binary);
        assert!(first.entries[3].hash.is_none());
        assert!(
            list(&root, "bin.dat", None).is_err(),
            "a file is not a directory"
        );
        assert!(list(&root, "", Some("x")).is_err());
    }

    #[test]
    fn reads_are_chunked_on_character_boundaries_and_refuse_binary_and_huge_files() {
        let (dir, root) = scratch("read");
        let content = "é".repeat(60_000);
        fs::write(dir.join("u.txt"), &content).unwrap();
        let mut offset = 0usize;
        let mut collected = String::new();
        let mut chunks = 0;
        loop {
            let reply = read(&root, "u.txt", Some(offset as u64)).expect("a chunk");
            assert!(reply.text.len() <= CHUNK);
            assert_eq!(reply.hash, sha_hex(content.as_bytes()));
            assert_eq!(reply.size, content.len());
            collected.push_str(&reply.text);
            offset += reply.text.len();
            chunks += 1;
            if reply.done {
                break;
            }
        }
        assert!(chunks >= 3);
        assert_eq!(collected, content);
        assert!(read(&root, "u.txt", Some(1)).is_err(), "inside a character");
        assert!(
            read(&root, "u.txt", Some(1_000_000)).is_err(),
            "past the end"
        );

        fs::write(dir.join("bin.dat"), b"a\0b").unwrap();
        assert!(matches!(
            read(&root, "bin.dat", None),
            Err(OpError::InvalidInput(reason)) if reason.contains("binary")
        ));
        fs::File::create(dir.join("huge.txt"))
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read(&root, "huge.txt", None),
            Err(OpError::LimitExceeded(_))
        ));
    }

    /// A temp file reserved in `dir` for `name`, the way a write reserves one.
    #[cfg(unix)]
    fn reserve_temp(dir: &Path, name: &str) -> io::Result<(PathBuf, fs::File)> {
        let (folder, _) = make_confined_dirs(dir, Path::new(""))?;
        let (temp, file) = create_temp_file(&folder, OsStr::new(name))?;
        Ok((dir.join(temp), file))
    }

    #[cfg(not(unix))]
    fn reserve_temp(dir: &Path, name: &str) -> io::Result<(PathBuf, fs::File)> {
        create_temp_file(&dir.join(name))
    }

    #[test]
    fn concurrent_temporary_reservations_are_distinct_while_held_open() {
        let (dir, _) = scratch("temp-reservations");
        let target = dir.join("shared.txt");
        let start = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    reserve_temp(&dir, "shared.txt").expect("a distinct reservation")
                })
            })
            .collect();
        let mut reserved: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        let unique = reserved
            .iter()
            .map(|(path, _)| path)
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(
            unique, 4,
            "concurrent writers must not share a temporary file"
        );
        assert!(
            !target.exists(),
            "reserving does not publish the destination"
        );
        for (index, (path, file)) in reserved.iter_mut().enumerate() {
            file.write_all(index.to_string().as_bytes()).unwrap();
            assert_eq!(fs::read_to_string(path).unwrap(), index.to_string());
        }
        drop(reserved);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writes_check_the_hash_and_leave_nothing_behind_on_refusal() {
        let (dir, root) = scratch("write");
        let first = write(&root, "docs/a.txt", "one\n", None, &mut NoHooks).expect("created");
        assert_eq!(first.hash, sha_hex(b"one\n"));
        assert_eq!(first.checkpoint, None);
        assert_eq!(first.path, "docs/a.txt");
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "one\n");

        // A file that exists must not be created again.
        match write(&root, "docs/a.txt", "two\n", None, &mut NoHooks).expect_err("exists") {
            OpError::Conflict(reason) => assert_eq!(reason, sha_hex(b"one\n")),
            other => panic!("expected a conflict, got {other}"),
        }
        // A stale hash, and a missing file that a hash names.
        let stale = sha_hex(b"zero\n");
        let text = write(&root, "docs/a.txt", "two\n", Some(&stale), &mut NoHooks)
            .unwrap_err()
            .to_string();
        assert_eq!(text, format!("conflict: {}", sha_hex(b"one\n")));
        let text = write(&root, "docs/none.txt", "x", Some(&stale), &mut NoHooks)
            .unwrap_err()
            .to_string();
        assert_eq!(text, "conflict: missing");
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "one\n");

        let second = write(
            &root,
            "docs/a.txt",
            "two\n",
            Some(&sha_hex(b"one\n")),
            &mut NoHooks,
        )
        .expect("replaced");
        assert_eq!(second.hash, sha_hex(b"two\n"));
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "two\n");
        let names: Vec<String> = fs::read_dir(dir.join("docs"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.txt"], "the temp file is renamed away");

        let big = "x".repeat(MAX_FILE_BYTES as usize + 1);
        assert!(matches!(
            write(&root, "big.txt", &big, None, &mut NoHooks),
            Err(OpError::LimitExceeded(_))
        ));
    }

    #[derive(Default)]
    struct Recorder {
        before: Vec<String>,
        created: Vec<String>,
        checkpoint: Option<String>,
    }

    impl WriteHooks for Recorder {
        fn before_write(&mut self, path: &Path) -> Result<Option<String>, OpError> {
            self.before.push(path.to_string_lossy().into_owned());
            Ok(self.checkpoint.clone())
        }

        fn created(&mut self, path: &Path) {
            self.created
                .push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }

    #[test]
    fn a_write_runs_its_hooks_around_the_change_and_reports_the_checkpoint() {
        let (dir, root) = scratch("hooks");
        let mut hooks = Recorder {
            checkpoint: Some("cp-7".to_string()),
            ..Recorder::default()
        };
        let reply = write(&root, "deep/er/a.txt", "x\n", None, &mut hooks).expect("created");
        assert!(dir.join("deep/er/a.txt").exists());
        assert_eq!(reply.checkpoint.as_deref(), Some("cp-7"));
        assert_eq!(hooks.before, ["deep/er/a.txt"]);
        // The folders made, deepest first, then the temp file before its rename.
        assert_eq!(hooks.created.len(), 3, "{:?}", hooks.created);
        assert_eq!(&hooks.created[..2], ["er", "deep"]);
        assert!(
            hooks.created[2].starts_with(".a.txt.plaza-tmp-"),
            "{:?}",
            hooks.created
        );

        // A refused write runs no hook.
        let refused = write(&root, "deep/er/a.txt", "y", None, &mut hooks);
        assert!(matches!(refused, Err(OpError::Conflict(_))));
        assert_eq!(hooks.before.len(), 1, "a refused write takes no checkpoint");
    }

    #[test]
    fn writes_refuse_an_oversized_existing_file_before_hashing_or_hooks() {
        let (dir, root) = scratch("write-existing-cap");
        let path = dir.join("large.txt");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        for expected_hash in [None, Some("stale-hash")] {
            let mut hooks = Recorder::default();
            assert!(matches!(
                write(
                    &root,
                    "large.txt",
                    "small replacement",
                    expected_hash,
                    &mut hooks
                ),
                Err(OpError::LimitExceeded(_))
            ));
            assert!(
                hooks.before.is_empty(),
                "refusal must precede the checkpoint"
            );
            assert!(
                hooks.created.is_empty(),
                "refusal must create no temporary file"
            );
            assert_eq!(fs::metadata(&path).unwrap().len(), MAX_FILE_BYTES + 1);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn search_is_bounded_and_skips_binary_secret_and_plaza_files() {
        let (dir, root) = scratch("search");
        let lines: String = (0..250).map(|i| format!("needle line {i}\n")).collect();
        fs::write(dir.join("many.txt"), lines).unwrap();
        fs::write(dir.join("case.txt"), "NEEDLE upper\nneedle lower\n").unwrap();
        fs::write(dir.join("bin.dat"), b"needle\0bin").unwrap();
        fs::write(dir.join(".env"), "needle secret\n").unwrap();
        fs::create_dir_all(dir.join(".plaza")).unwrap();
        fs::write(dir.join(".plaza/state"), "needle internal\n").unwrap();
        fs::write(dir.join("long.md"), format!("{}\n", "needle ".repeat(200))).unwrap();

        let capped = search(&root, "needle", Some("many.txt"), None).expect("a search");
        assert_eq!(capped.matches.len(), MAX_SEARCH_MATCHES);
        assert!(capped.truncated);
        assert_eq!(capped.matches[0].line, 1);

        let blind = search(&root, "needle", Some("case.txt"), None).expect("a search");
        assert_eq!(blind.matches.len(), 2, "lowercase q is case-insensitive");
        let exact = search(&root, "NEEDLE", Some("case.txt"), None).expect("a search");
        assert_eq!(exact.matches.len(), 1, "uppercase q is exact");

        let everything = search(&root, "needle", None, Some(200)).expect("a search");
        let paths: Vec<&str> = everything.matches.iter().map(|m| m.path.as_str()).collect();
        assert!(
            !paths
                .iter()
                .any(|p| p.contains("bin") || p.contains(".env") || p.contains(".plaza")),
            "{paths:?}"
        );

        let long = search(&root, "needle", Some("long.md"), Some(1)).expect("a search");
        assert!(long.matches[0].text.chars().count() <= SEARCH_LINE_CHARS);
        assert!(search(&root, "needle", None, Some(201)).is_err());
        assert!(search(&root, "needle", None, Some(0)).is_err());
        assert!(search(&root, "", None, None).is_err());
    }

    // The test makes a FIFO with mkfifo, so it runs on unix only. A read that opened the FIFO would wait for a
    // writer, so the read runs on a thread and the test waits on it with a bound.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_at_once_and_never_opened() {
        let (dir, root) = scratch("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(dir.join("pipe"))
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "mkfifo made the FIFO");
        let (done, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(read(&root, "pipe", None).map(|_| ()));
        });
        let reply = answer
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the read is refused without waiting for a writer");
        match reply {
            Err(OpError::InvalidInput(reason)) => {
                assert!(reason.contains("not a regular file"), "{reason}");
            }
            Err(other) => panic!("expected a refusal, got {other}"),
            Ok(()) => panic!("a FIFO was read"),
        }
    }

    // The test makes a FIFO with mkfifo, so it runs on unix only. A write that opened the FIFO would wait for a
    // reader, so the write runs on a thread and the test waits on it with a bound.
    #[cfg(unix)]
    #[test]
    fn a_write_refuses_a_fifo_without_opening_it() {
        let (dir, root) = scratch("write-fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(dir.join("pipe"))
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "mkfifo made the FIFO");
        let (done, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(write(&root, "pipe", "x", None, &mut NoHooks).map(|_| ()));
        });
        let reply = answer
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the write is refused without waiting for a reader");
        match reply {
            Err(OpError::InvalidInput(reason)) => {
                assert!(reason.contains("not a regular file"), "{reason}");
            }
            Err(other) => panic!("expected a refusal, got {other}"),
            Ok(()) => panic!("a FIFO was written"),
        }
    }

    #[test]
    fn an_existing_file_at_the_cap_is_overwritten_and_one_byte_over_is_refused() {
        let (dir, root) = scratch("write-existing-cap-edge");
        let exact = vec![b'x'; MAX_FILE_BYTES as usize];
        fs::write(dir.join("exact.txt"), &exact).unwrap();
        let reply = write(
            &root,
            "exact.txt",
            "small\n",
            Some(&sha_hex(&exact)),
            &mut NoHooks,
        )
        .expect("a file at the cap is overwritten");
        assert_eq!(reply.hash, sha_hex(b"small\n"));
        assert_eq!(
            fs::read_to_string(dir.join("exact.txt")).unwrap(),
            "small\n"
        );

        fs::write(
            dir.join("over.txt"),
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
        )
        .unwrap();
        match write(&root, "over.txt", "small\n", None, &mut NoHooks)
            .expect_err("a file one byte over the cap is refused")
        {
            OpError::LimitExceeded(reason) => assert_eq!(
                reason,
                format!(
                    "over.txt is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
                    MAX_FILE_BYTES + 1
                )
            ),
            other => panic!("expected the cap, got {other}"),
        }
        assert_eq!(
            fs::metadata(dir.join("over.txt")).unwrap().len(),
            MAX_FILE_BYTES + 1
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// What a search charges a match in `f000.txt` whose line is `line`: its JSON, and one byte more.
    fn search_charge(line: &str) -> usize {
        let row = SearchMatch {
            line: 1,
            path: "f000.txt".to_string(),
            text: line.to_string(),
        };
        serde_json::to_string(&row)
            .expect("a match serializes")
            .len()
            + 1
    }

    /// A line of `needle` and filler, charged `extra` bytes more than the bare word. A quote is two bytes of
    /// JSON, so the filler adds up to 388 bytes: 194 characters, all quotes.
    fn filler_line(extra: usize) -> String {
        let (plain, quotes) = if extra <= 194 {
            (extra, 0)
        } else {
            (194, extra - 194)
        };
        format!(
            "needle{}{}",
            "x".repeat(plain - quotes),
            "\"".repeat(quotes)
        )
    }

    #[test]
    fn a_search_reply_fills_the_output_budget_exactly_and_no_row_past_it() {
        let (dir, root) = scratch("search-budget-exact");
        let bare = search_charge("needle");
        let full = search_charge(&filler_line(388));
        // A reply starts at 64 bytes. Full lines fill most of the budget, then two lines whose charges sum to
        // what is left, so the reply lands exactly on the budget, and a last bare line that does not fit.
        let spare = OUTPUT_BUDGET - 64;
        let full_rows = (spare - 2 * bare) / full;
        let rest = spare - full_rows * full - 2 * bare;
        let first = rest.min(388);
        let second = rest - first;
        let mut lines = vec![filler_line(388); full_rows];
        lines.extend([filler_line(first), filler_line(second), filler_line(0)]);
        assert!(
            lines.len() < MAX_SEARCH_MATCHES,
            "the budget must end the reply, not the match cap"
        );
        for (index, line) in lines.iter().enumerate() {
            fs::write(dir.join(format!("f{index:03}.txt")), format!("{line}\n")).unwrap();
        }

        let reply = search(&root, "needle", None, None).expect("a search");
        assert!(reply.truncated);
        assert_eq!(
            reply.matches.len(),
            full_rows + 2,
            "the second line ends exactly at the budget and the last one does not fit"
        );
        let charged: usize = 64
            + reply
                .matches
                .iter()
                .map(|row| {
                    serde_json::to_string(row)
                        .expect("a match serializes")
                        .len()
                        + 1
                })
                .sum::<usize>();
        assert_eq!(charged, OUTPUT_BUDGET);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_at_the_size_cap_is_read_and_written_and_one_byte_over_is_refused() {
        let (dir, root) = scratch("size-cap");
        fs::write(dir.join("exact.txt"), vec![b'x'; MAX_FILE_BYTES as usize]).unwrap();
        let reply = read(&root, "exact.txt", None).expect("a file at the cap reads");
        assert_eq!(reply.size as u64, MAX_FILE_BYTES);

        let text = "x".repeat(MAX_FILE_BYTES as usize);
        write(&root, "fill.txt", &text, None, &mut NoHooks).expect("a text at the cap writes");
        assert_eq!(
            fs::metadata(dir.join("fill.txt")).unwrap().len(),
            MAX_FILE_BYTES
        );
        let over = format!("{text}x");
        match write(&root, "over.txt", &over, None, &mut NoHooks)
            .expect_err("one byte over the cap is refused")
        {
            OpError::LimitExceeded(reason) => assert_eq!(
                reason,
                format!(
                    "the text is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
                    MAX_FILE_BYTES + 1
                )
            ),
            other => panic!("expected the cap, got {other}"),
        }
        assert!(
            !dir.join("over.txt").exists(),
            "a refused text writes nothing"
        );

        fs::File::create(dir.join("huge.txt"))
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        match read(&root, "huge.txt", None).expect_err("one byte over the cap is refused") {
            OpError::LimitExceeded(reason) => assert_eq!(
                reason,
                format!(
                    "huge.txt is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
                    MAX_FILE_BYTES + 1
                )
            ),
            other => panic!("expected the cap, got {other}"),
        }
    }

    #[test]
    fn reads_and_writes_refuse_a_directory_with_their_own_reason() {
        let (dir, root) = scratch("directories");
        fs::create_dir_all(dir.join("docs")).unwrap();
        match write(&root, "docs", "x", None, &mut NoHooks).expect_err("a directory is not written")
        {
            OpError::InvalidInput(reason) => {
                assert_eq!(reason, "docs is a directory; a write takes one file")
            }
            other => panic!("expected invalid input, got {other}"),
        }
        match read(&root, "docs", None).expect_err("a directory is not read") {
            OpError::InvalidInput(reason) => {
                assert_eq!(reason, "docs is a directory; a read takes one file")
            }
            other => panic!("expected invalid input, got {other}"),
        }
    }

    #[test]
    fn an_offset_at_the_end_is_an_empty_last_chunk_and_past_it_is_refused() {
        let (dir, root) = scratch("offsets");
        fs::write(dir.join("u.txt"), "abc").unwrap();
        let end = read(&root, "u.txt", Some(3)).expect("the end is a valid offset");
        assert!(end.done && end.text.is_empty());
        match read(&root, "u.txt", Some(4)).expect_err("past the end is refused") {
            OpError::InvalidInput(reason) => {
                assert_eq!(reason, "offset 4 is past the end of u.txt (3 bytes)")
            }
            other => panic!("expected invalid input, got {other}"),
        }
    }

    #[test]
    fn a_listing_sniffs_only_the_first_8_kib_for_a_nul() {
        let (dir, root) = scratch("probe");
        let with_nul_at = |index: usize| {
            let mut bytes = vec![b'x'; index];
            bytes.push(0);
            bytes
        };
        fs::write(dir.join("mid.dat"), with_nul_at(4096)).unwrap();
        fs::write(dir.join("last.dat"), with_nul_at(8191)).unwrap();
        fs::write(dir.join("past.dat"), with_nul_at(8192)).unwrap();
        let page = list(&root, "", None).expect("a page");
        let binary = |name: &str| {
            page.entries
                .iter()
                .find(|entry| entry.path == name)
                .expect("listed")
                .binary
        };
        assert!(binary("mid.dat"));
        assert!(binary("last.dat"));
        assert!(!binary("past.dat"), "a NUL past 8 KiB is not sniffed");
    }

    /// What `list` charges one entry against a page: its JSON, its name, and 16.
    fn entry_cost(entry: &Entry) -> usize {
        serde_json::to_string(entry)
            .expect("an entry serializes")
            .len()
            + entry.path.len()
            + 16
    }

    #[test]
    fn a_listing_page_ends_at_the_output_budget_and_the_pages_cover_every_name() {
        let (dir, root) = scratch("list-budget");
        let mut expected = Vec::new();
        for i in 0..600 {
            let name = format!("{}{i:04}", "n".repeat(240));
            fs::write(dir.join(&name), "x").unwrap();
            expected.push(name);
        }
        let mut names = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = list(&root, "", cursor.as_deref()).expect("a page");
            assert!(page.entries.len() < PAGE_SIZE, "the budget ends the page");
            let used = 128 + page.entries.iter().map(entry_cost).sum::<usize>();
            assert!(
                used <= OUTPUT_BUDGET,
                "a page of {used} bytes is over the budget"
            );
            names.extend(page.entries.iter().map(|entry| entry.path.clone()));
            let Some(next) = page.next else { break };
            let following = list(&root, "", Some(next.as_str())).expect("the next page");
            assert!(
                used + entry_cost(&following.entries[0]) > OUTPUT_BUDGET,
                "the next entry would have fit on this page"
            );
            cursor = Some(next);
        }
        assert_eq!(names, expected);
    }

    #[test]
    fn a_cursor_names_one_entry_with_no_slash() {
        let (_dir, root) = scratch("cursors");
        for bad in ["0", "1", "1a/b"] {
            match list(&root, "", Some(bad)) {
                Err(OpError::InvalidInput(reason)) => {
                    assert_eq!(reason, "\"cursor\" is not a cursor from ide_list")
                }
                Err(other) => panic!("expected invalid input for {bad:?}, got {other}"),
                Ok(_) => panic!("{bad:?} is not a cursor"),
            }
        }
    }

    #[test]
    fn read_capped_reads_at_most_one_byte_past_the_cap() {
        let (dir, _root) = scratch("read-capped");
        let big = dir.join("big.bin");
        fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 5)
            .unwrap();
        assert_eq!(
            read_capped(&dir, Path::new("big.bin"))
                .expect("bytes")
                .len() as u64,
            MAX_FILE_BYTES + 1
        );
        fs::write(dir.join("small.txt"), "abc").unwrap();
        assert_eq!(
            read_capped(&dir, Path::new("small.txt")).expect("bytes"),
            b"abc"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_root_is_refused() {
        let (dir, root) = scratch("confine-out");
        let outside =
            std::env::temp_dir().join(format!("apz-ops-confine-out-{}", std::process::id()));
        fs::write(&outside, "outside").expect("an outside file");
        symlink(&outside, dir.join("link.txt")).expect("a link");
        assert!(read(&root, "link.txt", None).is_err());
        assert!(crate::root::open_confined(&dir, Path::new("link.txt")).is_err());
        let _ = fs::remove_file(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_through_a_folder_out_of_the_root_is_refused() {
        let (dir, root) = scratch("confine-folder");
        let outside =
            std::env::temp_dir().join(format!("apz-ops-confine-folder-{}", std::process::id()));
        fs::create_dir_all(&outside).expect("an outside folder");
        fs::write(outside.join("note.txt"), "outside").expect("an outside file");
        symlink(&outside, dir.join("linked")).expect("a folder link");
        assert!(read(&root, "linked/note.txt", None).is_err());
        assert!(crate::root::open_confined(&dir, Path::new("linked/note.txt")).is_err());
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn a_write_through_a_folder_link_out_of_the_root_is_refused_and_leaves_nothing_outside() {
        let (dir, root) = scratch("write-folder-link");
        let outside =
            std::env::temp_dir().join(format!("apz-ops-write-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).expect("an outside folder");
        symlink(&outside, dir.join("linked")).expect("a folder link");

        assert!(write(&root, "linked/new.txt", "x", None, &mut NoHooks).is_err());
        assert!(write(&root, "linked/deep/new.txt", "x", None, &mut NoHooks).is_err());
        // The handle path refuses the same folder when it is asked directly, and makes nothing outside.
        assert!(make_confined_dirs(&dir, Path::new("linked/deep")).is_err());
        assert!(crate::root::open_confined_dir(&dir, Path::new("linked")).is_err());
        assert_eq!(
            fs::read_dir(&outside).expect("the outside folder").count(),
            0,
            "nothing appears outside"
        );

        write(&root, "real/ok.txt", "fine\n", None, &mut NoHooks).expect("a normal write works");
        assert_eq!(
            fs::read_to_string(dir.join("real/ok.txt")).unwrap(),
            "fine\n"
        );
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn a_regular_file_and_a_link_inside_the_root_still_read() {
        let (dir, root) = scratch("confine-inside");
        fs::write(dir.join("plain.txt"), "plain").expect("a file");
        symlink(dir.join("plain.txt"), dir.join("alias.txt")).expect("a link");
        assert_eq!(
            read(&root, "plain.txt", None).expect("the read").text,
            "plain"
        );
        assert_eq!(
            read(&root, "alias.txt", None).expect("the read").text,
            "plain"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_without_waiting_for_a_writer() {
        let (dir, root) = scratch("confine-fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(dir.join("pipe"))
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "mkfifo made the FIFO");
        assert!(read(&root, "pipe", None).is_err());
        assert!(crate::root::open_confined(&dir, Path::new("pipe")).is_err());
    }

    #[test]
    fn a_needle_may_be_the_cap_and_one_byte_more_is_refused() {
        let (_dir, root) = scratch("needle-cap");
        let at_cap = "a".repeat(MAX_NEEDLE_BYTES);
        assert!(
            search(&root, &at_cap, None, None)
                .expect("a needle at the cap")
                .matches
                .is_empty()
        );
        let over = "a".repeat(MAX_NEEDLE_BYTES + 1);
        match search(&root, &over, None, None).expect_err("one byte over the cap is refused") {
            OpError::LimitExceeded(reason) => assert_eq!(
                reason,
                format!(
                    "\"q\" is {} bytes, over the {MAX_NEEDLE_BYTES}-byte cap",
                    MAX_NEEDLE_BYTES + 1
                )
            ),
            other => panic!("expected the cap, got {other}"),
        }
    }

    #[test]
    fn a_search_stops_at_the_scan_cap_and_counts_a_file_that_ends_on_it() {
        let (dir, root) = scratch("scan-cap");
        // Nineteen zero-filled files of the largest size: binary, so skipped, but their bytes count.
        for i in 0..19 {
            fs::File::create(dir.join(format!("a{i:02}.bin")))
                .unwrap()
                .set_len(MAX_FILE_BYTES)
                .unwrap();
        }
        // The twentieth is text that ends the scan exactly on the cap, and it holds a match.
        let mut text = b"needle\n".to_vec();
        text.resize(MAX_FILE_BYTES as usize, b'x');
        fs::write(dir.join("a19.txt"), &text).unwrap();
        fs::write(dir.join("b.txt"), "needle\n").unwrap();

        let reply = search(&root, "needle", None, None).expect("a search");
        assert!(reply.truncated, "the file after the cap is not scanned");
        assert_eq!(reply.matches.len(), 1);
        assert_eq!(reply.matches[0].path, "a19.txt");

        fs::remove_file(dir.join("b.txt")).unwrap();
        let exact = search(&root, "needle", None, None).expect("a search");
        assert!(
            !exact.truncated,
            "a scan that ends on the cap is not cut short"
        );
        assert_eq!(exact.matches.len(), 1);
    }

    #[test]
    fn a_path_that_is_empty_or_holds_a_nul_or_a_control_is_invalid_input() {
        let (_dir, root) = scratch("malformed");
        let cases = [
            ("", apz_ide::path::PathError::Empty.to_string()),
            ("a\0b", apz_ide::path::PathError::Nul.to_string()),
            ("a\u{1}b", apz_ide::path::PathError::Control.to_string()),
        ];
        for (raw, rule) in cases {
            match read(&root, raw, None) {
                Err(OpError::InvalidInput(reason)) => {
                    assert!(reason.ends_with(&rule), "{reason}")
                }
                Err(other) => panic!("expected invalid input for {raw:?}, got {other}"),
                Ok(_) => panic!("{raw:?} was read"),
            }
        }
    }

    #[test]
    fn a_path_under_a_file_is_a_resolve_error_not_a_missing_file() {
        let (dir, root) = scratch("under-file");
        fs::write(dir.join("plain.txt"), "x").unwrap();
        match read(&root, "plain.txt/child", None) {
            Err(err) => assert!(err.to_string().contains("cannot resolve"), "{err}"),
            Ok(_) => panic!("a path under a file is read"),
        }
    }

    /// The entry `list` makes for a plain file called `path` of `size` bytes. It is not binary.
    fn plain_file(path: &str, size: u64) -> Entry {
        Entry {
            path: path.to_string(),
            kind: EntryKind::File,
            size,
            hash: None,
            binary: false,
        }
    }

    /// The name of a tuning file: a `z` and `len - 1` more bytes. It sorts after the uniform names.
    fn tuning_name(len: usize) -> String {
        format!("z{}", "x".repeat(len - 1))
    }

    #[test]
    fn a_listing_that_lands_exactly_on_the_budget_keeps_every_entry() {
        let (dir, root) = scratch("list-exact");
        // Uniform names make entries of one cost, so `count` of them fit with `rest` of the room left. A
        // tuning file then costs exactly `rest`, the page lands on the budget, and the name after it does not fit.
        let uniform = |i: usize| format!("{i:0>50}");
        let uniform_cost = entry_cost(&plain_file(&uniform(0), 1));
        let least = entry_cost(&plain_file(&tuning_name(1), 1));
        let room = OUTPUT_BUDGET - 128;
        let count = (room - least) / uniform_cost;
        let rest = room - count * uniform_cost;
        // A tuning name of `len` bytes, and a size of 1 or 10 bytes (one more digit), can cost any of `rest`.
        let (len, size) = (1..=200)
            .flat_map(|len| [1u64, 10].map(|size| (len, size)))
            .find(|&(len, size)| entry_cost(&plain_file(&tuning_name(len), size)) == rest)
            .expect("a tuning entry that costs the rest exactly");
        for i in 0..count {
            fs::write(dir.join(uniform(i)), "x").unwrap();
        }
        let tuning = tuning_name(len);
        fs::write(dir.join(&tuning), "x".repeat(size as usize)).unwrap();
        fs::write(dir.join("zzz"), "x").unwrap();

        let first = list(&root, "", None).expect("the first page");
        let used = 128 + first.entries.iter().map(entry_cost).sum::<usize>();
        assert_eq!(used, OUTPUT_BUDGET, "the page lands exactly on the budget");
        assert_eq!(
            first.entries.len(),
            count + 1,
            "the entry that lands on the budget stays"
        );
        assert_eq!(
            first.entries.last().map(|entry| entry.path.as_str()),
            Some(tuning.as_str())
        );
        let next = first
            .next
            .expect("the entry past the budget starts the next page");
        let second = list(&root, "", Some(next.as_str())).expect("the next page");
        let names: Vec<&str> = second
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(names, ["zzz"]);
        assert!(second.next.is_none());
    }

    #[test]
    fn a_paged_read_at_exactly_the_budget_is_one_page_and_one_byte_more_is_two() {
        let (dir, root) = scratch("read-budget");
        // The JSON of the reply for `text` as one whole page, as `read` serializes it.
        let reply_json = |text: &str| {
            serde_json::to_string(&ReadReply {
                done: true,
                hash: sha_hex(text.as_bytes()),
                offset: 0,
                path: "exact.txt".to_string(),
                size: text.len(),
                text: text.to_string(),
            })
            .expect("a reply serializes")
            .len()
        };
        // `len` bytes of text, `quotes` of them double quotes. A quote is escaped in the JSON: one byte more.
        let body = |len: usize, quotes: usize| {
            format!("{}{}", "x".repeat(len - quotes), "\"".repeat(quotes))
        };
        // The text is one chunk, so the reply is limited by the budget alone.
        let len = CHUNK.min(OUTPUT_BUDGET - 200);
        let plain = reply_json(&body(len, 0));
        assert!(
            plain <= OUTPUT_BUDGET && OUTPUT_BUDGET - plain <= len,
            "the budget is reachable in one chunk"
        );
        let exact = body(len, OUTPUT_BUDGET - plain);
        assert_eq!(reply_json(&exact), OUTPUT_BUDGET);

        fs::write(dir.join("exact.txt"), &exact).unwrap();
        let whole = read(&root, "exact.txt", None).expect("one page");
        assert!(whole.done, "a reply exactly on the budget is one page");
        assert_eq!(whole.text, exact);

        // One more byte is over the budget, so the read is two pages that join back to the file.
        let more = format!("{exact}x");
        fs::write(dir.join("exact.txt"), &more).unwrap();
        let first = read(&root, "exact.txt", None).expect("the first page");
        assert!(!first.done, "one byte over the budget is not one page");
        assert!(serde_json::to_string(&first).unwrap().len() <= OUTPUT_BUDGET);
        let second = read(&root, "exact.txt", Some(first.text.len() as u64)).expect("the rest");
        assert!(second.done);
        assert_eq!(format!("{}{}", first.text, second.text), more);
    }
}
