//! The file tools: the agent's hands on the writable `/workspace` volume.
//!
//! [`conversation`] lives here too: a peer's message is not a filesystem
//! operation, but what the agent does with it is — one escaped line appended to
//! its own [`MESSAGES_LOG`] — so it comes through the same path discipline and
//! the same append as [`append_file`], and the agent never touches the peer.
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
//! | [`READ_DEFAULT_LINES`] | 2000 lines | `read_file` without `limit` |
//! | [`MAX_READ_LINE_CHARS`] | 2000 chars | one line `read_file` returns whole |
//! | [`MAX_FILE_BYTES`] | 1 MiB | any file the tools will read or grow |
//! | [`MAX_WRITE_BYTES`] | 256 KiB | one `write_file`/`append_file`/`edit_file` payload |
//! | [`MAX_MESSAGE_BYTES`] | 2 KiB | one `conversation` message's `text` |
//! | [`MAX_NEEDLE_BYTES`] | 4 KiB | a `search` pattern or an edit's `old` |
//! | [`MAX_MULTI_EDITS`] | 64 | edits in one `multi_edit` |
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

use crate::egress;
use crate::session::escape_for_transcript;
use crate::tools::ToolError;

/// Where the guest's init mounts the writable volume.
pub const WORKSPACE_ROOT: &str = "/workspace";

/// The most lines `read_file` returns when it is not told how many: 2000.
pub const READ_DEFAULT_LINES: usize = 2000;

/// The longest line `read_file` returns whole, in characters. A longer line is
/// cut to this many characters and ends in `…`.
pub const MAX_READ_LINE_CHARS: usize = 2000;

/// How many leading bytes of a file are searched for a NUL to call it binary.
const BINARY_PROBE_BYTES: usize = 8 * 1024;

/// The largest file any file tool will act on: 1 MiB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The largest single `write_file`/`append_file` payload, and the largest
/// `edit_file` replacement: 256 KiB.
pub const MAX_WRITE_BYTES: u64 = 256 * 1024;

/// The most edits one `multi_edit` applies.
pub const MAX_MULTI_EDITS: usize = 64;

/// The agent's conversation log, relative to the workspace root: the file
/// [`conversation`] appends one line to for every message a peer sends this
/// agent.
pub const MESSAGES_LOG: &str = "messages.log";

/// The most bytes one conversation's `text` may occupy: the contract's 2 KiB
/// text cap (`docs/agents/social.md`, the `/say` endpoint's `MAX_SAY_TEXT`),
/// which this tool re-checks because the hub's arguments are not trusted to
/// have stopped at the door.
pub const MAX_MESSAGE_BYTES: usize = 2048;

/// The largest `search` pattern, or edit `old` text: 4 KiB.
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

    /// The directory the workspace is rooted at, as configured (the shell's
    /// working directory).
    pub fn dir(&self) -> &Path {
        &self.root
    }

    /// The harness's own state directory under the root (the chat threads).
    /// The model's tools never reach it: see [`PLAZA_DIR`].
    pub fn plaza_dir(&self) -> PathBuf {
        self.root.join(PLAZA_DIR)
    }

    /// The root this workspace was configured with, as given.
    ///
    /// The tools work from the *canonical* root ([`Workspace::canonical_root`]);
    /// this is the raw path, which the tests address a scratch directory by.
    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root a task's Weft lane is checked out under: the canonical root when it is mounted, else the root as given.
    pub fn weft_root(&self) -> PathBuf {
        self.root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone())
    }

    /// The canonical workspace root, or `Unavailable` when it is not mounted.
    ///
    /// Canonicalizing once and working from the result is what makes the
    /// escape check below meaningful: the comparison is between two resolved
    /// paths, so a root that is itself a symlink cannot make the check say
    /// "inside" for something that is not.
    pub(crate) fn canonical_root(&self) -> Result<PathBuf, ToolError> {
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
    pub(crate) fn resolve(&self, raw: &str) -> Result<(PathBuf, String), ToolError> {
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
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    // A dangling link has no target to canonicalize, so the walk
                    // would pass it and a write would then create whatever it
                    // names, outside the workspace or in `.plaza`. Refuse it.
                    if fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
                        return Err(denied(format!(
                            "path {raw:?} passes through a symlink with no target; a write never follows one"
                        )));
                    }
                    match ancestor.parent() {
                        Some(parent) => ancestor = parent,
                        None => {
                            return Err(denied(format!("path {raw:?} cannot be resolved")));
                        }
                    }
                }
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
        // The lexical check above refuses `.plaza` by name; this is the same
        // refusal for a link that lands in it (`p -> .plaza`).
        if names_plaza(&landed, &root) {
            return Err(plaza_refusal(raw));
        }
        Ok((path, label_of(&parts)))
    }
}

/// The components of a tool-supplied path, relative to the workspace root.
///
/// Refuses an absolute path, a Windows-style prefix and a `..` component;
/// `.` is dropped. An empty result is the workspace root itself, which only
/// the directory tools (`list`, `search`) accept.
/// Where the workspace is mounted in the guest.
const WORKSPACE_MOUNT: &str = "/workspace";

/// The harness's own state directory under the workspace root. No model-facing
/// path may name it, and `list` at the root does not show it.
pub const PLAZA_DIR: &str = ".plaza";

fn relative_parts(raw: &str) -> Result<Vec<String>, ToolError> {
    // Models name the workspace by its mount point; `/workspace/x` is `x`.
    let raw = match raw.strip_prefix(WORKSPACE_MOUNT) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.trim_start_matches('/'),
        _ => raw,
    };
    let mut parts = Vec::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(part) => match part.to_str() {
                Some(PLAZA_DIR) => return Err(plaza_refusal(raw)),
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

/// The refusal for a path that names the harness's own state. The lexical
/// check and the check on the landed path both return it, so they read alike.
fn plaza_refusal(raw: &str) -> ToolError {
    denied(format!(
        "path {raw:?} names the harness's own state; the model's tools do not reach it"
    ))
}

/// Whether the canonical `landed` path sits inside `.plaza` under `root`.
pub(crate) fn names_plaza(landed: &Path, root: &Path) -> bool {
    matches!(
        landed.strip_prefix(root).ok().and_then(|rest| rest.components().next()),
        Some(Component::Normal(first)) if first == PLAZA_DIR
    )
}

/// A required string argument.
pub(crate) fn string_arg(args: &Value, key: &str) -> Result<String, ToolError> {
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

/// An optional whole-number count of lines under `key`, `default` when absent.
fn line_arg(args: &Value, key: &str, default: usize) -> Result<usize, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| invalid(format!("{key:?} must be a whole number of lines"))),
    }
}

