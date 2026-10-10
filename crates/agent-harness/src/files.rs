//! The file tools: the agent's hands on the writable `/workspace` volume.
//!
//! The guest's init mounts the plaza's writable volume at [`WORKSPACE_ROOT`]
//! (`/workspace`) before it starts the harness, and nothing else in the guest
//! is writable, so this volume is where an agent's work lives. These tools are
//! plain `std::fs` calls in the harness process — there is no shell and no
//! subprocess behind them — and the hub drives them: a tool arrives as a
//! `tool_call` and leaves as a `tool_result` (see [`crate::keepalive`]).
//!
//! # Path discipline
//!
//! The hub drives these tools, but neither the path nor the content is trusted
//! to stay in the sandbox: the volume is the boundary, and every tool re-proves
//! it. A path is refused, with the reason, when it is
//!
//! * absolute (`/etc/passwd`),
//! * carries a `..` component (`../x`, `a/../../x`), or
//! * is a symlink — or passes through one — whose *resolved* path lands
//!   outside the workspace.
//!
//! The escape check is on the resolved path, not the spelling: before any
//! read, write or rename, the deepest existing ancestor of the target is
//! canonicalized (which follows every link on the way) and must lie inside the
//! canonical workspace root. `search` additionally never follows a symlink at
//! all, so a link cannot walk the scan out of the volume. (This is not a
//! defence against another process racing the workspace's own contents between
//! the check and the call: the tools are the only writers of this volume.)
//!
//! # Shapes and caps
//!
//! A tool's *output* is one JSON object, as a string, exactly as
//! [`crate::tools::serve`] returns it; the wire carries it as
//! `tool_result.output`. Each tool's doc comment names its fields. Nothing is
//! silently clamped: a request over a cap is refused with
//! [`ToolError::LimitExceeded`], and a *result* cut short by a bound carries
//! `"truncated": true` rather than looking complete.
//!
//! The caps, in one place:
//!
//! | cap | value | what it bounds |
//! |-----|-------|----------------|
//! | [`READ_DEFAULT_BYTES`] | 64 KiB | `read_file` without `max_bytes` |
//! | [`MAX_FILE_BYTES`] | 1 MiB | any file the tools will read or grow |
//! | [`MAX_WRITE_BYTES`] | 256 KiB | one `write_file`/`append_file`/`edit_file` payload |
//! | [`MAX_NEEDLE_BYTES`] | 4 KiB | a `search` pattern or `edit_file` needle |
//! | [`MAX_LIST_ENTRIES`] | 1024 | entries returned by `list` |
//! | [`MAX_LIST_SCAN_ENTRIES`] | 65536 | entries `list` reads before stopping |
//! | [`MAX_SEARCH_MATCHES`] | 256 | matches returned by `search` |
//! | [`MAX_SEARCH_FILES`] | 4096 | files one `search` opens |
//! | [`MAX_SEARCH_SCAN_BYTES`] | 8 MiB | bytes one `search` reads |
//! | [`MAX_SEARCH_DEPTH`] | 16 | levels `search` descends |
//! | [`MAX_SEARCH_LINE_BYTES`] | 256 | bytes of a matching line `search` returns |

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::tools::ToolError;

/// Where the guest's init mounts the writable volume.
pub const WORKSPACE_ROOT: &str = "/workspace";

/// The most `read_file` returns when it is not told how much: 64 KiB.
pub const READ_DEFAULT_BYTES: u64 = 64 * 1024;

/// The largest file any file tool will act on: 1 MiB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The largest single `write_file`/`append_file` payload, and the largest
/// `edit_file` replacement: 256 KiB.
pub const MAX_WRITE_BYTES: u64 = 256 * 1024;

/// The largest `search` pattern, or `edit_file` needle: 4 KiB.
///
/// A needle is searched for over every byte of every file the tools will act
/// on, so an unbounded one would let a caller ask for quadratic work; 4 KiB is
/// far more than any path, identifier or line fragment needs.
pub const MAX_NEEDLE_BYTES: usize = 4 * 1024;

/// The most entries `list` returns.
pub const MAX_LIST_ENTRIES: usize = 1024;

/// The most entries `list` reads before it stops and reports the listing as
/// incomplete. A 16 MiB volume does not hold this many entries in practice, so
/// a listing below the cap is complete *and* in readdir order.
pub const MAX_LIST_SCAN_ENTRIES: usize = 64 * 1024;

/// The most matches `search` returns.
pub const MAX_SEARCH_MATCHES: usize = 256;

/// The most files one `search` opens.
pub const MAX_SEARCH_FILES: usize = 4096;

/// The most bytes one `search` reads in total.
pub const MAX_SEARCH_SCAN_BYTES: u64 = 8 * 1024 * 1024;

/// How deep `search` descends below the path it was given.
pub const MAX_SEARCH_DEPTH: usize = 16;

/// How much of one matching line `search` returns.
pub const MAX_SEARCH_LINE_BYTES: usize = 256;

/// The volume the file tools work inside.
///
/// The default is the guest's: [`WORKSPACE_ROOT`], where the guest's init
/// mounts the plaza's volume. Tests point it at a scratch directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::at(WORKSPACE_ROOT)
    }
}

impl Workspace {
    /// The workspace rooted at `root`.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The root this workspace was configured with, as given.
    ///
    /// The tools work from the *canonical* root ([`Workspace::canonical_root`]);
    /// this is the raw path, which the tests address a scratch directory by.
    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical workspace root, or `Unavailable` when it is not mounted.
    ///
    /// Canonicalizing once and working from the result is what makes the
    /// escape check below meaningful: the comparison is between two resolved
    /// paths, so a root that is itself a symlink cannot make the check say
    /// "inside" for something that is not.
    fn canonical_root(&self) -> Result<PathBuf, ToolError> {
        fs::canonicalize(&self.root).map_err(|err| ToolError::Unavailable {
            reason: format!(
                "the workspace {} is not mounted: {err}",
                self.root.display()
            ),
        })
    }

