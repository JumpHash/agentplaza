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

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use apz_ide::path::{check_dir, check_read, check_write, is_secret};
use apz_ide::tree::{Entry, EntryKind, PAGE_SIZE, TreePage};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::OpError;
use crate::page::{OUTPUT_BUDGET, chunk_end};
use crate::root::{Root, locate, native, shown};

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
        let binary = item.kind == EntryKind::File && sniff_binary(&target.abs.join(&item.name));
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
    let bytes = read_capped(&target.abs)
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
        Ok(_) => Some(
            fs::read(&target.abs)
                .map_err(|err| OpError::io(err, format!("cannot read {}", target.rel)))?,
        ),
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
    if let Some(parent) = target.abs.parent() {
        create_dirs(parent, hooks).map_err(|err| {
            OpError::io(err, format!("cannot create the folders of {}", target.rel))
        })?;
    }
    write_atomic(&target.abs, text.as_bytes(), hooks)
        .map_err(|err| OpError::io(err, format!("cannot write {}", target.rel)))?;
    if let Some(permissions) = permissions {
        fs::set_permissions(&target.abs, permissions)
            .map_err(|err| OpError::io(err, format!("cannot keep the mode of {}", target.rel)))?;
    }
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

/// The bytes of a file, read at most one byte past the cap so an over-cap file is seen.
fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
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

/// Whether the first [`BINARY_PROBE`] bytes of a file hold a NUL.
fn sniff_binary(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut head = Vec::new();
    if file.take(BINARY_PROBE).read_to_end(&mut head).is_err() {
        return false;
    }
    head.contains(&0)
}

/// `create_dir_all`, reporting every folder it creates to the hooks, deepest first.
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
fn write_atomic(path: &Path, bytes: &[u8], hooks: &mut dyn WriteHooks) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = dir.join(format!(
        ".{}.plaza-tmp-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    hooks.created(&temp);
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
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
            scan_file(&path, &rel, scan);
        }
    }
}

fn scan_file(path: &Path, rel: &str, scan: &mut Scan) {
    if scan.glob.as_ref().is_some_and(|glob| !glob.is_match(rel)) {
        return;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() > MAX_FILE_BYTES {
        return;
    }
    if scan.scanned + metadata.len() > MAX_SEARCH_SCAN {
        scan.truncated = true;
        return;
    }
    let Ok(bytes) = fs::read(path) else {
        return;
    };
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
}