pub(crate) fn invalid(reason: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        reason: reason.into(),
    }
}

pub(crate) fn denied(reason: impl Into<String>) -> ToolError {
    ToolError::Denied {
        reason: reason.into(),
    }
}

pub(crate) fn not_found(reason: impl Into<String>) -> ToolError {
    ToolError::NotFound {
        reason: reason.into(),
    }
}

pub(crate) fn limit(reason: impl Into<String>) -> ToolError {
    ToolError::LimitExceeded {
        reason: reason.into(),
    }
}

/// Refuses a write whose target is itself a symlink. `resolve` canonicalizes
/// the deepest existing ANCESTOR, so a dangling link inside the workspace (the
/// link exists, its target does not) passes the walk and `open` would then
/// create the target outside the workspace; a live link would make the write
/// land wherever it points. Reads may follow links that resolve inside; a
/// write never follows one.
pub(crate) fn refuse_symlink_target(path: &std::path::Path, label: &str) -> Result<(), ToolError> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(denied(format!(
            "{label} is a symlink; a write never follows one"
        )));
    }
    Ok(())
}

/// A filesystem error in the tool taxonomy, with `what` naming the operation
/// and the path it failed on.
pub(crate) fn io_tool_error(err: io::Error, what: impl fmt::Display) -> ToolError {
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
/// Shared by `edit_file` and `multi_edit`, which have to hold the file they
/// rewrite: a file over the cap is refused rather than edited in part.
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
/// In a root guest, hands a file or folder the file tools made to the agent user (uid 1000),
/// so the agent's own shell — which runs as that user — can change it like anything it made.
pub(crate) fn give_to_agent(path: &Path) {
    if crate::bash::runs_as_root().unwrap_or(false) {
        let id = crate::bash::UNPRIVILEGED;
        let _ = std::os::unix::fs::lchown(path, Some(id), Some(id));
    }
}

/// `create_dir_all`, handing every folder it creates to the agent user.
pub(crate) fn create_dirs(dir: &Path) -> io::Result<()> {
    let missing: Vec<PathBuf> = dir
        .ancestors()
        .take_while(|a| !a.exists())
        .map(Path::to_path_buf)
        .collect();
    fs::create_dir_all(dir)?;
    for made in missing {
        give_to_agent(&made);
    }
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
    give_to_agent(&temp);
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// One line as `cat -n` numbers it: the number right-aligned in six columns, a
/// tab, the line, and a newline. A line over [`MAX_READ_LINE_CHARS`] characters
/// is cut to that many and ends in `…`.
fn numbered_line(number: usize, line: &str) -> String {
    let body = if line.chars().count() > MAX_READ_LINE_CHARS {
        let cut: String = line.chars().take(MAX_READ_LINE_CHARS).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    };
    format!("{number:>6}\t{body}\n")
}

/// `read_file`: the lines of one file inside the workspace, numbered as `cat -n`
/// numbers them.
///
/// Arguments: `path` (required, relative); `offset` (optional, the 0-based first
/// line, default 0); `limit` (optional, how many lines, default
/// [`READ_DEFAULT_LINES`], at least 1); `max_bytes` (optional, how many bytes to
/// read at most, default [`MAX_FILE_BYTES`]).
///
/// Output: `{"path", "size", "bytes", "truncated", "content"}` — `size` is the
/// file's size on disk, `bytes` the text read (before numbering), `content` the
/// numbered lines returned. When lines remain past the window the content ends
/// with `[showing lines 1-2000 of 5312; pass offset to read more]`; when the byte
/// cap ended the read it says so the same way. `truncated` is true in either
/// case, so a partial read never looks complete.
///
/// A directory path returns what [`list`] returns for it. A binary file (a NUL
/// byte in its first 8 KiB) returns `content` = `[binary file, N bytes]`. A file
/// that is not UTF-8 text is refused (`invalid input`, "is not UTF-8 text")
/// rather than returned as mojibake. A cap that lands inside a multi-byte
/// character is not a refusal — the character the cap cut is dropped and
/// `truncated` says so.
pub fn read_file(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = string_arg(args, "path")?;
    let offset = line_arg(args, "offset", 0)?;
    let limit = line_arg(args, "limit", READ_DEFAULT_LINES)?;
    if limit == 0 {
        return Err(invalid("\"limit\" must be at least 1 line"));
    }
    let cap = byte_arg(args, "max_bytes", MAX_FILE_BYTES)?.unwrap_or(MAX_FILE_BYTES);
    let (path, label) = workspace.resolve(&raw)?;
    let metadata =
        fs::metadata(&path).map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    if metadata.is_dir() {
        // A directory has no lines to read; its entries are what `list` reports.
        return list(workspace, &json!({ "path": raw }));
    }

    let file =
        fs::File::open(&path).map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| io_tool_error(err, format!("cannot read {label}")))?;
    let byte_cut = bytes.len() as u64 > cap;
    if byte_cut {
        // The cap is the window; the byte after it is only ever read to learn
        // that the file goes on.
        bytes.truncate(cap as usize);
    }
    if bytes.iter().take(BINARY_PROBE_BYTES).any(|byte| *byte == 0) {
        return Ok(json!({
            "path": label,
            "size": metadata.len(),
            "bytes": 0,
            "truncated": false,
            "content": format!("[binary file, {} bytes]", metadata.len()),
        })
        .to_string());
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // The cap cut the last character in half: the prefix before it is the
        // file's own bytes and the file is valid, so keep them. (`from_utf8`
        // reports that case as an error with no invalid sequence, which is how
        // it is told apart from a file that really is not text.)
        Err(err) if byte_cut && err.utf8_error().error_len().is_none() => {
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

    let total = text.split_inclusive('\n').count();
    let mut content = String::new();
    for (index, line) in text
        .split_inclusive('\n')
        .enumerate()
        .skip(offset)
        .take(limit)
    {
        content.push_str(&numbered_line(
            index + 1,
            line.strip_suffix('\n').unwrap_or(line),
        ));
    }
    let last = offset.saturating_add(limit).min(total);
    let lines_cut = last < total;
    if lines_cut {
        content.push_str(&format!(
            "[showing lines {}-{last} of {total}; pass offset to read more]\n",
            offset + 1
        ));
    } else if offset >= total && total > 0 {
        content.push_str(&format!(
            "[offset {offset} is past the end; the file has {total} lines]\n"
        ));
    }
    if byte_cut {
        content.push_str(&format!(
            "[the file is over the {cap}-byte read cap; the rest is not shown]\n"
        ));
    }
    Ok(json!({
        "path": label,
        "size": metadata.len(),
        "bytes": text.len(),
        "truncated": byte_cut || lines_cut,
        "content": content,
    })
    .to_string())
}

/// `download`: GETs an http(s) URL through the proxy and saves the body as one
/// file inside the workspace.
///
/// Arguments: `url` (required, http or https), `path` (required, relative; the
/// rules of `write_file`). A body over [`crate::egress::DOWNLOAD_CAP`] is refused
/// rather than saved short. Output: `{"path", "bytes", "created"}`.
pub fn download(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let url = egress::http_url_arg(args)?;
    let raw = string_arg(args, "path")?;
    let (path, label) = workspace.resolve(&raw)?;
    refuse_symlink_target(&path, &label)?;
    if fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(invalid(format!(
            "{label} is a directory; download writes one file"
        )));
    }
    let fetched = egress::fetch(&url, egress::DOWNLOAD_CAP)?;
    if fetched.truncated {
        return Err(limit(format!(
            "{url} is over the {}-byte download cap",
            egress::DOWNLOAD_CAP
        )));
    }
    let created = !path.exists();
    if let Some(parent) = path.parent() {
        create_dirs(parent)
            .map_err(|err| io_tool_error(err, format!("cannot create the folders of {label}")))?;
    }
    write_atomic(&path, &fetched.body)
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    Ok(json!({
        "path": label,
        "bytes": fetched.body.len(),
        "created": created,
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
    refuse_symlink_target(&path, &label)?;
    if let Ok(metadata) = fs::metadata(&path)
        && metadata.is_dir()
    {
        return Err(invalid(format!(
            "{label} is a directory; write_file writes one file"
        )));
    }
    let created = !path.exists();
    // `resolve` proved the deepest existing ancestor is inside the workspace,
    // so the missing directories it creates are too.
    if let Some(parent) = path.parent() {
        create_dirs(parent)
            .map_err(|err| io_tool_error(err, format!("cannot create the folders of {label}")))?;
    }
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
    let content = string_arg(args, "content")?;
    let (_path, label, size) = append_bytes(workspace, &string_arg(args, "path")?, &content)?;
    Ok(json!({
        "path": label,
        "bytes": content.len(),
        "size": size,
    })
    .to_string())
}

/// `conversation`: a peer spoke to this agent — the message is appended to this
/// agent's own [`MESSAGES_LOG`], and nothing else in the guest is touched.
///
/// Arguments: `from_role` (required, a non-empty string — the speaker's role),
/// `text` (required, a non-empty string, at most [`MAX_MESSAGE_BYTES`]), and
/// `from_cid` (optional: the speaker's guest CID, when the wire carried one).
/// An argument this tool does not know is ignored, as every tool ignores one.
///
/// The line it appends, exactly:
///
/// ```text
/// <yyyy-mm-ddThh:mm:ssZ> from <role> (cid <N>): <text>
/// ```
///
/// with the `(cid <N>)` part omitted when `from_cid` was not given. The
/// timestamp is **this guest's own clock** in UTC (a guest with no clock writes
/// the epoch, which is what its clock says); the role and the text have every
/// control character escaped (`\x0a` for a newline), so one message is one line
/// and a hostile text cannot forge a line from another agent.
///
/// Output: `{"received": true, "log": "messages.log", "lines": N, "size": M}` —
/// the log's line count and byte size after the append. The peer is not a party
/// to this call: the hub delivered the message, and the agent's receipt is that
/// it wrote it down. A failure is the shared taxonomy and changes nothing: a
/// refused message is not appended, and the log is never part-written (the
/// append is one `write` of the whole line, fsynced, under the same cap and the
/// same symlink refusal as [`append_file`]).
pub fn conversation(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let from_role = string_arg(args, "from_role")?;
    if from_role.is_empty() {
        return Err(invalid("\"from_role\" must not be empty"));
    }
    let text = string_arg(args, "text")?;
    if text.is_empty() {
        return Err(invalid("\"text\" must not be empty"));
    }
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(limit(format!(
            "the text is {} bytes, over the {MAX_MESSAGE_BYTES}-byte conversation cap",
            text.len()
        )));
    }
    let from_cid = match args.get("from_cid") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64() {
            Some(cid) => Some(cid),
            None => return Err(invalid("\"from_cid\" must be a non-negative integer")),
        },
    };
    let line = message_line(now_secs(), &from_role, from_cid, &text);
    let (log, label, size) = append_bytes(workspace, MESSAGES_LOG, &line)?;
    let lines = count_lines(&log)?;
    Ok(json!({
        "received": true,
        "log": label,
        "lines": lines,
        "size": size,
    })
    .to_string())
}

/// One log line for one message: `<iso utc> from <role> (cid <N>): <text>`.
///
/// Split out from [`conversation`] so the line's format is pinned by a test
/// without a clock, a workspace or a peer, and so the escaping is one place:
/// role and text are escaped with the transcript's own rule (see
/// [`crate::session::escape_for_transcript`]), which is what keeps a text that
/// carries a newline from becoming two lines.
fn message_line(secs: u64, from_role: &str, from_cid: Option<u64>, text: &str) -> String {
    let role = escape_for_transcript(from_role);
    let text = escape_for_transcript(text);
    match from_cid {
        Some(cid) => format!("{} from {role} (cid {cid}): {text}\n", iso_utc(secs)),
        None => format!("{} from {role}: {text}\n", iso_utc(secs)),
    }
}

/// This guest's clock, in seconds since the Unix epoch.
///
/// A guest whose clock reads before the epoch (a VM with no time source) is
/// reported as the epoch rather than as an error: the log states the clock this
/// guest has, which is the honest thing for a timestamp to be.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// `secs` since the Unix epoch as an ISO-8601 UTC stamp: `2026-10-11T05:22:31Z`.
///
/// Hand-rolled because the harness's dependency list is deliberately two JSON
/// crates (see `main.rs`), and the arithmetic is closed-form: days since the
/// epoch, then the civil date of those days ([`civil_from_days`]). Pinned by
/// tests against known epochs.
fn iso_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let todo = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        todo / 3600,
        (todo % 3600) / 60,
        todo % 60,
    )
}

/// The `(year, month, day)` of `days` since 1970-01-01 in the proleptic
/// Gregorian calendar (Howard Hinnant's `civil_from_days`).
///
/// `u64` seconds only reach positive days, so the input is an `i64` day count
/// that is never negative in practice; the era arithmetic is the general form
/// anyway, so there is no boundary at which this can panic.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (
        if month <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        month as u32,
        day,
    )
}