    /// The path `raw` names inside the workspace, with the normalized relative
    /// label to report it by.
    ///
    /// Refuses an absolute path, a `..` component and a symlink that resolves
    /// outside the workspace (see the module docs). The returned path is the
    /// joined one — inside the workspace by construction — and the label is
    /// always relative, so no absolute (host or guest) path crosses the wire.
    fn resolve(&self, raw: &str) -> Result<(PathBuf, String), ToolError> {
        let parts = relative_parts(raw)?;
        let root = self.canonical_root()?;
        let mut path = root.clone();
        for part in &parts {
            path.push(part);
        }

        // The deepest existing ancestor of the target, canonicalized. Walking
        // up is what covers a target that does not exist yet: its nearest
        // existing directory still resolves, and a link anywhere on the way is
        // followed, so the answer is where the call would really land.
        let mut ancestor: &Path = &path;
        let landed = loop {
            match fs::canonicalize(ancestor) {
                Ok(resolved) => break resolved,
                Err(err) if err.kind() == io::ErrorKind::NotFound => match ancestor.parent() {
                    Some(parent) => ancestor = parent,
                    None => {
                        return Err(denied(format!("path {raw:?} cannot be resolved")));
                    }
                },
                Err(err) => {
                    return Err(io_tool_error(err, format!("cannot resolve {raw:?}")));
                }
            }
        };
        if !landed.starts_with(&root) {
            return Err(denied(format!(
                "path {raw:?} resolves to {} outside the workspace",
                landed.display()
            )));
        }
        Ok((path, label_of(&parts)))
    }
}

/// The components of a tool-supplied path, relative to the workspace root.
///
/// Refuses an absolute path, a Windows-style prefix and a `..` component;
/// `.` is dropped. An empty result is the workspace root itself, which only
/// the directory tools (`list`, `search`) accept.
fn relative_parts(raw: &str) -> Result<Vec<String>, ToolError> {
    let mut parts = Vec::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(part) => match part.to_str() {
                Some(part) => parts.push(part.to_string()),
                None => return Err(invalid(format!("path {raw:?} is not valid UTF-8"))),
            },
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(denied(format!(
                    "path {raw:?} is absolute; every path is inside the workspace"
                )));
            }
            Component::ParentDir => {
                return Err(denied(format!(
                    "path {raw:?} contains \"..\"; every path is inside the workspace"
                )));
            }
        }
    }
    Ok(parts)
}

/// How a resolved path is named in a result: relative, never absolute.
fn label_of(parts: &[String]) -> String {
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// A required string argument.
fn string_arg(args: &Value, key: &str) -> Result<String, ToolError> {
    match args.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(invalid(format!("{key:?} must be a string"))),
        None => Err(invalid(format!("{key:?} is required"))),
    }
}

/// The optional `path` argument, defaulting to the workspace root.
///
/// The default is the empty relative path, which resolves to the root: that is
/// what makes `list {}` and `search {pattern}` mean "the whole workspace".
fn path_arg(args: &Value) -> Result<String, ToolError> {
    match args.get("path") {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(invalid("\"path\" must be a string")),
    }
}

/// A byte count argument, bounded by `cap`.
fn byte_arg(args: &Value, key: &str, cap: u64) -> Result<Option<u64>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            Some(bytes) if bytes >= 1 && bytes <= cap => Ok(Some(bytes)),
            Some(0) => Err(invalid(format!("{key:?} must be at least 1 byte"))),
            Some(bytes) => Err(limit(format!(
                "{key:?} is {bytes} bytes, over the {cap}-byte cap"
            ))),
            None => Err(invalid(format!("{key:?} must be a whole number of bytes"))),
        },
    }
}

fn invalid(reason: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        reason: reason.into(),
    }
}

fn denied(reason: impl Into<String>) -> ToolError {
    ToolError::Denied {
        reason: reason.into(),
    }
}

fn not_found(reason: impl Into<String>) -> ToolError {
    ToolError::NotFound {
        reason: reason.into(),
    }
}

fn limit(reason: impl Into<String>) -> ToolError {
    ToolError::LimitExceeded {
        reason: reason.into(),
    }
}

/// A filesystem error in the tool taxonomy, with `what` naming the operation
/// and the path it failed on.
fn io_tool_error(err: io::Error, what: impl fmt::Display) -> ToolError {
    match err.kind() {
        io::ErrorKind::NotFound => not_found(format!("{what}: {err}")),
        io::ErrorKind::PermissionDenied => denied(format!("{what}: {err}")),
        _ => ToolError::Internal {
            reason: format!("{what}: {err}"),
        },
    }
}

/// Reads a whole file as text, bounded by [`MAX_FILE_BYTES`].
///
/// Shared by `edit_file`, which has to hold the file it rewrites: a file over
/// the cap is refused rather than edited in part.
fn read_text(path: &Path, label: &str) -> Result<String, ToolError> {
    let file =
        fs::File::open(path).map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    let metadata = file
        .metadata()
        .map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    if metadata.is_dir() {
        return Err(invalid(format!(
            "{label} is a directory; this reads one file"
        )));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(limit(format!(
            "{label} is {} bytes, over the {MAX_FILE_BYTES}-byte cap",
            metadata.len()
        )));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(limit(format!(
            "{label} is bigger than the {MAX_FILE_BYTES}-byte cap"
        )));
    }
    String::from_utf8(bytes).map_err(|err| {
        invalid(format!(
            "{label} is not UTF-8 text (an invalid sequence at byte {})",
            err.utf8_error().valid_up_to()
        ))
    })
}

/// Writes `bytes` to `path` atomically: a sibling temp file, flushed, renamed
/// over the target.
///
/// The rename is what makes a write all-or-nothing: a crash or a kill leaves
/// either the old file or the new one, never a half-written file. The temp file
/// is created in the target's own directory — a rename cannot cross a
/// filesystem — which is inside the workspace by construction. A failed rename
/// removes it again, so a refusal leaves no litter.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// `read_file`: the text of one file inside the workspace.
///
/// Arguments: `path` (required, relative), `max_bytes` (optional, default
/// [`READ_DEFAULT_BYTES`], at most [`MAX_FILE_BYTES`]).
///
/// Output: `{"path", "size", "bytes", "truncated", "content"}` — `size` is the
/// file's size on disk, `content` the text returned (`bytes` bytes of it), and
/// `truncated` is true when the cap, rather than the end of the file, ended it.
///
/// A file that is not UTF-8 text is refused (`ok: false`, "is not UTF-8 text")
/// rather than returned as mojibake: an agent reads text, and a mangled answer
/// is worse than an honest error. A cap that lands inside a multi-byte
/// character is not a refusal — the character the cap cut is dropped and
/// `truncated` says so, which is what keeps a whole-file read of a valid file
/// honest.
pub fn read_file(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = string_arg(args, "path")?;
    let cap = byte_arg(args, "max_bytes", MAX_FILE_BYTES)?.unwrap_or(READ_DEFAULT_BYTES);
    let (path, label) = workspace.resolve(&raw)?;
    let metadata =
        fs::metadata(&path).map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    if metadata.is_dir() {
        return Err(invalid(format!(
            "{label} is a directory; read_file reads one file"
        )));
    }

    let file =
        fs::File::open(&path).map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    let truncated = bytes.len() as u64 > cap;
    if truncated {
        // The cap is the window; the byte after it is only ever read to learn
        // that the file goes on.
        bytes.truncate(cap as usize);
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // The cap cut the last character in half: the prefix before it is the
        // file's own bytes and the file is valid, so keep them. (`from_utf8`
        // reports that case as an error with no invalid sequence, which is how
        // it is told apart from a file that really is not text.)
        Err(err) if truncated && err.utf8_error().error_len().is_none() => {
            let valid = err.utf8_error().valid_up_to();
            let bytes = err.into_bytes();
            // The prefix is valid by construction, so this is lossless.
            String::from_utf8_lossy(&bytes[..valid]).into_owned()
        }
        Err(err) => {
            return Err(invalid(format!(
                "{label} is not UTF-8 text (an invalid sequence at byte {})",
                err.utf8_error().valid_up_to()
            )));
        }
    };

    Ok(json!({
        "path": label,
        "size": metadata.len(),
        "bytes": text.len(),
        "truncated": truncated,
        "content": text,
    })
    .to_string())
}

/// `write_file`: creates or replaces one file inside the workspace.
///
/// Arguments: `path` (required, relative), `content` (required, a string, at
/// most [`MAX_WRITE_BYTES`]).
///
/// Output: `{"path", "bytes", "created"}` — `created` is true when the file did
/// not exist before. The write is atomic (temp file + rename), and the parent
/// directory is never created: a path whose directory does not exist is
/// `not_found`, not a tree the tool built on its own.
pub fn write_file(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = string_arg(args, "path")?;
    let content = string_arg(args, "content")?;
    if content.len() as u64 > MAX_WRITE_BYTES {
        return Err(limit(format!(
            "the content is {} bytes, over the {MAX_WRITE_BYTES}-byte write cap",
            content.len()
        )));
    }
    let (path, label) = workspace.resolve(&raw)?;
    if let Ok(metadata) = fs::metadata(&path)
        && metadata.is_dir()
    {
        return Err(invalid(format!(
            "{label} is a directory; write_file writes one file"
        )));
    }
    let created = !path.exists();
    write_atomic(&path, content.as_bytes())
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    Ok(json!({
        "path": label,
        "bytes": content.len(),
        "created": created,
    })
    .to_string())
}

/// `append_file`: adds `content` to the end of a file, creating it if needed.
///
/// Arguments: `path` (required, relative), `content` (required, a string, at
/// most [`MAX_WRITE_BYTES`]).
///
/// Output: `{"path", "bytes", "size"}` — `bytes` appended, `size` the file's
/// size after the append. A file that would grow past [`MAX_FILE_BYTES`] is
/// refused rather than truncated: an append that silently dropped its tail
/// would be a lie about what is in the file.
pub fn append_file(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = string_arg(args, "path")?;
    let content = string_arg(args, "content")?;
    if content.len() as u64 > MAX_WRITE_BYTES {
        return Err(limit(format!(
            "the content is {} bytes, over the {MAX_WRITE_BYTES}-byte write cap",
            content.len()
        )));
    }
    let (path, label) = workspace.resolve(&raw)?;
    let existing = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(invalid(format!(
                "{label} is a directory; append_file appends to one file"
            )));
        }
        Ok(metadata) => metadata.len(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
        Err(err) => return Err(io_tool_error(err, format!("cannot open {label}"))),
    };
    let size = existing + content.len() as u64;
    if size > MAX_FILE_BYTES {
        return Err(limit(format!(
            "appending would make {label} {size} bytes, over the {MAX_FILE_BYTES}-byte cap"
        )));
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| io_tool_error(err, format!("cannot open {label}")))?;
    file.write_all(content.as_bytes())
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    file.sync_all()
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    Ok(json!({
        "path": label,
        "bytes": content.len(),
        "size": size,
    })
    .to_string())
}

/// `edit_file`: one literal replacement, and only when it is unambiguous.
///
/// Arguments: `path` (required, relative), `find` (required, a non-empty
/// literal, at most [`MAX_NEEDLE_BYTES`]), `replace` (required, may be empty to
/// delete).
///
/// Output: `{"path", "matches", "size"}` with `matches` always 1: the find text
/// must appear **exactly once** in the file. Zero occurrences and more than one
/// are both `ok: false` with the reason — a fuzzy or first-of-many replacement
/// would be a guess about which occurrence the caller meant, and the tool
/// never guesses. The rewrite is atomic, like `write_file`.
pub fn edit_file(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = string_arg(args, "path")?;
    let find = string_arg(args, "find")?;
    let replace = string_arg(args, "replace")?;
    if find.is_empty() {
        return Err(invalid("\"find\" must not be empty"));
    }
    if find.len() > MAX_NEEDLE_BYTES {
        return Err(limit(format!(
            "\"find\" is {} bytes, over the {MAX_NEEDLE_BYTES}-byte cap",
            find.len()
        )));
    }
    if replace.len() as u64 > MAX_WRITE_BYTES {
        return Err(limit(format!(
            "\"replace\" is {} bytes, over the {MAX_WRITE_BYTES}-byte cap",
            replace.len()
        )));
    }
    let (path, label) = workspace.resolve(&raw)?;
    let content = read_text(&path, &label)?;
    let matches = content.matches(&find).count();
    if matches == 0 {
        return Err(not_found(format!(
            "the text to replace does not appear in {label}"
        )));
    }
    if matches > 1 {
        return Err(invalid(format!(
            "the text to replace appears {matches} times in {label}; edit_file needs exactly one match"
        )));
    }
    let updated = content.replacen(&find, &replace, 1);
    if updated.len() as u64 > MAX_FILE_BYTES {
        return Err(limit(format!(
            "the edit would make {label} {} bytes, over the {MAX_FILE_BYTES}-byte cap",
            updated.len()
        )));
    }
    write_atomic(&path, updated.as_bytes())
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    Ok(json!({
        "path": label,
        "matches": matches,
        "size": updated.len(),
    })
    .to_string())
}