/// How many lines the log at `path` holds: the receipt's `lines`.
///
/// Counted over the file's bytes rather than its text — a log an agent wrote
/// something else into with `write_file` still has a line count, and the
/// append that produced this receipt has already happened, so a receipt that
/// failed on its bookkeeping would report a delivery that did land as one that
/// did not (and invite a double append). Every line this tool writes ends in
/// `\n`, so the newline count is the line count.
fn count_lines(path: &Path) -> Result<u64, ToolError> {
    let bytes =
        fs::read(path).map_err(|err| io_tool_error(err, "cannot read the conversation log"))?;
    Ok(bytes.iter().filter(|byte| **byte == b'\n').count() as u64)
}

/// Appends `content` to `relative` in `workspace`, creating the file if needed.
///
/// The one place a file is grown by appending — [`append_file`] and
/// [`conversation`] both come through here — so the symlink refusal, the
/// [`MAX_FILE_BYTES`] cap and the fsynced `O_APPEND` write are the same rules
/// for both. Returns the file's resolved path, the path as a result names it,
/// and the size after the append; a file that would grow past the cap is
/// refused rather than truncated, because an append that silently dropped its
/// tail would be a lie about what is in the file.
fn append_bytes(
    workspace: &Workspace,
    raw: &str,
    content: &str,
) -> Result<(PathBuf, String, u64), ToolError> {
    if content.len() as u64 > MAX_WRITE_BYTES {
        return Err(limit(format!(
            "the content is {} bytes, over the {MAX_WRITE_BYTES}-byte write cap",
            content.len()
        )));
    }
    let (path, label) = workspace.resolve(raw)?;
    refuse_symlink_target(&path, &label)?;
    let existing = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(invalid(format!(
                "{label} is a directory; an append writes one file"
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
    give_to_agent(&path);
    file.write_all(content.as_bytes())
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    file.sync_all()
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    Ok((path, label, size))
}

/// One exact replacement, as `edit_file` and `multi_edit` apply it.
struct Edit {
    old: String,
    new: String,
    replace_all: bool,
}

/// A required text argument under `key`, or under `alias` when only the alias
/// is given (`find` and `replace` are the names the older callers use).
fn edit_text(args: &Value, key: &str, alias: &str) -> Result<String, ToolError> {
    let key = if args.get(key).is_none() && args.get(alias).is_some() {
        alias
    } else {
        key
    };
    string_arg(args, key)
}

/// The edit `args` describe: `old`, `new` and the optional `replace_all`
/// (default false).
///
/// Refused: an empty `old` (it would match everywhere), an `old` over
/// [`MAX_NEEDLE_BYTES`], a `new` over [`MAX_WRITE_BYTES`], and an `old` equal to
/// `new` (an edit that changes nothing is a mistake to report, not a no-op to
/// accept).
fn edit_spec(args: &Value) -> Result<Edit, ToolError> {
    let old = edit_text(args, "old", "find")?;
    let new = edit_text(args, "new", "replace")?;
    let replace_all = match args.get("replace_all") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(invalid("\"replace_all\" must be true or false")),
    };
    if old.is_empty() {
        return Err(invalid("\"old\" must not be empty"));
    }
    if old.len() > MAX_NEEDLE_BYTES {
        return Err(limit(format!(
            "\"old\" is {} bytes, over the {MAX_NEEDLE_BYTES}-byte cap",
            old.len()
        )));
    }
    if new.len() as u64 > MAX_WRITE_BYTES {
        return Err(limit(format!(
            "\"new\" is {} bytes, over the {MAX_WRITE_BYTES}-byte cap",
            new.len()
        )));
    }
    if old == new {
        return Err(invalid(
            "\"old\" and \"new\" are the same; the edit would change nothing",
        ));
    }
    Ok(Edit {
        old,
        new,
        replace_all,
    })
}

/// The refusal for an `old` that does not occur, naming the closest line when
/// there is one: the first line of `old`, trimmed, searched for by a trimmed
/// `contains` over the file's lines.
fn missing_message(content: &str, label: &str, old: &str) -> String {
    let head = old.lines().next().unwrap_or("").trim();
    let closest = if head.is_empty() {
        None
    } else {
        content
            .lines()
            .enumerate()
            .find(|(_, line)| line.trim().contains(head))
    };
    match closest {
        Some((index, line)) => format!(
            "the text to replace does not appear in {label}; closest line is {}: {:?}",
            index + 1,
            line.trim()
        ),
        None => format!("the text to replace does not appear in {label}"),
    }
}

/// Applies `edit` to `content` under the rules every edit shares: `old` must
/// occur, and exactly once unless `replace_all` says otherwise. Returns the
/// updated text, how many replacements were made, and the byte offset of the
/// first one (which the updated text shares with `content` up to that point).
fn replace_exact(
    content: &str,
    label: &str,
    edit: &Edit,
) -> Result<(String, usize, usize), ToolError> {
    let found = content.matches(&edit.old).count();
    if found == 0 {
        return Err(not_found(missing_message(content, label, &edit.old)));
    }
    if found > 1 && !edit.replace_all {
        return Err(invalid(format!(
            "the text to replace appears {found} times in {label}; add surrounding context to make it unique, or set \"replace_all\" to replace all {found}"
        )));
    }
    let first = content.find(&edit.old).unwrap_or(0);
    let updated = if edit.replace_all {
        content.replace(&edit.old, &edit.new)
    } else {
        content.replacen(&edit.old, &edit.new, 1)
    };
    if updated.len() as u64 > MAX_FILE_BYTES {
        return Err(limit(format!(
            "the edit would make {label} {} bytes, over the {MAX_FILE_BYTES}-byte cap",
            updated.len()
        )));
    }
    let replacements = if edit.replace_all { found } else { 1 };
    Ok((updated, replacements, first))
}

/// The three numbered lines around a change at byte offset `at`: the line it
/// starts on, and a line either side where the file has one.
fn snippet(updated: &str, at: usize) -> String {
    let changed = updated[..at].matches('\n').count();
    updated
        .lines()
        .enumerate()
        .skip(changed.saturating_sub(1))
        .take(3)
        .map(|(index, line)| numbered_line(index + 1, line))
        .collect()
}

/// `edit_file`: one exact replacement, and only when it is unambiguous.
///
/// Arguments: `path` (required, relative), `old` (required, a non-empty literal,
/// at most [`MAX_NEEDLE_BYTES`]), `new` (required, may be empty to delete, at most
/// [`MAX_WRITE_BYTES`]), and `replace_all` (optional, default false). `find` and
/// `replace` are accepted as the names of `old` and `new`.
///
/// `old` must occur. Zero occurrences is `not_found`, and the message names the
/// closest line when one contains the first line of `old`. More than one, without
/// `replace_all`, is `invalid input` with the count, telling the caller to add
/// context or to set `replace_all`. `old` equal to `new` is refused.
///
/// Output: `{"path", "replacements", "size", "snippet"}` — `replacements` is the
/// number made, and `snippet` the three numbered lines around the first change
/// (see [`numbered_line`]). The rewrite is atomic, like `write_file`.
pub fn edit_file(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = string_arg(args, "path")?;
    let edit = edit_spec(args)?;
    let (path, label) = workspace.resolve(&raw)?;
    refuse_symlink_target(&path, &label)?;
    let content = read_text(&path, &label)?;
    let (updated, replacements, first) = replace_exact(&content, &label, &edit)?;
    write_atomic(&path, updated.as_bytes())
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    Ok(json!({
        "path": label,
        "replacements": replacements,
        "size": updated.len(),
        "snippet": snippet(&updated, first),
    })
    .to_string())
}

/// `multi_edit`: several exact-string edits to one file, applied in order, all or nothing.
///
/// Arguments: `path` (required, relative) and `edits` (required, an array of 1 to
/// [`MAX_MULTI_EDITS`] objects, each taking `old`, `new` and `replace_all` as
/// [`edit_file`] does). Each edit runs against the text the earlier ones left, under
/// the same rules as `edit_file`.
///
/// Output: `{"path", "edits", "replacements", "size"}`. If any edit fails, nothing is
/// written and the error names its index (`edit 1 refused, nothing written: ...`).
/// The file is written once, atomically, only when every edit has applied.
pub fn multi_edit(workspace: &Workspace, input: &Value) -> Result<String, String> {
    apply_edits(workspace, input).map_err(|err| err.to_string())
}

/// The work of [`multi_edit`], with its refusals as the shared taxonomy.
fn apply_edits(workspace: &Workspace, input: &Value) -> Result<String, ToolError> {
    let raw = string_arg(input, "path")?;
    let specs = edit_list(input)?;
    let (path, label) = workspace.resolve(&raw)?;
    refuse_symlink_target(&path, &label)?;
    let mut content = read_text(&path, &label)?;
    let mut replacements = 0;
    for (index, spec) in specs.iter().enumerate() {
        let (updated, count, _first) = edit_spec(spec)
            .and_then(|edit| replace_exact(&content, &label, &edit))
            .map_err(|err| invalid(format!("edit {index} refused, nothing written: {err}")))?;
        replacements += count;
        content = updated;
    }
    write_atomic(&path, content.as_bytes())
        .map_err(|err| io_tool_error(err, format!("cannot write {label}")))?;
    Ok(json!({
        "path": label,
        "edits": specs.len(),
        "replacements": replacements,
        "size": content.len(),
    })
    .to_string())
}

/// The `edits` array of a `multi_edit`: present, non-empty and within the cap.
fn edit_list(input: &Value) -> Result<&[Value], ToolError> {
    match input.get("edits") {
        Some(Value::Array(items)) if items.is_empty() => {
            Err(invalid("\"edits\" must hold at least one edit"))
        }
        Some(Value::Array(items)) if items.len() > MAX_MULTI_EDITS => Err(limit(format!(
            "{} edits, over the {MAX_MULTI_EDITS}-edit cap",
            items.len()
        ))),
        Some(Value::Array(items)) => Ok(items.as_slice()),
        Some(_) => Err(invalid("\"edits\" must be an array")),
        None => Err(invalid("\"edits\" is required")),
    }
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
        // The harness's state directory is not the model's to see.
        if label == "." && name == PLAZA_DIR {
            continue;
        }
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
        // The harness's state directory is not the model's to search, as `list` does not show it.
        if label == "." && name == PLAZA_DIR {
            continue;
        }
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

    fn read_back(workspace: &Workspace, relative: &str) -> String {
        fs::read_to_string(workspace.root().join(relative)).expect("the file")
    }

    // --- path discipline: the refusals every tool shares -------------------

    #[test]
    fn path_discipline_denies_absolute_and_parent_paths() {
        // An absolute path is denied with its reason, by every path-taking tool.
        {
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
        // A parent component is denied however it is spelled.
        {
            let (workspace, _outside) = scratch("parent");
            write_inside(&workspace, "notes.txt", b"inside\n");
            for path in ["../notes.txt", "a/../../notes.txt", "./../x", "a/..", ".."] {
                let err = refused(&workspace, "read_file", json!({ "path": path }));
                assert!(matches!(err, ToolError::Denied { .. }), "{path}: {err:?}");
                assert!(err.to_string().contains("\"..\""), "{path}: {err}");
            }
        }
    }

    #[test]
    fn dangling_links_are_refused_before_any_write_creates_their_target() {
        // A dangling symlink target is refused by every write.
        {
            // The escape the security review found: `resolve` canonicalizes the
            // deepest EXISTING ancestor, so a link whose target does not exist yet
            // passes the walk — and `open` would then create the target outside
            // the workspace. Every mutating tool must refuse a symlink target.
            let (workspace, outside) = scratch("symlink-dangling");
            let target = outside.join("planted.txt");
            symlink(&target, workspace.root().join("esc")).expect("a dangling symlink");

            for (tool, args) in [
                (
                    "append_file",
                    json!({ "path": "esc", "content": "escaped\n" }),
                ),
                (
                    "write_file",
                    json!({ "path": "esc", "content": "escaped\n" }),
                ),
                (
                    "edit_file",
                    json!({ "path": "esc", "find": "x", "replace": "y" }),
                ),
            ] {
                let err = refused(&workspace, tool, args);
                assert!(
                    matches!(err, ToolError::Denied { .. }),
                    "{tool} through a dangling link: {err:?}"
                );
                assert!(err.to_string().contains("symlink"), "{tool}: {err}");
            }
            assert!(
                !target.exists(),
                "no write may have created the link's target"
            );
        }
        // A dangling link on the way is refused before a write creates its target.
        {
            // `d` names a directory that does not exist yet; the walk cannot
            // canonicalize it, and `create_dir_all` would then make the target.
            let (workspace, outside) = scratch("symlink-dangling-dir");
            symlink(outside.join("missing"), workspace.root().join("d"))
                .expect("a dangling directory link");

            let err = refused(
                &workspace,
                "write_file",
                json!({ "path": "d/x.txt", "content": "x" }),
            );
            assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
            assert!(err.to_string().contains("symlink"), "{err}");
            assert!(!outside.join("missing").exists());
        }
    }

    #[test]
    fn symlinks_out_of_the_workspace_or_into_plaza_are_refused() {
        // A symlink that leaves the workspace is denied by every tool that follows it.
        {
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
        // A symlink into the harness's own state is refused by every tool.
        {
            // `resolve` checked the lexical path for `.plaza` but not the path a
            // link lands on, so `p -> .plaza` reached the harness's state.
            let (workspace, _outside) = scratch("symlink-plaza");
            write_inside(&workspace, ".plaza/state.txt", b"harness state\n");
            symlink(workspace.root().join(".plaza"), workspace.root().join("p"))
                .expect("a link to the state directory");
            symlink(
                workspace.root().join(".plaza/state.txt"),
                workspace.root().join("state"),
            )
            .expect("a link to a state file");

            for (tool, args) in [
                ("read_file", json!({ "path": "p/state.txt" })),
                ("read_file", json!({ "path": "state" })),
                ("list", json!({ "path": "p" })),
                ("write_file", json!({ "path": "p/new.txt", "content": "x" })),
                (
                    "append_file",
                    json!({ "path": "p/state.txt", "content": "x" }),
                ),
                (
                    "edit_file",
                    json!({ "path": "state", "find": "harness", "replace": "y" }),
                ),
            ] {
                let err = refused(&workspace, tool, args);
                assert!(matches!(err, ToolError::Denied { .. }), "{tool}: {err:?}");
                assert!(
                    err.to_string().contains("harness's own state"),
                    "{tool}: {err}"
                );
            }
            // Search does not walk into it through the link either.
            let result = served(&workspace, "search", json!({ "pattern": "harness" }));
            assert_eq!(
                result["matches"].as_array().map(Vec::len),
                Some(0),
                "{result}"
            );
            assert_eq!(read_back(&workspace, ".plaza/state.txt"), "harness state\n");
            assert!(!workspace.root().join(".plaza/new.txt").exists());
        }
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
    fn read_file_numbers_lines_and_serves_windows_and_cuts_long_lines() {
        // Lines are numbered like cat -n, and a read past the default window says what it showed.
        {
            let (workspace, _outside) = scratch("read-numbered");
            write_inside(&workspace, "ab.txt", b"a\nb\n");
            let result = served(&workspace, "read_file", json!({ "path": "ab.txt" }));
            assert_eq!(result["content"], "     1\ta\n     2\tb\n");
            assert_eq!(result["truncated"], false);
            // Past the default window of 2000 lines it says what it showed.
            let long: String = (1..=2500).map(|n| format!("{n}\n")).collect();
            write_inside(&workspace, "many.txt", long.as_bytes());
            let result = served(&workspace, "read_file", json!({ "path": "many.txt" }));
            let content = result["content"].as_str().expect("content");
            assert!(content.contains("  2000\t2000\n"), "{content:?}");
            assert!(!content.contains("  2001\t"), "{content:?}");
            assert!(
                content.ends_with("[showing lines 1-2000 of 2500; pass offset to read more]\n"),
                "{content:?}"
            );
            assert_eq!(result["truncated"], true);
        }
        // `offset` and `limit` serve a line window and say when lines remain.
        {
            let (workspace, _outside) = scratch("read-lines");
            write_inside(&workspace, "notes.txt", b"one\ntwo\nthree\nfour\n");
            let window = |args: Value| served(&workspace, "read_file", args);
            // `offset` is the 0-based first line, and `limit` counts lines.
            let second = window(json!({ "path": "notes.txt", "offset": 1, "limit": 2 }));
            assert_eq!(
                second["content"],
                "     2\ttwo\n     3\tthree\n[showing lines 2-3 of 4; pass offset to read more]\n"
            );
            assert_eq!(second["truncated"], true);
            let tail = window(json!({ "path": "notes.txt", "offset": 2 }));
            assert_eq!(tail["content"], "     3\tthree\n     4\tfour\n");
            assert_eq!(tail["truncated"], false);
            let past = window(json!({ "path": "notes.txt", "offset": 99 }));
            assert!(
                past["content"]
                    .as_str()
                    .expect("content")
                    .contains("past the end"),
                "{past}"
            );
            assert!(matches!(
                refused(
                    &workspace,
                    "read_file",
                    json!({ "path": "notes.txt", "limit": 0 })
                ),
                ToolError::InvalidInput { .. }
            ));
            assert!(matches!(
                refused(
                    &workspace,
                    "read_file",
                    json!({ "path": "notes.txt", "offset": -1 })
                ),
                ToolError::InvalidInput { .. }
            ));
        }
        // A line over 2000 characters is cut and marked.
        {
            let (workspace, _outside) = scratch("read-long-line");
            write_inside(
                &workspace,
                "long.txt",
                format!("{}\nshort\n", "x".repeat(2500)).as_bytes(),
            );
            let result = served(&workspace, "read_file", json!({ "path": "long.txt" }));
            let content = result["content"].as_str().expect("content");
            let first = format!("     1\t{}…\n", "x".repeat(2000));
            assert!(content.starts_with(&first), "{content:?}");
            assert!(content.ends_with("     2\tshort\n"), "{content:?}");
        }
    }

    #[test]
    fn read_file_on_a_directory_returns_its_entries() {
        let (workspace, _outside) = scratch("read-dir");
        write_inside(&workspace, "a.txt", b"1");
        let result = served(&workspace, "read_file", json!({ "path": "." }));
        let names: Vec<&str> = result["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| entry["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, vec!["a.txt"]);
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
        let content = result["content"].as_str().expect("content");
        assert!(content.starts_with("     1\t0123\n"), "{content:?}");
        assert!(content.contains("read cap"), "{content:?}");
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
    fn read_file_reports_a_binary_file_by_its_size_not_its_bytes() {
        let (workspace, _outside) = scratch("read-binary");
        write_inside(&workspace, "blob.bin", &[0xff, 0xfe, 0x00, 0x01]);
        let result = served(&workspace, "read_file", json!({ "path": "blob.bin" }));
        assert_eq!(result["content"], "[binary file, 4 bytes]");
        // Bytes that are neither UTF-8 nor binary (no NUL) are still refused.
        write_inside(&workspace, "latin.txt", &[0xff, 0xfe, b'a']);
        let err = refused(&workspace, "read_file", json!({ "path": "latin.txt" }));
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        assert!(err.to_string().contains("is not UTF-8 text"), "{err}");
    }

    // --- write_file --------------------------------------------------------

    #[test]
    fn the_mount_point_prefix_is_the_workspace() {
        assert_eq!(
            relative_parts("/workspace/a/b.txt").unwrap(),
            ["a", "b.txt"]
        );
        assert!(relative_parts("/workspace").unwrap().is_empty());
        assert!(relative_parts("/workspacex/a").is_err());
        assert!(relative_parts("/etc/passwd").is_err());
    }

    #[test]
    fn write_file_creates_missing_folders() {
        let (workspace, _outside) = scratch("nested-write");
        write_file(
            &workspace,
            &json!({"path": "notes/a/b.md", "content": "hi"}),
        )
        .expect("nested write");
        assert_eq!(
            fs::read_to_string(workspace.root().join("notes/a/b.md")).unwrap(),
            "hi"
        );
    }

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
            "     1\ttwo\n"
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

    // --- append_file -------------------------------------------------------

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

    // --- conversation: the per-agent message log ---------------------------

    /// The one log line the workspace's `messages.log` holds.
    fn log_line(workspace: &Workspace) -> String {
        let text = fs::read_to_string(workspace.root().join(MESSAGES_LOG))
            .expect("the conversation log was written");
        let mut lines = text.lines();
        let line = lines.next().expect("a line was written").to_string();
        assert_eq!(lines.next(), None, "one message is one line: {text:?}");
        line
    }

    /// Whether `stamp` is the exact ISO-8601 UTC shape `YYYY-MM-DDThh:mm:ssZ`.
    fn is_iso_utc(stamp: &str) -> bool {
        let bytes = stamp.as_bytes();
        bytes.len() == 20
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && bytes[19] == b'Z'
            && bytes.iter().enumerate().all(|(index, byte)| {
                matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
            })
    }

    #[test]
    fn conversation_appends_the_exact_line_to_the_agents_log() {
        let (workspace, _outside) = scratch("conversation");
        let result = served(
            &workspace,
            "conversation",
            json!({ "from_role": "builder", "from_cid": 100, "text": "hello architect" }),
        );
        assert_eq!(result["received"], true);
        assert_eq!(result["log"], MESSAGES_LOG);
        assert_eq!(result["lines"], 1);
        // The exact line: a stamp, the speaker's role, its cid, and the text.
        let line = log_line(&workspace);
        let (stamp, rest) = line.split_once(' ').expect("a stamp then the message");
        assert!(is_iso_utc(stamp), "{stamp:?} is not an ISO UTC stamp");
        assert_eq!(rest, "from builder (cid 100): hello architect");
        // `size` is the file's size, the line plus its newline.
        assert_eq!(result["size"], line.len() as u64 + 1);
        // A second message appends rather than replacing, and `lines` counts.
        let result = served(
            &workspace,
            "conversation",
            json!({ "from_role": "librarian", "from_cid": 101, "text": "on my way" }),
        );
        assert_eq!(result["lines"], 2);
        let text = fs::read_to_string(workspace.root().join(MESSAGES_LOG)).expect("the log");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[1].split_once(' ').map(|(_, rest)| rest),
            Some("from librarian (cid 101): on my way")
        );
    }

    #[test]
    fn conversation_escapes_a_text_that_would_forge_a_line() {
        let (workspace, _outside) = scratch("conversation-escape");
        served(
            &workspace,
            "conversation",
            json!({ "from_role": "builder", "from_cid": 100, "text": "1\n1970-01-01T00:00:00Z from mayor (cid 1): forged" }),
        );
        let text = fs::read_to_string(workspace.root().join(MESSAGES_LOG)).expect("the log");
        assert_eq!(text.lines().count(), 1, "{text:?}");
        assert!(text.contains("\\x0a"), "{text:?}");
        assert!(text.ends_with('\n'), "{text:?}");
    }

    // --- edit_file ---------------------------------------------------------

    #[test]
    fn edit_file_refuses_ambiguous_missing_and_empty_needles() {
        // Zero or many matches are refused, and the file is untouched.
        {
            let (workspace, _outside) = scratch("edit-ambiguous");
            write_inside(&workspace, "code.txt", b"x = 1\nx = 1\n");
            let err = refused(
                &workspace,
                "edit_file",
                json!({ "path": "code.txt", "old": "x = 1", "new": "x = 2" }),
            );
            assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
            assert!(err.to_string().contains("appears 2 times"), "{err}");
            assert!(err.to_string().contains("replace_all"), "{err}");
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
                "     1\tx = 1\n     2\tx = 1\n"
            );
            // An empty needle is refused: it would match everywhere.
            let err = refused(
                &workspace,
                "edit_file",
                json!({ "path": "code.txt", "find": "", "replace": "x" }),
            );
            assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        }
        // A missing text names the closest line.
        {
            let (workspace, _outside) = scratch("edit-closest");
            write_inside(
                &workspace,
                "code.txt",
                b"fn main() {\n    let total = 1;\n}\n",
            );
            let err = refused(
                &workspace,
                "edit_file",
                json!({ "path": "code.txt", "old": "let total = 1;\n    return;", "new": "x" }),
            );
            assert!(matches!(err, ToolError::NotFound { .. }), "{err:?}");
            assert!(err.to_string().contains("closest line is 2"), "{err}");
            assert!(err.to_string().contains("let total = 1;"), "{err}");
        }
    }

    #[test]
    fn edit_file_replaces_one_or_every_match_and_refuses_a_no_op() {
        // One unambiguous match is replaced and shown as a snippet.
        {
            let (workspace, _outside) = scratch("edit-ok");
            write_inside(&workspace, "code.txt", b"one\ntwo\nthree\nfour\nfive\n");
            let result = served(
                &workspace,
                "edit_file",
                json!({ "path": "code.txt", "old": "three", "new": "THREE" }),
            );
            assert_eq!(result["replacements"], 1);
            assert_eq!(
                result["snippet"],
                "     2\ttwo\n     3\tTHREE\n     4\tfour\n"
            );
            assert_eq!(
                read_back(&workspace, "code.txt"),
                "one\ntwo\nTHREE\nfour\nfive\n"
            );
        }
        // replace_all replaces every match, and an edit that changes nothing is refused.
        {
            let (workspace, _outside) = scratch("edit-all");
            write_inside(&workspace, "a.txt", b"x\nx\nx\n");
            let err = refused(
                &workspace,
                "edit_file",
                json!({ "path": "a.txt", "old": "x", "new": "y" }),
            );
            assert!(err.to_string().contains("appears 3 times"), "{err}");
            let result = served(
                &workspace,
                "edit_file",
                json!({ "path": "a.txt", "old": "x", "new": "y", "replace_all": true }),
            );
            assert_eq!(result["replacements"], 3);
            assert_eq!(read_back(&workspace, "a.txt"), "y\ny\ny\n");
            // An edit that changes nothing is refused and writes nothing.
            let err = refused(
                &workspace,
                "edit_file",
                json!({ "path": "a.txt", "old": "y", "new": "y" }),
            );
            assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
            assert_eq!(read_back(&workspace, "a.txt"), "y\ny\ny\n");
        }
    }

    // --- multi_edit --------------------------------------------------------

    #[test]
    fn multi_edit_applies_in_order_or_writes_nothing_on_failure() {
        // The edits apply in order and the file is written once.
        {
            let (workspace, _outside) = scratch("multi-ok");
            write_inside(&workspace, "a.txt", b"alpha\nbeta\n");
            let out = multi_edit(
                &workspace,
                &json!({
                    "path": "a.txt",
                    "edits": [
                        { "old": "alpha", "new": "gamma" },
                        { "old": "gamma", "new": "delta", "replace_all": false },
                        { "old": "beta", "new": "BETA" },
                    ],
                }),
            )
            .expect("the edits apply");
            let result: Value = serde_json::from_str(&out).expect("JSON");
            assert_eq!(result["edits"], 3);
            assert_eq!(result["replacements"], 3);
            assert_eq!(read_back(&workspace, "a.txt"), "delta\nBETA\n");
        }
        // A failing edit writes nothing, and an empty edit list is refused.
        {
            let (workspace, _outside) = scratch("multi-fail");
            write_inside(&workspace, "a.txt", b"alpha\nbeta\n");
            let err = multi_edit(
                &workspace,
                &json!({
                    "path": "a.txt",
                    "edits": [
                        { "old": "alpha", "new": "gamma" },
                        { "old": "missing", "new": "x" },
                    ],
                }),
            )
            .expect_err("edit 1 should be refused");
            assert!(err.contains("edit 1"), "{err}");
            assert_eq!(read_back(&workspace, "a.txt"), "alpha\nbeta\n");
            // An empty edit list is refused before the file is touched.
            let err = multi_edit(&workspace, &json!({ "path": "a.txt", "edits": [] }))
                .expect_err("no edits");
            assert!(err.contains("at least one edit"), "{err}");
        }
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
            "conversation",
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
                6 => json!({ "pattern": next().to_string(), "max_bytes": -1, "offset": next() }),
                _ => json!({ "path": format!("f{index}"), "find": "", "replace": null }),
            };
            for name in names {
                let _ = crate::tools::serve(&workspace, name, &value);
            }
        }
    }
}