/// `list`: the entries of one directory inside the workspace.
///
/// Arguments: `path` (optional; absent or `"."` is the workspace root).
///
/// Output: `{"path", "entries", "truncated", "skipped"}` where each entry is
/// `{"name", "kind", "size"}` — `kind` is `file`, `dir`, `symlink` or `other`,
/// and `size` is the file's size, `null` for anything else. Entries are sorted
/// by name, so the answer is the same for the same directory. `truncated` is
/// true when the directory held more entries than the cap (or than the scan
/// bound, where the returned set is a bounded sample, not a prefix of the
/// sorted order), and `skipped` counts names this side cannot report (a name
/// that is not UTF-8) rather than dropping them silently.
pub fn list(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = path_arg(args)?;
    let (path, label) = workspace.resolve(&raw)?;
    let metadata =
        fs::metadata(&path).map_err(|err| io_tool_error(err, format!("cannot list {label}")))?;
    if !metadata.is_dir() {
        return Err(invalid(format!("{label} is not a directory")));
    }
    let entries =
        fs::read_dir(&path).map_err(|err| io_tool_error(err, format!("cannot list {label}")))?;

    let mut rows: Vec<(String, &'static str, Option<u64>)> = Vec::new();
    let mut skipped = 0usize;
    let mut cut = false;
    for entry in entries {
        if rows.len() >= MAX_LIST_SCAN_ENTRIES {
            cut = true;
            break;
        }
        let entry = entry.map_err(|err| io_tool_error(err, format!("cannot list {label}")))?;
        // A name that is not UTF-8 cannot be named back to the hub as a path
        // argument, so it is counted, not mangled.
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|err| io_tool_error(err, format!("cannot list {label}")))?;
        let kind = if metadata.is_dir() {
            "dir"
        } else if metadata.is_file() {
            "file"
        } else if metadata.file_type().is_symlink() {
            "symlink"
        } else {
            "other"
        };
        rows.push((name, kind, metadata.is_file().then_some(metadata.len())));
    }
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    let truncated = cut || rows.len() > MAX_LIST_ENTRIES;
    rows.truncate(MAX_LIST_ENTRIES);

    let entries: Vec<Value> = rows
        .into_iter()
        .map(|(name, kind, size)| json!({ "name": name, "kind": kind, "size": size }))
        .collect();
    Ok(json!({
        "path": label,
        "entries": entries,
        "truncated": truncated,
        "skipped": skipped,
    })
    .to_string())
}

/// `search`: a literal substring search under one path, bounded and honest.
///
/// Arguments: `pattern` (required, a non-empty literal, at most
/// [`MAX_NEEDLE_BYTES`]), `path` (optional; absent or `"."` is the whole
/// workspace).
///
/// Output: `{"pattern", "path", "matches", "files_scanned", "bytes_scanned",
/// "skipped", "truncated"}` where each match is `{"file", "line", "text"}` —
/// `file` relative to the workspace, `line` 1-based, `text` the line (cut to
/// [`MAX_SEARCH_LINE_BYTES`]). Files are walked depth-first in sorted name
/// order, so the same tree gives the same answer. A file that is not UTF-8 text
/// is skipped (never searched as mojibake), as is one that cannot be read; both
/// are counted in `skipped`. `truncated` is true when a bound — matches, files,
/// bytes, depth, an unreadable-in-full file, or a cut line — ended the search
/// early: the result is never presented as complete when it is not.
pub fn search(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let pattern = string_arg(args, "pattern")?;
    if pattern.is_empty() {
        return Err(invalid("\"pattern\" must not be empty"));
    }
    if pattern.len() > MAX_NEEDLE_BYTES {
        return Err(limit(format!(
            "\"pattern\" is {} bytes, over the {MAX_NEEDLE_BYTES}-byte cap",
            pattern.len()
        )));
    }
    let raw = path_arg(args)?;
    let (path, label) = workspace.resolve(&raw)?;
    let metadata =
        fs::metadata(&path).map_err(|err| io_tool_error(err, format!("cannot search {label}")))?;

    let mut scan = Scan {
        pattern: &pattern,
        matches: Vec::new(),
        files: 0,
        bytes: 0,
        skipped: 0,
        cut: false,
        line_cut: false,
    };
    if metadata.is_dir() {
        walk(&path, &label, 0, &mut scan)?;
    } else if metadata.is_file() {
        scan_file(&path, &label, &mut scan);
    } else {
        return Err(invalid(format!(
            "{label} is neither a file nor a directory"
        )));
    }

    Ok(json!({
        "pattern": pattern,
        "path": label,
        "matches": scan.matches,
        "files_scanned": scan.files,
        "bytes_scanned": scan.bytes,
        "skipped": scan.skipped,
        "truncated": scan.cut || scan.line_cut,
    })
    .to_string())
}

/// One `search`'s running state and its bounds.
struct Scan<'a> {
    /// The literal being looked for.
    pattern: &'a str,
    /// The matches found so far, in walk order.
    matches: Vec<Value>,
    /// How many files have been opened.
    files: usize,
    /// How many bytes have been read.
    bytes: u64,
    /// How many files were not searched (not UTF-8 text, or unreadable).
    skipped: usize,
    /// A bound stopped the scan: the result is not complete. Doubles as the
    /// walk's stop flag, so a bound ends the walk where it was reached.
    cut: bool,
    /// A returned line was cut to [`MAX_SEARCH_LINE_BYTES`].
    line_cut: bool,
}

/// Walks `dir` depth-first in sorted name order, scanning the files in it.
fn walk(dir: &Path, label: &str, depth: usize, scan: &mut Scan<'_>) -> Result<(), ToolError> {
    if scan.cut {
        return Ok(());
    }
    if depth > MAX_SEARCH_DEPTH {
        scan.cut = true;
        return Ok(());
    }
    let entries =
        fs::read_dir(dir).map_err(|err| io_tool_error(err, format!("cannot search {label}")))?;
    // Sorted before the recursion, so the walk — and which files a bound
    // leaves out — is a property of the tree, not of readdir's order.
    let mut children: Vec<(String, PathBuf, bool, bool)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| io_tool_error(err, format!("cannot search {label}")))?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                scan.skipped += 1;
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                scan.skipped += 1;
                continue;
            }
        };
        children.push((name, entry.path(), file_type.is_dir(), file_type.is_file()));
    }
    children.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, path, is_dir, is_file) in children {
        if scan.cut {
            return Ok(());
        }
        let child = if label == "." {
            name
        } else {
            format!("{label}/{name}")
        };
        if is_dir {
            walk(&path, &child, depth + 1, scan)?;
        } else if is_file {
            scan_file(&path, &child, scan);
        }
        // A symlink is neither: it is not followed, which is what keeps a link
        // from walking the scan out of the volume.
    }
    Ok(())
}

/// Scans one file for `scan.pattern`, counting it against every bound.
fn scan_file(path: &Path, label: &str, scan: &mut Scan<'_>) {
    if scan.files >= MAX_SEARCH_FILES || scan.bytes >= MAX_SEARCH_SCAN_BYTES {
        scan.cut = true;
        return;
    }
    scan.files += 1;
    let mut bytes = Vec::new();
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => {
            scan.skipped += 1;
            return;
        }
    };
    if file
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        scan.skipped += 1;
        return;
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        // Too big to search whole: reporting it as searched would be a lie.
        scan.cut = true;
        return;
    }
    scan.bytes += bytes.len() as u64;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            // A binary file is skipped, never searched as mojibake.
            scan.skipped += 1;
            return;
        }
    };
    for (index, line) in text.lines().enumerate() {
        if scan.matches.len() >= MAX_SEARCH_MATCHES {
            scan.cut = true;
            return;
        }
        if !line.contains(scan.pattern) {
            continue;
        }
        let (text, cut) = cut_line(line);
        scan.line_cut |= cut;
        scan.matches.push(json!({
            "file": label,
            "line": index + 1,
            "text": text,
        }));
    }
    if scan.bytes >= MAX_SEARCH_SCAN_BYTES {
        scan.cut = true;
    }
}

/// The first [`MAX_SEARCH_LINE_BYTES`] bytes of `line`, cut on a character
/// boundary, with whether it was cut.
fn cut_line(line: &str) -> (&str, bool) {
    if line.len() <= MAX_SEARCH_LINE_BYTES {
        return (line, false);
    }
    let mut end = MAX_SEARCH_LINE_BYTES;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    (&line[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A fresh scratch workspace (and, beside it, a scratch directory that is
    /// *outside* it, for the escape tests).
    fn scratch(name: &str) -> (Workspace, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "apz-harness-{}-{name}-{unique}",
            std::process::id()
        ));
        let inside = base.join("workspace");
        let outside = base.join("outside");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&inside).expect("a scratch workspace");
        fs::create_dir_all(&outside).expect("a scratch directory beside it");
        (Workspace::at(inside), outside)
    }

    /// The JSON a tool returned, parsed.
    fn served(workspace: &Workspace, name: &str, args: Value) -> Value {
        let output = crate::tools::serve(workspace, name, &args)
            .unwrap_or_else(|err| panic!("{name} {args} should serve: {err}"));
        serde_json::from_str(&output).expect("a tool's output is JSON")
    }

    /// The error a tool refused with.
    fn refused(workspace: &Workspace, name: &str, args: Value) -> ToolError {
        crate::tools::serve(workspace, name, &args).expect_err("this call should be refused")
    }

    fn write_inside(workspace: &Workspace, relative: &str, bytes: &[u8]) {
        let path = workspace.root().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("the parent directory");
        }
        fs::write(path, bytes).expect("the scratch file");
    }

    // --- path discipline: the refusals every tool shares -------------------

    #[test]
    fn an_absolute_path_is_denied_with_its_reason() {
        let (workspace, _outside) = scratch("absolute");
        for name in [
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
            "search",
        ] {
            let mut args = json!({ "path": "/etc/passwd" });
            if name == "write_file" || name == "append_file" {
                args["content"] = json!("x");
            }
            if name == "edit_file" {
                args["find"] = json!("x");
                args["replace"] = json!("y");
            }
            if name == "search" {
                args["pattern"] = json!("x");
            }
            let err = refused(&workspace, name, args);
            assert!(
                matches!(
                    err,
                    ToolError::Denied { .. } | ToolError::InvalidInput { .. }
                ),
                "{name}: {err:?}"
            );
            assert!(err.to_string().contains("absolute"), "{name}: {err}");
        }
    }

    #[test]
    fn a_parent_component_is_denied_however_it_is_spelled() {
        let (workspace, _outside) = scratch("parent");
        write_inside(&workspace, "notes.txt", b"inside\n");
        for path in ["../notes.txt", "a/../../notes.txt", "./../x", "a/..", ".."] {
            let err = refused(&workspace, "read_file", json!({ "path": path }));
            assert!(matches!(err, ToolError::Denied { .. }), "{path}: {err:?}");
            assert!(err.to_string().contains("\"..\""), "{path}: {err}");
        }
    }

    #[test]
    fn a_symlink_that_leaves_the_workspace_is_denied() {
        let (workspace, outside) = scratch("symlink-out");
        fs::write(outside.join("secret.txt"), b"host data\n").expect("the outside file");
        symlink(
            outside.join("secret.txt"),
            workspace.root().join("link.txt"),
        )
        .expect("a symlink to outside");
        symlink(&outside, workspace.root().join("outsidedir"))
            .expect("a symlink to an outside dir");

        // Reading through the link, or listing it, resolves outside.
        let err = refused(&workspace, "read_file", json!({ "path": "link.txt" }));
        assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
        assert!(err.to_string().contains("outside the workspace"), "{err}");
        let err = refused(&workspace, "list", json!({ "path": "outsidedir" }));
        assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
        // Writing through it too, and through a link *inside* a path.
        let err = refused(
            &workspace,
            "write_file",
            json!({ "path": "outsidedir/new.txt", "content": "x" }),
        );
        assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
        let err = refused(
            &workspace,
            "append_file",
            json!({ "path": "outsidedir/new.txt", "content": "x" }),
        );
        assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
        // And search does not follow one out either.
        let result = served(&workspace, "search", json!({ "pattern": "host" }));
        assert_eq!(
            result["matches"].as_array().map(Vec::len),
            Some(0),
            "{result}"
        );
        // Nothing was written outside.
        assert!(!outside.join("new.txt").exists());
    }

    #[test]
    fn a_symlink_that_stays_inside_is_followed() {
        let (workspace, _outside) = scratch("symlink-in");
        write_inside(&workspace, "real.txt", b"the real file\n");
        symlink("real.txt", workspace.root().join("alias.txt")).expect("a symlink inside");
        let result = served(&workspace, "read_file", json!({ "path": "alias.txt" }));
        assert_eq!(result["content"], "the real file\n");
    }

    #[test]
    fn a_missing_workspace_is_unavailable_not_a_panic() {
        let workspace = Workspace::at("/no/such/plaza/workspace");
        let err = refused(&workspace, "list", json!({}));
        assert!(matches!(err, ToolError::Unavailable { .. }), "{err:?}");
        assert!(err.to_string().starts_with("unavailable: "), "{err}");
    }

    // --- read_file ---------------------------------------------------------

    #[test]
    fn read_file_returns_the_text_and_its_size() {
        let (workspace, _outside) = scratch("read");
        write_inside(&workspace, "notes.txt", "hello\n".as_bytes());
        let result = served(&workspace, "read_file", json!({ "path": "notes.txt" }));
        assert_eq!(result["path"], "notes.txt");
        assert_eq!(result["content"], "hello\n");
        assert_eq!(result["size"], 6);
        assert_eq!(result["bytes"], 6);
        assert_eq!(result["truncated"], false);
    }

    #[test]
    fn read_file_reports_truncation_instead_of_pretending() {
        let (workspace, _outside) = scratch("read-cap");
        write_inside(&workspace, "big.txt", "0123456789".as_bytes());
        let result = served(
            &workspace,
            "read_file",
            json!({ "path": "big.txt", "max_bytes": 4 }),
        );
        assert_eq!(result["content"], "0123");
        assert_eq!(result["size"], 10);
        assert_eq!(result["bytes"], 4);
        assert_eq!(result["truncated"], true);

        // A cap over the hard cap is refused, not silently clamped.
        let err = refused(
            &workspace,
            "read_file",
            json!({ "path": "big.txt", "max_bytes": MAX_FILE_BYTES + 1 }),
        );
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
        assert!(err.to_string().starts_with("limit exceeded: "), "{err}");
        // As is a cap of zero bytes, or one that is not a whole number.
        assert!(matches!(
            refused(
                &workspace,
                "read_file",
                json!({ "path": "big.txt", "max_bytes": 0 })
            ),
            ToolError::InvalidInput { .. }
        ));
        assert!(matches!(
            refused(
                &workspace,
                "read_file",
                json!({ "path": "big.txt", "max_bytes": 1.5 })
            ),
            ToolError::InvalidInput { .. }
        ));
    }

    #[test]
    fn read_file_keeps_a_valid_file_valid_when_the_cap_cuts_a_character() {
        let (workspace, _outside) = scratch("read-char");
        // "é" is two bytes: a 1-byte cap lands inside it.
        write_inside(&workspace, "utf8.txt", "é!\n".as_bytes());
        let result = served(
            &workspace,
            "read_file",
            json!({ "path": "utf8.txt", "max_bytes": 1 }),
        );
        assert_eq!(result["content"], "");
        assert_eq!(result["bytes"], 0);
        assert_eq!(result["truncated"], true, "{result}");
        let result = served(
            &workspace,
            "read_file",
            json!({ "path": "utf8.txt", "max_bytes": 2 }),
        );
        assert_eq!(result["content"], "é");
        assert_eq!(result["truncated"], true, "{result}");
    }

    #[test]
    fn read_file_refuses_binary_content_rather_than_mangling_it() {
        let (workspace, _outside) = scratch("read-binary");
        write_inside(&workspace, "blob.bin", &[0xff, 0xfe, 0x00, 0x01]);
        let err = refused(&workspace, "read_file", json!({ "path": "blob.bin" }));
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        assert!(err.to_string().contains("is not UTF-8 text"), "{err}");
    }

    #[test]
    fn read_file_refuses_a_missing_file_and_a_directory() {
        let (workspace, _outside) = scratch("read-missing");
        fs::create_dir_all(workspace.root().join("sub")).expect("a subdirectory");
        let err = refused(&workspace, "read_file", json!({ "path": "nope.txt" }));
        assert!(matches!(err, ToolError::NotFound { .. }), "{err:?}");
        assert!(err.to_string().starts_with("not found: "), "{err}");
        let err = refused(&workspace, "read_file", json!({ "path": "sub" }));
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        assert!(err.to_string().contains("is a directory"), "{err}");
    }

    #[test]
    fn read_file_requires_its_path_argument() {
        let (workspace, _outside) = scratch("read-args");
        assert!(matches!(
            refused(&workspace, "read_file", json!({})),
            ToolError::InvalidInput { .. }
        ));
        assert!(matches!(
            refused(&workspace, "read_file", json!({ "path": 7 })),
            ToolError::InvalidInput { .. }
        ));
    }

    // --- write_file --------------------------------------------------------

    #[test]
    fn write_file_creates_then_replaces() {
        let (workspace, _outside) = scratch("write");
        fs::create_dir_all(workspace.root().join("a")).expect("the parent directory");
        let result = served(
            &workspace,
            "write_file",
            json!({ "path": "a/b.txt", "content": "one" }),
        );
        assert_eq!(result["path"], "a/b.txt");
        assert_eq!(result["bytes"], 3);
        assert_eq!(result["created"], true);
        let result = served(
            &workspace,
            "write_file",
            json!({ "path": "a/b.txt", "content": "two" }),
        );
        assert_eq!(result["created"], false);
        assert_eq!(
            served(&workspace, "read_file", json!({ "path": "a/b.txt" }))["content"],
            "two"
        );

        // The write left no temp file behind.
        let names: Vec<String> = fs::read_dir(workspace.root().join("a"))
            .expect("the directory")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .into_string()
                    .expect("UTF-8")
            })
            .collect();
        assert_eq!(names, vec!["b.txt".to_string()], "{names:?}");
    }

    #[test]
    fn write_file_never_creates_the_parent_directory() {
        let (workspace, _outside) = scratch("write-parent");
        let err = refused(
            &workspace,
            "write_file",
            json!({ "path": "no/such/dir/x.txt", "content": "x" }),
        );
        assert!(matches!(err, ToolError::NotFound { .. }), "{err:?}");
        assert!(!workspace.root().join("no").exists());
    }

    #[test]
    fn write_file_refuses_an_oversized_payload_and_a_directory_target() {
        let (workspace, _outside) = scratch("write-caps");
        let big = "x".repeat(MAX_WRITE_BYTES as usize + 1);
        let err = refused(
            &workspace,
            "write_file",
            json!({ "path": "big.txt", "content": big }),
        );
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
        fs::create_dir_all(workspace.root().join("dir")).expect("a directory");
        let err = refused(
            &workspace,
            "write_file",
            json!({ "path": "dir", "content": "x" }),
        );
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        assert!(err.to_string().contains("is a directory"), "{err}");
    }

    // --- append_file -------------------------------------------------------

    #[test]
    fn append_file_creates_then_appends() {
        let (workspace, _outside) = scratch("append");
        let result = served(
            &workspace,
            "append_file",
            json!({ "path": "log.txt", "content": "one\n" }),
        );
        assert_eq!(result["bytes"], 4);
        assert_eq!(result["size"], 4);
        let result = served(
            &workspace,
            "append_file",
            json!({ "path": "log.txt", "content": "two\n" }),
        );
        assert_eq!(result["size"], 8);
        assert_eq!(
            served(&workspace, "read_file", json!({ "path": "log.txt" }))["content"],
            "one\ntwo\n"
        );
    }

    #[test]
    fn append_file_refuses_to_grow_a_file_past_the_cap() {
        let (workspace, _outside) = scratch("append-cap");
        write_inside(
            &workspace,
            "full.txt",
            &vec![b'a'; MAX_FILE_BYTES as usize - 1],
        );
        let err = refused(
            &workspace,
            "append_file",
            json!({ "path": "full.txt", "content": "ab" }),
        );
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
        assert!(err.to_string().contains("over the"), "{err}");
        // A chunk over the write cap is refused before the file is touched.
        let err = refused(
            &workspace,
            "append_file",
            json!({ "path": "new.txt", "content": "x".repeat(MAX_WRITE_BYTES as usize + 1) }),
        );
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
        assert!(!workspace.root().join("new.txt").exists());
    }

    // --- edit_file ---------------------------------------------------------

    #[test]
    fn edit_file_replaces_the_one_match() {
        let (workspace, _outside) = scratch("edit");
        write_inside(&workspace, "code.txt", b"alpha = 1\nbeta = 1\nalpha = 1\n");
        let result = served(
            &workspace,
            "edit_file",
            json!({ "path": "code.txt", "find": "beta = 1", "replace": "beta = 2" }),
        );
        assert_eq!(result["matches"], 1);
        assert_eq!(
            served(&workspace, "read_file", json!({ "path": "code.txt" }))["content"],
            "alpha = 1\nbeta = 2\nalpha = 1\n"
        );
    }

    #[test]
    fn edit_file_refuses_zero_and_many_matches() {
        let (workspace, _outside) = scratch("edit-ambiguous");
        write_inside(&workspace, "code.txt", b"x = 1\nx = 1\n");
        let err = refused(
            &workspace,
            "edit_file",
            json!({ "path": "code.txt", "find": "x = 1", "replace": "x = 2" }),
        );
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        assert!(err.to_string().contains("appears 2 times"), "{err}");
        let err = refused(
            &workspace,
            "edit_file",
            json!({ "path": "code.txt", "find": "nope", "replace": "x" }),
        );
        assert!(matches!(err, ToolError::NotFound { .. }), "{err:?}");
        assert!(err.to_string().contains("does not appear"), "{err}");
        // The file is untouched by either refusal.
        assert_eq!(
            served(&workspace, "read_file", json!({ "path": "code.txt" }))["content"],
            "x = 1\nx = 1\n"
        );
        // An empty needle is refused: it would match everywhere.
        let err = refused(
            &workspace,
            "edit_file",
            json!({ "path": "code.txt", "find": "", "replace": "x" }),
        );
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn edit_file_can_delete_and_refuses_a_binary_file() {
        let (workspace, _outside) = scratch("edit-delete");
        write_inside(&workspace, "t.txt", b"keep\ndrop\n");
        served(
            &workspace,
            "edit_file",
            json!({ "path": "t.txt", "find": "drop\n", "replace": "" }),
        );
        assert_eq!(
            served(&workspace, "read_file", json!({ "path": "t.txt" }))["content"],
            "keep\n"
        );
        write_inside(&workspace, "b.bin", &[0x00, 0xff]);
        let err = refused(
            &workspace,
            "edit_file",
            json!({ "path": "b.bin", "find": "x", "replace": "y" }),
        );
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        assert!(err.to_string().contains("is not UTF-8 text"), "{err}");
    }

    #[test]
    fn edit_file_refuses_a_file_over_its_cap() {
        let (workspace, _outside) = scratch("edit-cap");
        write_inside(
            &workspace,
            "big.txt",
            &vec![b'a'; MAX_FILE_BYTES as usize + 1],
        );
        let err = refused(
            &workspace,
            "edit_file",
            json!({ "path": "big.txt", "find": "a", "replace": "b" }),
        );
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
        // A needle over its own cap is refused too.
        let err = refused(
            &workspace,
            "edit_file",
            json!({ "path": "big.txt", "find": "x".repeat(MAX_NEEDLE_BYTES + 1), "replace": "y" }),
        );
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
    }

    // --- list --------------------------------------------------------------

    #[test]
    fn list_is_sorted_and_typed() {
        let (workspace, _outside) = scratch("list");
        write_inside(&workspace, "b.txt", b"12");
        write_inside(&workspace, "a.txt", b"1");
        fs::create_dir_all(workspace.root().join("sub")).expect("a subdirectory");
        symlink("a.txt", workspace.root().join("link")).expect("a symlink");

        let result = served(&workspace, "list", json!({}));
        assert_eq!(result["path"], ".");
        assert_eq!(result["truncated"], false);
        assert_eq!(result["skipped"], 0);
        let entries = result["entries"].as_array().expect("entries").clone();
        let names: Vec<&str> = entries
            .iter()
            .map(|entry| entry["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "link", "sub"]);
        assert_eq!(entries[0]["kind"], "file");
        assert_eq!(entries[0]["size"], 1);
        assert_eq!(entries[1]["size"], 2);
        assert_eq!(entries[2]["kind"], "symlink");
        assert_eq!(entries[2]["size"], Value::Null);
        assert_eq!(entries[3]["kind"], "dir");
        assert_eq!(entries[3]["size"], Value::Null);

        // A subdirectory lists by its relative path.
        let result = served(&workspace, "list", json!({ "path": "." }));
        assert_eq!(result["path"], ".");
        let result = served(&workspace, "list", json!({ "path": "sub" }));
        assert_eq!(result["path"], "sub");
    }

    #[test]
    fn list_reports_truncation_of_a_larger_directory() {
        let (workspace, _outside) = scratch("list-cap");
        for index in 0..MAX_LIST_ENTRIES + 1 {
            write_inside(&workspace, &format!("f{index:05}.txt"), b"");
        }
        let result = served(&workspace, "list", json!({}));
        let entries = result["entries"].as_array().expect("entries");
        assert_eq!(entries.len(), MAX_LIST_ENTRIES);
        assert_eq!(result["truncated"], true);
        // Deterministic: the smallest names, sorted.
        assert_eq!(entries[0]["name"], "f00000.txt");
        assert_eq!(entries[MAX_LIST_ENTRIES - 1]["name"], "f01023.txt");
    }

    #[test]
    fn list_refuses_a_file_and_a_missing_path() {
        let (workspace, _outside) = scratch("list-bad");
        write_inside(&workspace, "a.txt", b"x");
        let err = refused(&workspace, "list", json!({ "path": "a.txt" }));
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        assert!(err.to_string().contains("is not a directory"), "{err}");
        let err = refused(&workspace, "list", json!({ "path": "nope" }));
        assert!(matches!(err, ToolError::NotFound { .. }), "{err:?}");
    }

    // --- search ------------------------------------------------------------

    #[test]
    fn search_finds_literal_matches_in_a_deterministic_order() {
        let (workspace, _outside) = scratch("search");
        write_inside(&workspace, "b.txt", b"needle here\nnot here\n");
        write_inside(&workspace, "a.txt", b"nothing\n");
        write_inside(&workspace, "sub/c.txt", b"one\nneedle again\n");
        let result = served(&workspace, "search", json!({ "pattern": "needle" }));
        assert_eq!(result["truncated"], false);
        assert_eq!(result["skipped"], 0);
        assert_eq!(result["files_scanned"], 3);
        let matches = result["matches"].as_array().expect("matches").clone();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0]["file"], "b.txt");
        assert_eq!(matches[0]["line"], 1);
        assert_eq!(matches[0]["text"], "needle here");
        assert_eq!(matches[1]["file"], "sub/c.txt");
        assert_eq!(matches[1]["line"], 2);
        // The whole workspace is the default; a path narrows it.
        let result = served(
            &workspace,
            "search",
            json!({ "pattern": "needle", "path": "sub" }),
        );
        assert_eq!(result["path"], "sub");
        assert_eq!(result["matches"].as_array().map(Vec::len), Some(1));
        // A pattern with no match is an empty result, not an error.
        let result = served(&workspace, "search", json!({ "pattern": "absent" }));
        assert_eq!(result["matches"].as_array().map(Vec::len), Some(0));
        assert_eq!(result["truncated"], false);
    }

    #[test]
    fn search_skips_binary_files_and_counts_them() {
        let (workspace, _outside) = scratch("search-binary");
        write_inside(&workspace, "text.txt", b"needle\n");
        write_inside(&workspace, "blob.bin", &[0xff, 0x00, 0xfe]);
        let result = served(&workspace, "search", json!({ "pattern": "needle" }));
        assert_eq!(result["matches"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["skipped"], 1);
        assert_eq!(result["truncated"], false);
    }

    #[test]
    fn search_reports_truncation_by_matches_and_by_cut_lines() {
        let (workspace, _outside) = scratch("search-cap");
        let mut many = String::new();
        for _ in 0..MAX_SEARCH_MATCHES + 10 {
            many.push_str("needle\n");
        }
        write_inside(&workspace, "many.txt", many.as_bytes());
        let result = served(&workspace, "search", json!({ "pattern": "needle" }));
        assert_eq!(
            result["matches"].as_array().map(Vec::len),
            Some(MAX_SEARCH_MATCHES)
        );
        assert_eq!(result["truncated"], true);

        // A line longer than the per-line cap is cut, and that is said too.
        let (workspace, _outside) = scratch("search-long-line");
        let long = format!("{}needle", "x".repeat(MAX_SEARCH_LINE_BYTES * 2));
        write_inside(&workspace, "long.txt", long.as_bytes());
        let result = served(&workspace, "search", json!({ "pattern": "needle" }));
        let matches = result["matches"].as_array().expect("matches");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0]["text"].as_str().map(str::len),
            Some(MAX_SEARCH_LINE_BYTES)
        );
        assert_eq!(result["truncated"], true);
    }

    #[test]
    fn search_refuses_an_empty_or_oversized_pattern() {
        let (workspace, _outside) = scratch("search-args");
        assert!(matches!(
            refused(&workspace, "search", json!({ "pattern": "" })),
            ToolError::InvalidInput { .. }
        ));
        assert!(matches!(
            refused(&workspace, "search", json!({ "pattern": 5 })),
            ToolError::InvalidInput { .. }
        ));
        let err = refused(
            &workspace,
            "search",
            json!({ "pattern": "x".repeat(MAX_NEEDLE_BYTES + 1) }),
        );
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
    }

    #[test]
    fn search_scans_one_file_when_the_path_is_a_file() {
        let (workspace, _outside) = scratch("search-file");
        write_inside(&workspace, "a.txt", b"needle\n");
        let result = served(
            &workspace,
            "search",
            json!({ "pattern": "needle", "path": "a.txt" }),
        );
        assert_eq!(result["files_scanned"], 1);
        assert_eq!(result["matches"].as_array().map(Vec::len), Some(1));
    }

    // --- the shape of the whole table --------------------------------------

    #[test]
    fn every_tool_refuses_arbitrary_arguments_without_panicking() {
        // The hub's arguments are not trusted: whatever arrives, a tool answers
        // (Ok or Err) and never unwinds.
        let (workspace, _outside) = scratch("random");
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let names = [
            "echo",
            "uptime",
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
            "search",
        ];
        for index in 0..2000 {
            let value = match next() % 8 {
                0 => Value::Null,
                1 => json!(next() as i64),
                2 => json!(next()),
                3 => json!([next(), "x", null]),
                4 => json!({ "path": next() as i64 }),
                5 => json!({ "path": ["a", "b"], "content": { "k": 1 } }),
                6 => json!({ "pattern": next().to_string(), "max_bytes": -1 }),
                _ => json!({ "path": format!("f{index}"), "find": "", "replace": null }),
            };
            for name in names {
                let _ = crate::tools::serve(&workspace, name, &value);
            }
        }
    }
}
