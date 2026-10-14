//! The IDE's workspace tools for the owner: the operator's view of an agent's `/workspace`.
//!
//! The hub reaches these through `tell` like every operator tool, and [`crate::tools::serve`]
//! runs them. The owner is the principal, so a write needs no approval, but each write first
//! takes a checkpoint labelled `owner edit: <path>`, and a write that names a stale hash is
//! refused as `conflict:`. Every reply is JSON text.
//!
//! # Paths
//!
//! [`apz_ide::path`] refuses the lexical cases: absolute, `..`, secrets, and writes under
//! `.plaza/`. Then the real filesystem: the deepest existing ancestor is canonicalized (which
//! follows links) and must land inside the canonical workspace root, and the place it lands is
//! checked by the same rules, so a link into a secret or into `.plaza/` is refused too. A write
//! also refuses a target that is itself a symlink. `.plaza/` is readable, never writable, and
//! hidden from listings and search.
//!
//! # Replies
//!
//! Each reply is under [`OUTPUT_BUDGET`], so it fits the operator's output cap. Text is paged
//! in chunks of at most [`CHUNK`] bytes on character boundaries; a chunk is cut shorter when its
//! reply would not fit. Nothing is cut silently: a reply says `done`, `next` or `truncated`.
//!
//! # Runs
//!
//! `ide_run_start` starts `sh -c` in the workspace and returns at once, so a run never holds a
//! tell past its five seconds. Its output is kept as a tail of [`RUN_OUTPUT_CAP`] bytes, read by
//! `ide_run_poll` from an offset. The process group ends at the timeout, on stop, and when the
//! shell exits, so nothing a run started outlives it. At most [`MAX_RUNS`] runs are kept.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_ide::path::{PathError, check_dir, check_read, check_write, is_secret};
use apz_ide::tree::{Entry, EntryKind, PAGE_SIZE, TreePage};
use apz_proto::wire::MAX_TOOL_OUTPUT;
use serde_json::{Value, json};

use crate::bash::{self, Privilege, SHELL};
use crate::checkpoints::{self, Summary};
use crate::files::{self, PLAZA_DIR, Workspace};
use crate::tools::ToolError;

/// Most text one reply carries.
const CHUNK: usize = 48 * 1024;
/// The largest file the IDE reads or writes as text.
const MAX_TEXT: u64 = files::MAX_FILE_BYTES;
/// Bytes sniffed for a NUL when a listing says whether a file is binary.
const BINARY_PROBE: u64 = 8 * 1024;
/// A reply's size cap: the operator's output cap, less room for the envelope.
const OUTPUT_BUDGET: usize = MAX_TOOL_OUTPUT - 1024;
/// The largest diff `ide_changes` renders.
const MAX_DIFF: usize = 8 * 1024 * 1024;
const MAX_SEARCH_MATCHES: usize = 200;
const SEARCH_LINE_CHARS: usize = 200;
const MAX_SEARCH_SCAN: u64 = 20 * 1024 * 1024;
/// Runs kept at once, and how long a finished one stays to be polled.
const MAX_RUNS: usize = 4;
const RUN_KEEP: Duration = Duration::from_secs(10 * 60);
const RUN_DEFAULT_TIMEOUT_S: u64 = 120;
const RUN_MAX_TIMEOUT_S: u64 = 600;
const RUN_OUTPUT_CAP: usize = 1024 * 1024;
const RUN_POLL: Duration = Duration::from_millis(20);
/// How long the output is awaited once the shell and its group are gone.
const RUN_DRAIN: Duration = Duration::from_millis(500);
/// How long a stop waits for the run to end before it answers.
const RUN_STOP_WAIT: Duration = Duration::from_secs(2);

/// `ide_list {path, cursor?}`: one page of a directory, directories first, then by name.
///
/// Output: `{"entries": [{path, kind, size, hash, binary}], "next"}`. `next` is the cursor for the
/// following page, or null. A page holds up to [`PAGE_SIZE`] entries and fewer when the reply
/// would not fit. Secret paths are skipped, and `.plaza` at the root is hidden.
pub fn list(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = files::string_arg(args, "path")?;
    let target = locate(workspace, &raw, check_dir, check_dir)?;
    let metadata = fs::metadata(&target.abs)
        .map_err(|err| files::io_tool_error(err, format!("cannot list {}", shown(&target.rel))))?;
    if !metadata.is_dir() {
        return Err(files::invalid(format!(
            "{} is not a directory",
            shown(&target.rel)
        )));
    }
    let root_listing = target.rel.is_empty();
    let items = fs::read_dir(&target.abs)
        .map_err(|err| files::io_tool_error(err, format!("cannot list {}", shown(&target.rel))))?;
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

    let from = match args.get("cursor") {
        None | Some(Value::Null) => 0,
        Some(Value::String(cursor)) => {
            let key = parse_cursor(cursor)?;
            found.partition_point(|item| key_of(item) <= key)
        }
        Some(_) => return Err(files::invalid("\"cursor\" must be a string")),
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
    serde_json::to_string(&TreePage { entries, next })
        .map_err(|err| internal(format!("cannot encode the listing: {err}")))
}

/// `ide_read {path, offset?}`: a chunk of a text file from byte `offset`.
///
/// Output: `{"path", "size", "hash", "offset", "text", "done"}`. `hash` is the sha256 of the whole
/// file, which a later `ide_write` sends back as `expected_hash`. A file over 1 MiB, a binary
/// file and a file that is not UTF-8 are refused.
pub fn read(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = files::string_arg(args, "path")?;
    let target = locate(workspace, &raw, check_read, check_read)?;
    let metadata = fs::metadata(&target.abs)
        .map_err(|err| files::io_tool_error(err, format!("cannot read {}", target.rel)))?;
    if metadata.is_dir() {
        return Err(files::invalid(format!(
            "{} is a directory; ide_read reads one file",
            target.rel
        )));
    }
    if metadata.len() > MAX_TEXT {
        return Err(files::limit(format!(
            "{} is {} bytes, over the {MAX_TEXT}-byte cap",
            target.rel,
            metadata.len()
        )));
    }
    let bytes = read_capped(&target.abs)
        .map_err(|err| files::io_tool_error(err, format!("cannot read {}", target.rel)))?;
    if bytes.len() as u64 > MAX_TEXT {
        return Err(files::limit(format!(
            "{} grew past the {MAX_TEXT}-byte cap while it was read",
            target.rel
        )));
    }
    let text = text_of(&target.rel, &bytes)?;
    let hash = sha_hex(&bytes);
    let offset = match args.get("offset") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .and_then(|at| usize::try_from(at).ok())
            .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
    };
    if offset > text.len() {
        return Err(files::invalid(format!(
            "offset {offset} is past the end of {} ({} bytes)",
            target.rel,
            text.len()
        )));
    }
    if !text.is_char_boundary(offset) {
        return Err(files::invalid(format!(
            "offset {offset} is inside a character of {}",
            target.rel
        )));
    }
    let reply = |end: usize| {
        json!({
            "path": target.rel,
            "size": text.len(),
            "hash": hash,
            "offset": offset,
            "text": &text[offset..end],
            "done": end == text.len(),
        })
        .to_string()
    };
    let end = chunk_end(text, offset, |end| reply(end).len() <= OUTPUT_BUDGET);
    Ok(reply(end))
}

/// `ide_write {path, text, expected_hash}`: replaces a text file, or creates it.
///
/// `expected_hash` is the file's hash as last read, or `null` for a file that must not exist yet.
/// A mismatch is refused as `conflict: <current hash>` (or `conflict: missing`) and nothing is
/// written or checkpointed. Otherwise the file is checkpointed as `owner edit: <path>` and written
/// atomically. Output: `{"path", "hash", "checkpoint"}`.
pub fn write(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let raw = files::string_arg(args, "path")?;
    let text = files::string_arg(args, "text")?;
    if text.len() as u64 > MAX_TEXT {
        return Err(files::limit(format!(
            "the text is {} bytes, over the {MAX_TEXT}-byte cap",
            text.len()
        )));
    }
    let expected = match args.get("expected_hash") {
        Some(Value::Null) => None,
        Some(Value::String(hash)) => Some(hash.clone()),
        Some(_) => {
            return Err(files::invalid(
                "\"expected_hash\" must be a string, or null for a file that must not exist yet",
            ));
        }
        None => {
            return Err(files::invalid(
                "\"expected_hash\" is required: the hash last read, or null for a file that must not exist yet",
            ));
        }
    };
    let target = locate(workspace, &raw, check_write, check_write)?;
    files::refuse_symlink_target(&target.abs, &target.rel)?;
    let current = match fs::metadata(&target.abs) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(files::invalid(format!(
                "{} is a directory; ide_write writes one file",
                target.rel
            )));
        }
        Ok(_) => Some(
            fs::read(&target.abs)
                .map_err(|err| files::io_tool_error(err, format!("cannot read {}", target.rel)))?,
        ),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(files::io_tool_error(
                err,
                format!("cannot read {}", target.rel),
            ));
        }
    };
    let current_hash = current.as_deref().map(sha_hex);
    let refusal = match (&expected, &current_hash) {
        (None, None) => None,
        (None, Some(now)) => Some(now.clone()),
        (Some(_), None) => Some("missing".to_string()),
        (Some(want), Some(now)) => (want != now).then(|| now.clone()),
    };
    if let Some(reason) = refusal {
        return Err(ToolError::Conflict { reason });
    }

    let checkpoint = checkpoints::snapshot(workspace, &format!("owner edit: {}", target.rel))
        .map_err(internal)?;
    let permissions = fs::metadata(&target.abs)
        .ok()
        .map(|metadata| metadata.permissions());
    if let Some(parent) = target.abs.parent() {
        files::create_dirs(parent).map_err(|err| {
            files::io_tool_error(err, format!("cannot create the folders of {}", target.rel))
        })?;
    }
    files::write_atomic(&target.abs, text.as_bytes())
        .map_err(|err| files::io_tool_error(err, format!("cannot write {}", target.rel)))?;
    if let Some(permissions) = permissions {
        fs::set_permissions(&target.abs, permissions).map_err(|err| {
            files::io_tool_error(err, format!("cannot keep the mode of {}", target.rel))
        })?;
    }
    Ok(json!({
        "path": target.rel,
        "hash": sha_hex(text.as_bytes()),
        "checkpoint": checkpoint,
    })
    .to_string())
}

/// `ide_changes {base?, offset?}`: the unified diff from a base checkpoint to the workspace now.
///
/// `base` is `work_start` (the default: the latest checkpoint a work item took at its start, or
/// the oldest checkpoint when there is none) or `checkpoint:<n>`. Output: `{"base", "checkpoint",
/// "offset", "total", "diff", "done"}`, where `checkpoint` is the base's id and `diff` a chunk of
/// the git-style diff, which `apz_ide::parse_unified` reads. Secret files are left out.
pub fn changes(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let base = match args.get("base") {
        None | Some(Value::Null) => "work_start".to_string(),
        Some(Value::String(base)) => base.clone(),
        Some(_) => {
            return Err(files::invalid(
                "\"base\" must be \"work_start\" or \"checkpoint:<n>\"",
            ));
        }
    };
    let summaries = checkpoints::summaries(workspace);
    let chosen = resolve_base(&summaries, &base)?;
    let diff = checkpoints::unified_since(workspace, chosen.n, |path| !is_secret(path))
        .map_err(internal)?;
    if diff.len() > MAX_DIFF {
        return Err(files::limit(format!(
            "the diff is {} bytes, over the {MAX_DIFF}-byte cap; use a later base",
            diff.len()
        )));
    }
    let offset = match args.get("offset") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .and_then(|at| usize::try_from(at).ok())
            .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
    };
    if offset > diff.len() {
        return Err(files::invalid(format!(
            "offset {offset} is past the end of the diff ({} bytes)",
            diff.len()
        )));
    }
    if !diff.is_char_boundary(offset) {
        return Err(files::invalid(format!(
            "offset {offset} is inside a character of the diff"
        )));
    }
    let reply = |end: usize| {
        json!({
            "base": base,
            "checkpoint": chosen.id,
            "offset": offset,
            "total": diff.len(),
            "diff": &diff[offset..end],
            "done": end == diff.len(),
        })
        .to_string()
    };
    let end = chunk_end(&diff, offset, |end| reply(end).len() <= OUTPUT_BUDGET);
    Ok(reply(end))
}

/// `ide_checkpoints`: every checkpoint, newest first: `{"checkpoints": [{n, label, created_ms, files}]}`.
pub fn list_checkpoints(workspace: &Workspace, _args: &Value) -> Result<String, ToolError> {
    let rows: Vec<Value> = checkpoints::summaries(workspace)
        .iter()
        .rev()
        .map(|s| json!({"n": s.n, "label": s.label, "created_ms": s.at_ms, "files": s.files}))
        .collect();
    Ok(json!({ "checkpoints": rows }).to_string())
}

/// `ide_restore {n}`: takes an `owner restore` checkpoint of the current state, then restores
/// checkpoint `n`. Output: `{"restored": n, "checkpoint": <the owner restore checkpoint>}`.
pub fn restore(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let n = match args.get("n") {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| files::invalid("\"n\" must be a whole number"))?,
        None => return Err(files::invalid("\"n\" is required: the checkpoint number")),
    };
    if !checkpoints::summaries(workspace).iter().any(|s| s.n == n) {
        return Err(files::not_found(format!("no checkpoint {n}")));
    }
    let checkpoint = checkpoints::snapshot(workspace, "owner restore").map_err(internal)?;
    // The new checkpoint may have pruned the oldest one; then there is nothing to roll back to.
    if !checkpoints::summaries(workspace).iter().any(|s| s.n == n) {
        return Err(files::not_found(format!(
            "checkpoint {n} was pruned by the owner restore checkpoint {checkpoint}; nothing was restored"
        )));
    }
    checkpoints::restore(workspace, &json!({ "id": n.to_string() })).map_err(internal)?;
    Ok(json!({ "restored": n, "checkpoint": checkpoint }).to_string())
}

/// `ide_search {q, glob?, max?}`: a plain substring search of the text files.
///
/// Case-insensitive when `q` has no uppercase letters. `glob` matches the path relative to the
/// workspace. `max` defaults to and is capped at [`MAX_SEARCH_MATCHES`]. Binary, secret and
/// `.plaza/` files are skipped, and the scan stops after [`MAX_SEARCH_SCAN`] bytes. Output:
/// `{"matches": [{path, line, text}], "truncated"}`, where `text` is cut to 200 characters.
pub fn search(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let q = files::string_arg(args, "q")?;
    if q.is_empty() {
        return Err(files::invalid("\"q\" must not be empty"));
    }
    if q.len() > files::MAX_NEEDLE_BYTES {
        return Err(files::limit(format!(
            "\"q\" is {} bytes, over the {}-byte cap",
            q.len(),
            files::MAX_NEEDLE_BYTES
        )));
    }
    let glob = match args.get("glob") {
        None | Some(Value::Null) => None,
        Some(Value::String(pattern)) => Some(
            globset::Glob::new(pattern)
                .map_err(|err| files::invalid(format!("\"glob\" is not a glob: {err}")))?
                .compile_matcher(),
        ),
        Some(_) => return Err(files::invalid("\"glob\" must be a string")),
    };
    let max = match args.get("max") {
        None | Some(Value::Null) => MAX_SEARCH_MATCHES,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| files::invalid("\"max\" must be a whole number"))?
            as usize,
    };
    if max == 0 {
        return Err(files::invalid("\"max\" must be at least 1"));
    }
    if max > MAX_SEARCH_MATCHES {
        return Err(files::limit(format!(
            "\"max\" is {max}, over the {MAX_SEARCH_MATCHES}-match cap"
        )));
    }
    let case_blind = q == q.to_lowercase();
    let mut scan = Scan {
        needle: if case_blind { q.to_lowercase() } else { q },
        case_blind,
        glob,
        max,
        matches: Vec::new(),
        used: 64,
        scanned: 0,
        truncated: false,
    };
    let root = workspace.canonical_root()?;
    walk_for_search(&root, "", &mut scan);
    Ok(json!({ "matches": scan.matches, "truncated": scan.truncated }).to_string())
}

/// `ide_run_start {command, timeout_s?}`: starts `sh -c command` in the workspace and returns at once.
///
/// Output: `{"run": id}`. The run is the agent user's, in its own process group, with a cleared
/// environment. `timeout_s` defaults to 120 and is capped at 600. At most [`MAX_RUNS`] runs are
/// kept; a finished one is made room for, and when all are running the start is refused.
pub fn run_start(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    start_in(&REGISTRY, workspace, args)
}

/// `ide_run_poll {run, offset?}`: the run's output from `offset`, at most one chunk.
///
/// Output: `{"run", "offset", "output", "next", "done", "exit_code", "timed_out"}`. `offset` is
/// where `output` starts, which is later than asked when the tail has dropped earlier bytes;
/// `next` is the offset to poll from next. `exit_code` is null while running or when a signal
/// ended the run.
pub fn run_poll(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    poll_in(&REGISTRY, args)
}

/// `ide_run_stop {run}`: kills the run's process group and waits briefly for it to end.
/// Output: `{"run", "stopped", "done"}`, where `stopped` is true when the run was still running.
pub fn run_stop(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    stop_in(&REGISTRY, args)
}

// ---------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------

/// A path the owner's tools may touch: its cleaned name, and where it is.
pub(crate) struct Target {
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
}

/// The entry a listing sorts and pages by: its path, name and kind.
struct Listed {
    rel: String,
    name: String,
    kind: EntryKind,
    size: u64,
}

/// Checks `raw` with `lexical`, then the real filesystem: the deepest existing ancestor is
/// canonicalized and must land inside the canonical workspace root, and the place it lands is
/// checked with `landed`.
pub(crate) fn locate(
    workspace: &Workspace,
    raw: &str,
    lexical: fn(&str) -> Result<String, PathError>,
    landed: fn(&str) -> Result<String, PathError>,
) -> Result<Target, ToolError> {
    let rel = lexical(raw).map_err(|err| path_refusal(raw, err))?;
    let root = workspace.canonical_root()?;
    let mut ancestor = root.join(&rel);
    let resolved = loop {
        match fs::canonicalize(&ancestor) {
            Ok(resolved) => break resolved,
            Err(err) if err.kind() == io::ErrorKind::NotFound => match ancestor.parent() {
                Some(parent) => ancestor = parent.to_path_buf(),
                None => return Err(files::denied(format!("{raw:?} cannot be resolved"))),
            },
            Err(err) => return Err(files::io_tool_error(err, format!("cannot resolve {raw:?}"))),
        }
    };
    let Ok(inside) = resolved.strip_prefix(&root) else {
        return Err(files::denied(format!(
            "{raw:?} resolves to {} outside the workspace",
            resolved.display()
        )));
    };
    let inside = inside.to_string_lossy();
    if !inside.is_empty() {
        landed(&inside).map_err(|err| path_refusal(raw, err))?;
    }
    Ok(Target {
        abs: root.join(&rel),
        rel,
    })
}

/// The refusal for a path the rules name. Malformed input is invalid; the rest are denied.
fn path_refusal(raw: &str, err: PathError) -> ToolError {
    match err {
        PathError::Empty | PathError::Nul | PathError::Control => {
            files::invalid(format!("{raw:?}: {err}"))
        }
        _ => files::denied(format!("{raw:?} refused: {err}")),
    }
}

fn shown(rel: &str) -> &str {
    if rel.is_empty() { "." } else { rel }
}

fn internal(reason: impl Into<String>) -> ToolError {
    ToolError::Internal {
        reason: reason.into(),
    }
}

fn sha_hex(bytes: &[u8]) -> String {
    checkpoints::sha_hex(bytes)
}

/// The bytes of a file, read at most one byte past the cap so an over-cap file is seen.
fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_TEXT + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The bytes as text, or a refusal: the IDE opens text only.
fn text_of<'a>(rel: &str, bytes: &'a [u8]) -> Result<&'a str, ToolError> {
    if bytes.contains(&0) {
        return Err(files::invalid(format!(
            "{rel} is binary (it holds a NUL byte); the IDE opens text only"
        )));
    }
    std::str::from_utf8(bytes)
        .map_err(|_| files::invalid(format!("{rel} is not UTF-8 text; the IDE opens text only")))
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

// ---------------------------------------------------------------------------------------------
// Arguments, cursors and chunks
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

fn parse_cursor(cursor: &str) -> Result<(u8, String, String), ToolError> {
    let (is_dir, name) = match (cursor.strip_prefix('0'), cursor.strip_prefix('1')) {
        (Some(name), _) => (true, name),
        (None, Some(name)) => (false, name),
        (None, None) => return Err(files::invalid("\"cursor\" is not a cursor from ide_list")),
    };
    if name.is_empty() || name.contains('/') {
        return Err(files::invalid("\"cursor\" is not a cursor from ide_list"));
    }
    Ok(display_key(is_dir, name))
}

/// The end of the chunk of `text` that starts at `start`: at most [`CHUNK`] bytes, on a character
/// boundary, and short enough that `fits` (the reply around the chunk) is within the budget.
fn chunk_end(text: &str, start: usize, fits: impl Fn(usize) -> bool) -> usize {
    let mut end = floor_boundary(text, (start + CHUNK).min(text.len()));
    while end > start && !fits(end) {
        end = floor_boundary(text, start + (end - start) / 2);
    }
    end
}

/// The largest index at most `at` that starts a character of `text`.
fn floor_boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The byte-level twin of [`chunk_end`] for run output: a cut never splits a character.
fn byte_chunk_end(bytes: &[u8], fits: impl Fn(usize) -> bool) -> usize {
    let mut end = bytes.len().min(CHUNK);
    loop {
        while end > 0 && end < bytes.len() && is_continuation(bytes[end]) {
            end -= 1;
        }
        if end == 0 || fits(end) {
            return end;
        }
        end /= 2;
    }
}

fn is_continuation(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

/// The base of a `base` argument: the checkpoint it names.
fn resolve_base<'a>(summaries: &'a [Summary], base: &str) -> Result<&'a Summary, ToolError> {
    if summaries.is_empty() {
        return Err(files::not_found("no checkpoints yet"));
    }
    if base == "work_start" {
        // Checkpoints are oldest first: the latest work start, else the oldest checkpoint.
        return Ok(summaries
            .iter()
            .rev()
            .find(|s| s.label.starts_with("work-"))
            .unwrap_or(&summaries[0]));
    }
    let n = base
        .strip_prefix("checkpoint:")
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| {
            files::invalid(format!(
                "\"base\" must be \"work_start\" or \"checkpoint:<n>\", not {base:?}"
            ))
        })?;
    summaries
        .iter()
        .find(|s| s.n == n)
        .ok_or_else(|| files::not_found(format!("no checkpoint {n}")))
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

struct Scan {
    needle: String,
    case_blind: bool,
    glob: Option<globset::GlobMatcher>,
    max: usize,
    matches: Vec<Value>,
    /// The reply's size so far, against [`OUTPUT_BUDGET`].
    used: usize,
    scanned: u64,
    truncated: bool,
}

fn walk_for_search(root: &Path, dir: &str, scan: &mut Scan) {
    let Ok(items) = fs::read_dir(if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    }) else {
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
        let path = root.join(&rel);
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
    if metadata.len() > MAX_TEXT {
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
        let shown: String = line.chars().take(SEARCH_LINE_CHARS).collect();
        let row = json!({ "path": rel, "line": index + 1, "text": shown });
        let cost = row.to_string().len() + 1;
        if scan.used + cost > OUTPUT_BUDGET {
            scan.truncated = true;
            return;
        }
        scan.used += cost;
        scan.matches.push(row);
    }
}

// ---------------------------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------------------------

/// The runs this harness holds. One process, one workspace: a static is enough.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    next: 0,
    runs: BTreeMap::new(),
});

#[derive(Default)]
struct Registry {
    next: u64,
    runs: BTreeMap<String, Arc<Run>>,
}

/// One run: its output tail, how it ended, and whether a stop was asked for.
#[derive(Default)]
struct Run {
    output: Mutex<Tail>,
    state: Mutex<RunState>,
    stop: AtomicBool,
}

/// The last [`RUN_OUTPUT_CAP`] bytes of a run's output, and the offset of the first.
#[derive(Default)]
struct Tail {
    bytes: Vec<u8>,
    base: u64,
}

#[derive(Default)]
enum RunState {
    #[default]
    Running,
    Done {
        exit_code: Option<i32>,
        timed_out: bool,
        at: Instant,
    },
}

impl Run {
    fn append(&self, bytes: &[u8]) {
        let mut tail = locked(&self.output);
        tail.bytes.extend_from_slice(bytes);
        if tail.bytes.len() > RUN_OUTPUT_CAP {
            let dropped = tail.bytes.len() - RUN_OUTPUT_CAP;
            tail.bytes.drain(..dropped);
            tail.base += dropped as u64;
        }
    }

    fn finish(&self, exit_code: Option<i32>, timed_out: bool) {
        *locked(&self.state) = RunState::Done {
            exit_code,
            timed_out,
            at: Instant::now(),
        };
    }

    fn is_running(&self) -> bool {
        matches!(*locked(&self.state), RunState::Running)
    }

    /// Finished, and longer ago than [`RUN_KEEP`].
    fn expired(&self, now: Instant) -> bool {
        matches!(*locked(&self.state), RunState::Done { at, .. } if now.duration_since(at) >= RUN_KEEP)
    }

    fn finished_at(&self) -> Option<Instant> {
        match *locked(&self.state) {
            RunState::Running => None,
            RunState::Done { at, .. } => Some(at),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn start_in(
    registry: &Mutex<Registry>,
    workspace: &Workspace,
    args: &Value,
) -> Result<String, ToolError> {
    let command = match args.get("command") {
        Some(Value::String(command)) if !command.trim().is_empty() => command.clone(),
        _ => return Err(files::invalid("\"command\" must be a non-empty string")),
    };
    let timeout_s = match args.get("timeout_s") {
        None | Some(Value::Null) => RUN_DEFAULT_TIMEOUT_S,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| files::invalid("\"timeout_s\" must be a whole number of seconds"))?,
    };
    if timeout_s == 0 {
        return Err(files::invalid("\"timeout_s\" must be at least 1"));
    }
    if timeout_s > RUN_MAX_TIMEOUT_S {
        return Err(files::limit(format!(
            "\"timeout_s\" is {timeout_s}, over the {RUN_MAX_TIMEOUT_S}-second cap"
        )));
    }

    let mut registry = locked(registry);
    let now = Instant::now();
    registry.runs.retain(|_, run| !run.expired(now));
    if registry.runs.len() >= MAX_RUNS {
        let oldest_finished = registry
            .runs
            .iter()
            .filter_map(|(id, run)| run.finished_at().map(|at| (at, id.clone())))
            .min();
        match oldest_finished {
            Some((_, id)) => {
                registry.runs.remove(&id);
            }
            None => {
                return Err(files::limit(format!(
                    "{MAX_RUNS} runs are running; stop one first"
                )));
            }
        }
    }

    let (child, reader) = spawn(workspace, &command)?;
    registry.next += 1;
    let id = format!("run-{}", registry.next);
    let run = Arc::new(Run::default());
    registry.runs.insert(id.clone(), Arc::clone(&run));
    supervise(run, child, reader, now + Duration::from_secs(timeout_s));
    Ok(json!({ "run": id }).to_string())
}

/// Starts `command` in the workspace, its output on one pipe. The builder is dropped after the
/// spawn so the pipe's write ends close in this process and the reader sees end of file.
fn spawn(workspace: &Workspace, command: &str) -> Result<(Child, io::PipeReader), ToolError> {
    let (reader, writer) =
        io::pipe().map_err(|err| internal(format!("cannot open the output pipe: {err}")))?;
    let stderr = writer
        .try_clone()
        .map_err(|err| internal(format!("cannot open the output pipe: {err}")))?;
    let mut builder = bash::command(SHELL, &["-c", command], workspace.dir(), Privilege::Agent)
        .map_err(internal)?;
    builder.stdout(writer).stderr(stderr);
    let child = builder
        .spawn()
        .map_err(|err| internal(format!("cannot start the shell: {err}")))?;
    drop(builder);
    Ok((child, reader))
}

/// Reads the run's output into its tail, and ends the run at its exit, its timeout or a stop.
/// The process group is killed on every end, and the run is finished once the output has drained.
fn supervise(run: Arc<Run>, mut child: Child, reader: io::PipeReader, deadline: Instant) {
    let (drained_tx, drained) = mpsc::channel::<()>();
    let sink = Arc::clone(&run);
    thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => sink.append(&chunk[..read]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = drained_tx.send(());
    });
    thread::spawn(move || {
        let (exit_code, timed_out) = loop {
            match child.try_wait() {
                Ok(Some(status)) => break (status.code(), false),
                Ok(None) if run.stop.load(Ordering::SeqCst) => {
                    bash::kill_group(&mut child);
                    break (child.wait().ok().and_then(|status| status.code()), false);
                }
                Ok(None) if Instant::now() >= deadline => {
                    bash::kill_group(&mut child);
                    break (child.wait().ok().and_then(|status| status.code()), true);
                }
                Ok(None) => thread::sleep(RUN_POLL),
                Err(_) => break (None, false),
            }
        };
        // Whatever the shell left running in its group goes too, so the output can end.
        bash::kill_group(&mut child);
        let _ = drained.recv_timeout(RUN_DRAIN);
        run.finish(exit_code, timed_out);
    });
}

fn lookup(registry: &Mutex<Registry>, id: &str) -> Result<Arc<Run>, ToolError> {
    locked(registry)
        .runs
        .get(id)
        .cloned()
        .ok_or_else(|| files::not_found(format!("no run {id}")))
}

fn poll_in(registry: &Mutex<Registry>, args: &Value) -> Result<String, ToolError> {
    let id = files::string_arg(args, "run")?;
    let offset = match args.get("offset") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
    };
    let run = lookup(registry, &id)?;
    let (done, exit_code, timed_out) = match *locked(&run.state) {
        RunState::Running => (false, None, false),
        RunState::Done {
            exit_code,
            timed_out,
            ..
        } => (true, exit_code, timed_out),
    };
    let tail = locked(&run.output);
    let total = tail.base + tail.bytes.len() as u64;
    if offset > total {
        return Err(files::invalid(format!(
            "offset {offset} is past the end of the output ({total} bytes)"
        )));
    }
    // Where the output starts: the offset, or the oldest byte still kept. A start inside a
    // character (a cut in the tail, or an offset given mid-character) moves on to the next one.
    let mut skip = (offset.max(tail.base) - tail.base) as usize;
    let limit = skip + 3;
    while skip < tail.bytes.len() && skip < limit && is_continuation(tail.bytes[skip]) {
        skip += 1;
    }
    let available = &tail.bytes[skip..];
    let start = tail.base + skip as u64;
    let reply = |chunk: &[u8]| {
        json!({
            "run": id,
            "offset": start,
            "output": String::from_utf8_lossy(chunk),
            "next": start + chunk.len() as u64,
            "done": done,
            "exit_code": exit_code,
            "timed_out": timed_out,
        })
        .to_string()
    };
    let end = byte_chunk_end(available, |end| {
        reply(&available[..end]).len() <= OUTPUT_BUDGET
    });
    Ok(reply(&available[..end]))
}

fn stop_in(registry: &Mutex<Registry>, args: &Value) -> Result<String, ToolError> {
    let id = files::string_arg(args, "run")?;
    let run = lookup(registry, &id)?;
    let was_running = run.is_running();
    run.stop.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + RUN_STOP_WAIT;
    while run.is_running() && Instant::now() < deadline {
        thread::sleep(RUN_POLL);
    }
    Ok(json!({ "run": id, "stopped": was_running, "done": !run.is_running() }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools;
    use apz_ide::diff::parse_unified;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> (PathBuf, Workspace) {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-ide-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        let workspace = Workspace::at(&dir);
        (dir, workspace)
    }

    fn ok(result: Result<String, ToolError>) -> Value {
        let text = result.expect("the tool succeeds");
        assert!(
            text.len() <= MAX_TOOL_OUTPUT,
            "a reply of {} bytes is over the cap",
            text.len()
        );
        serde_json::from_str(&text).expect("the reply is JSON")
    }

    fn denied(result: Result<String, ToolError>) -> String {
        match result.expect_err("the tool refuses") {
            ToolError::Denied { reason } => reason,
            other => panic!("expected a denial, got {other}"),
        }
    }

    #[test]
    fn refused_paths_are_refused_by_the_rules_and_by_the_real_filesystem() {
        let (dir, ws) = scratch("paths");
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
                denied(read(&ws, &json!({ "path": raw }))).contains("refused"),
                "read {raw}"
            );
            assert!(
                denied(write(
                    &ws,
                    &json!({ "path": raw, "text": "x", "expected_hash": null })
                ))
                .contains("refused"),
                "write {raw}"
            );
        }
        assert!(
            denied(write(
                &ws,
                &json!({ "path": ".plaza/state", "text": "x", "expected_hash": null })
            ))
            .contains(".plaza")
        );
        assert!(denied(read(&ws, &json!({ "path": ".env" }))).contains("secret"));
        assert!(
            denied(write(
                &ws,
                &json!({ "path": ".env", "text": "x", "expected_hash": null })
            ))
            .contains("secret")
        );
        assert!(denied(list(&ws, &json!({ "path": ".ssh" }))).contains("secret"));
        assert!(
            denied(read(&ws, &json!({ "path": "esc/secret.txt" })))
                .contains("outside the workspace")
        );
        assert!(denied(list(&ws, &json!({ "path": "esc" }))).contains("outside the workspace"));
        assert!(
            denied(write(
                &ws,
                &json!({ "path": "esc/new.txt", "text": "x", "expected_hash": null })
            ))
            .contains("outside")
        );
        assert!(
            denied(write(
                &ws,
                &json!({ "path": "dangle", "text": "x", "expected_hash": null })
            ))
            .contains("symlink")
        );
        assert!(
            denied(read(&ws, &json!({ "path": "link.txt" }))).contains("secret"),
            "a link to a secret is a secret"
        );
        assert!(!outside.join("new.txt").exists() && !outside.join("missing.txt").exists());
    }

    #[test]
    fn listing_pages_500_entries_with_directories_first_and_hides_secrets() {
        let (dir, ws) = scratch("list");
        fs::create_dir_all(dir.join("d1")).unwrap();
        fs::create_dir_all(dir.join("d2")).unwrap();
        fs::write(dir.join(".env"), "x").unwrap();
        fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        for i in 0..1100 {
            fs::write(dir.join(format!("f{i:04}")), "x").unwrap();
        }
        let mut names = Vec::new();
        let mut cursor = Value::Null;
        let mut pages = 0;
        loop {
            let args = if cursor.is_null() {
                json!({ "path": "" })
            } else {
                json!({ "path": "", "cursor": cursor })
            };
            let page = ok(list(&ws, &args));
            pages += 1;
            let entries = page["entries"].as_array().unwrap();
            assert!(entries.len() <= PAGE_SIZE);
            names.extend(
                entries
                    .iter()
                    .map(|e| e["path"].as_str().unwrap().to_string()),
            );
            if page["next"].is_null() {
                break;
            }
            cursor = page["next"].clone();
        }
        assert_eq!(pages, 3, "1103 entries in pages of 500");
        assert_eq!(names.len(), 1103);
        assert_eq!(&names[..2], ["d1", "d2"], "directories first");
        assert_eq!(names[2], "bin.dat");
        assert_eq!(names[3], "f0000");
        assert!(!names.iter().any(|n| n == ".env" || n == ".plaza"));
        let first = ok(list(&ws, &json!({ "path": "" })));
        assert_eq!(first["entries"][2]["binary"], json!(true));
        assert_eq!(first["entries"][3]["binary"], json!(false));
        assert!(first["entries"][3]["hash"].is_null());
        assert!(
            list(&ws, &json!({ "path": "bin.dat" })).is_err(),
            "a file is not a directory"
        );
        assert!(list(&ws, &json!({ "path": "", "cursor": "x" })).is_err());
    }

    #[test]
    fn reads_are_chunked_on_character_boundaries_and_refuse_binary_and_huge_files() {
        let (dir, ws) = scratch("read");
        let content = "é".repeat(60_000);
        fs::write(dir.join("u.txt"), &content).unwrap();
        let mut offset = 0;
        let mut collected = String::new();
        let mut chunks = 0;
        loop {
            let reply = ok(read(&ws, &json!({ "path": "u.txt", "offset": offset })));
            let text = reply["text"].as_str().unwrap();
            assert!(text.len() <= CHUNK);
            assert_eq!(reply["hash"], json!(sha_hex(content.as_bytes())));
            assert_eq!(reply["size"], json!(content.len()));
            collected.push_str(text);
            offset += text.len();
            chunks += 1;
            if reply["done"] == json!(true) {
                break;
            }
        }
        assert!(chunks >= 3);
        assert_eq!(collected, content);
        assert!(
            read(&ws, &json!({ "path": "u.txt", "offset": 1 })).is_err(),
            "inside a character"
        );
        assert!(
            read(&ws, &json!({ "path": "u.txt", "offset": 1_000_000 })).is_err(),
            "past the end"
        );

        fs::write(dir.join("bin.dat"), b"a\0b").unwrap();
        assert!(denied_or_invalid(read(&ws, &json!({ "path": "bin.dat" }))).contains("binary"));
        fs::File::create(dir.join("huge.txt"))
            .unwrap()
            .set_len(MAX_TEXT + 1)
            .unwrap();
        match read(&ws, &json!({ "path": "huge.txt" })).expect_err("over 1 MiB") {
            ToolError::LimitExceeded { reason } => assert!(reason.contains("cap"), "{reason}"),
            other => panic!("expected the size refusal, got {other}"),
        }
    }

    fn denied_or_invalid(result: Result<String, ToolError>) -> String {
        match result.expect_err("refused") {
            ToolError::InvalidInput { reason } | ToolError::Denied { reason } => reason,
            other => panic!("expected a refusal, got {other}"),
        }
    }

    #[test]
    fn writes_check_the_hash_and_take_an_owner_edit_checkpoint() {
        let (dir, ws) = scratch("write");
        let first = ok(write(
            &ws,
            &json!({ "path": "docs/a.txt", "text": "one\n", "expected_hash": null }),
        ));
        assert_eq!(first["hash"], json!(sha_hex(b"one\n")));
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "one\n");
        let labels: Vec<String> = checkpoints::summaries(&ws)
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert!(
            labels.contains(&"owner edit: docs/a.txt".to_string()),
            "{labels:?}"
        );
        assert_eq!(
            first["checkpoint"].as_str().unwrap(),
            checkpoints::summaries(&ws).last().unwrap().id
        );

        let before = checkpoints::summaries(&ws).len();
        // A file that exists must not be created again.
        match write(
            &ws,
            &json!({ "path": "docs/a.txt", "text": "two\n", "expected_hash": null }),
        )
        .expect_err("exists")
        {
            ToolError::Conflict { reason } => assert_eq!(reason, sha_hex(b"one\n")),
            other => panic!("expected a conflict, got {other}"),
        }
        // A stale hash.
        let stale = sha_hex(b"zero\n");
        let text = write(
            &ws,
            &json!({ "path": "docs/a.txt", "text": "two\n", "expected_hash": stale }),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(text, format!("conflict: {}", sha_hex(b"one\n")));
        // A missing file that a hash names.
        let text = write(
            &ws,
            &json!({ "path": "docs/none.txt", "text": "x", "expected_hash": stale }),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(text, "conflict: missing");
        assert_eq!(
            checkpoints::summaries(&ws).len(),
            before,
            "a refused write takes no checkpoint"
        );
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "one\n");

        let second = ok(write(
            &ws,
            &json!({ "path": "docs/a.txt", "text": "two\n", "expected_hash": sha_hex(b"one\n") }),
        ));
        assert_eq!(second["hash"], json!(sha_hex(b"two\n")));
        assert_eq!(fs::read_to_string(dir.join("docs/a.txt")).unwrap(), "two\n");
        let names: Vec<String> = fs::read_dir(dir.join("docs"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.txt"], "the temp file is renamed away");

        let big = "x".repeat(MAX_TEXT as usize + 1);
        assert!(matches!(
            write(
                &ws,
                &json!({ "path": "big.txt", "text": big, "expected_hash": null })
            ),
            Err(ToolError::LimitExceeded { .. })
        ));
        assert!(
            write(&ws, &json!({ "path": "y.txt", "text": "y" })).is_err(),
            "expected_hash is required"
        );
    }

    #[test]
    fn changes_parse_with_the_shared_parser_and_leave_secrets_out() {
        let (dir, ws) = scratch("changes");
        fs::write(
            dir.join("keep.txt"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        fs::write(dir.join("bin.dat"), b"a\0b").unwrap();
        fs::write(dir.join(".env"), "TOKEN=old\n").unwrap();
        checkpoints::snapshot(&ws, "work-7").unwrap();

        fs::write(
            dir.join("keep.txt"),
            "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        fs::remove_file(dir.join("gone.txt")).unwrap();
        fs::create_dir_all(dir.join("sub dir")).unwrap();
        fs::write(dir.join("sub dir/new file.txt"), "fresh\n").unwrap();
        fs::write(dir.join("bin.dat"), b"c\0d").unwrap();
        fs::write(dir.join(".env"), "TOKEN=new\n").unwrap();

        let reply = ok(changes(&ws, &json!({})));
        assert_eq!(reply["base"], json!("work_start"));
        assert_eq!(reply["done"], json!(true));
        let diff = reply["diff"].as_str().unwrap();
        assert!(!diff.contains("TOKEN"), "secrets are left out");
        let parsed = parse_unified(diff).expect("the shared parser reads the diff");
        let by_path: BTreeMap<&str, &apz_ide::FileChange> =
            parsed.files.iter().map(|f| (f.path.as_str(), f)).collect();
        assert_eq!(by_path.len(), 4, "{by_path:?}");
        let keep = by_path["keep.txt"];
        assert_eq!(keep.status, apz_ide::Status::Modified);
        assert_eq!(keep.hunks.len(), 1);
        assert_eq!(by_path["gone.txt"].status, apz_ide::Status::Deleted);
        assert_eq!(
            by_path["sub dir/new file.txt"].status,
            apz_ide::Status::Added
        );
        assert!(by_path["bin.dat"].binary);
        assert_eq!(by_path["bin.dat"].status, apz_ide::Status::Modified);

        // The same diff by a checkpoint number, and by its absence.
        let n = checkpoints::summaries(&ws)
            .iter()
            .find(|s| s.label == "work-7")
            .unwrap()
            .n;
        let by_number = ok(changes(&ws, &json!({ "base": format!("checkpoint:{n}") })));
        assert_eq!(by_number["diff"], reply["diff"]);
        assert!(changes(&ws, &json!({ "base": "nope" })).is_err());
        assert!(changes(&ws, &json!({ "base": "checkpoint:999999" })).is_err());
    }

    #[test]
    fn a_long_diff_pages_in_chunks_that_join_back_into_the_whole() {
        let (dir, ws) = scratch("changes-pages");
        let old: String = (0..6000)
            .map(|i| format!("line {i} with some padding text\n"))
            .collect();
        fs::write(dir.join("big.txt"), &old).unwrap();
        checkpoints::snapshot(&ws, "work-1").unwrap();
        let new: String = (0..6000)
            .map(|i| format!("LINE {i} with some padding text\n"))
            .collect();
        fs::write(dir.join("big.txt"), &new).unwrap();

        let mut offset = 0;
        let mut joined = String::new();
        let mut total: usize;
        loop {
            let reply = ok(changes(&ws, &json!({ "offset": offset })));
            total = reply["total"].as_u64().unwrap() as usize;
            let piece = reply["diff"].as_str().unwrap();
            joined.push_str(piece);
            offset += piece.len();
            if reply["done"] == json!(true) {
                break;
            }
        }
        assert_eq!(joined.len(), total);
        assert!(total > CHUNK, "the diff spans several chunks");
        let parsed = parse_unified(&joined).expect("the joined diff parses");
        assert_eq!(parsed.files[0].path, "big.txt");
        assert!(parsed.files[0].hunks.iter().any(|h| !h.lines.is_empty()));
    }

    #[test]
    fn restore_round_trips_and_keeps_an_owner_restore_checkpoint() {
        let (dir, ws) = scratch("restore");
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        let n = {
            checkpoints::snapshot(&ws, "work-3").unwrap();
            checkpoints::summaries(&ws).last().unwrap().n
        };
        ok(write(
            &ws,
            &json!({ "path": "a.txt", "text": "two\n", "expected_hash": sha_hex(b"one\n") }),
        ));
        fs::write(dir.join("b.txt"), "extra\n").unwrap();

        let reply = ok(restore(&ws, &json!({ "n": n })));
        assert_eq!(reply["restored"], json!(n));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
        assert!(!dir.join("b.txt").exists());
        let labels: Vec<String> = checkpoints::summaries(&ws)
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert!(labels.contains(&"owner restore".to_string()), "{labels:?}");
        assert_eq!(
            reply["checkpoint"].as_str().unwrap(),
            checkpoints::summaries(&ws)
                .iter()
                .find(|s| s.label == "owner restore")
                .unwrap()
                .id
        );
        assert!(matches!(
            restore(&ws, &json!({ "n": 999_999 })),
            Err(ToolError::NotFound { .. })
        ));
    }

    #[test]
    fn the_checkpoint_list_is_newest_first() {
        let (dir, ws) = scratch("checkpoint-list");
        fs::write(dir.join("a.txt"), "1").unwrap();
        checkpoints::snapshot(&ws, "work-1").unwrap();
        fs::write(dir.join("a.txt"), "2").unwrap();
        checkpoints::snapshot(&ws, "work-2").unwrap();
        let rows = ok(list_checkpoints(&ws, &json!({})));
        let rows = rows["checkpoints"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["label"], json!("work-2"));
        assert_eq!(rows[1]["label"], json!("work-1"));
        assert_eq!(rows[0]["files"], json!(1));
        assert!(rows[0]["created_ms"].as_u64().unwrap() > 0);
    }

    #[test]
    fn search_is_bounded_and_skips_binary_secret_and_plaza_files() {
        let (dir, ws) = scratch("search");
        let lines: String = (0..250).map(|i| format!("needle line {i}\n")).collect();
        fs::write(dir.join("many.txt"), lines).unwrap();
        fs::write(dir.join("case.txt"), "NEEDLE upper\nneedle lower\n").unwrap();
        fs::write(dir.join("bin.dat"), b"needle\0bin").unwrap();
        fs::write(dir.join(".env"), "needle secret\n").unwrap();
        fs::create_dir_all(dir.join(".plaza")).unwrap();
        fs::write(dir.join(".plaza/state"), "needle internal\n").unwrap();
        fs::write(dir.join("long.md"), format!("{}\n", "needle ".repeat(200))).unwrap();

        let capped = ok(search(&ws, &json!({ "q": "needle", "glob": "many.txt" })));
        assert_eq!(
            capped["matches"].as_array().unwrap().len(),
            MAX_SEARCH_MATCHES
        );
        assert_eq!(capped["truncated"], json!(true));
        assert_eq!(capped["matches"][0]["line"], json!(1));

        let blind = ok(search(&ws, &json!({ "q": "needle", "glob": "case.txt" })));
        assert_eq!(
            blind["matches"].as_array().unwrap().len(),
            2,
            "lowercase q is case-insensitive"
        );
        let exact = ok(search(&ws, &json!({ "q": "NEEDLE", "glob": "case.txt" })));
        assert_eq!(
            exact["matches"].as_array().unwrap().len(),
            1,
            "uppercase q is exact"
        );

        let everything = ok(search(&ws, &json!({ "q": "needle", "max": 200 })));
        let paths: Vec<&str> = everything["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap())
            .collect();
        assert!(
            !paths
                .iter()
                .any(|p| p.contains("bin") || p.contains(".env") || p.contains(".plaza")),
            "{paths:?}"
        );

        let long = ok(search(
            &ws,
            &json!({ "q": "needle", "glob": "long.md", "max": 1 }),
        ));
        assert!(long["matches"][0]["text"].as_str().unwrap().chars().count() <= SEARCH_LINE_CHARS);
        assert!(search(&ws, &json!({ "q": "needle", "max": 201 })).is_err());
        assert!(search(&ws, &json!({ "q": "needle", "max": 0 })).is_err());
        assert!(search(&ws, &json!({ "q": "" })).is_err());
    }

    fn run_registry() -> Mutex<Registry> {
        Mutex::new(Registry::default())
    }

    fn settle(registry: &Mutex<Registry>, id: &str) -> Value {
        let started = Instant::now();
        loop {
            let reply = ok(poll_in(registry, &json!({ "run": id, "offset": 0 })));
            if reply["done"] == json!(true) {
                return reply;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "run {id} did not end"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn pid_is_alive(pid: &str) -> bool {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            // A zombie has ended; its state is the field after the parenthesised name.
            Ok(stat) => stat
                .rsplit(')')
                .next()
                .is_some_and(|rest| !rest.trim_start().starts_with('Z')),
            Err(_) => false,
        }
    }

    #[test]
    fn a_run_reports_its_output_and_exit_code() {
        let (_dir, ws) = scratch("run-output");
        let registry = run_registry();
        let started = ok(start_in(
            &registry,
            &ws,
            &json!({ "command": "echo hi; echo err 1>&2; exit 3" }),
        ));
        let id = started["run"].as_str().unwrap().to_string();
        let reply = settle(&registry, &id);
        assert_eq!(reply["exit_code"], json!(3));
        assert_eq!(reply["timed_out"], json!(false));
        let output = reply["output"].as_str().unwrap();
        assert!(
            output.contains("hi") && output.contains("err"),
            "{output:?}"
        );
        assert!(matches!(
            poll_in(&registry, &json!({ "run": "run-99" })),
            Err(ToolError::NotFound { .. })
        ));
        assert!(matches!(
            start_in(
                &registry,
                &ws,
                &json!({ "command": "true", "timeout_s": 601 })
            ),
            Err(ToolError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn a_run_past_its_timeout_is_killed_with_its_whole_group() {
        let (dir, ws) = scratch("run-timeout");
        let registry = run_registry();
        let command = "sleep 60 & echo $! > child.pid; sleep 60";
        let started = ok(start_in(
            &registry,
            &ws,
            &json!({ "command": command, "timeout_s": 1 }),
        ));
        let reply = settle(&registry, started["run"].as_str().unwrap());
        assert_eq!(reply["timed_out"], json!(true));
        assert!(reply["exit_code"].is_null());
        let pid =
            fs::read_to_string(dir.join("child.pid")).expect("the shell wrote its child's pid");
        let pid = pid.trim();
        assert!(
            !pid_is_alive(pid),
            "the background child {pid} was killed with the group"
        );
    }

    #[test]
    fn a_stopped_run_ends_and_a_second_stop_says_so() {
        let (_dir, ws) = scratch("run-stop");
        let registry = run_registry();
        let started = ok(start_in(&registry, &ws, &json!({ "command": "sleep 60" })));
        let id = started["run"].as_str().unwrap().to_string();
        let stopped = ok(stop_in(&registry, &json!({ "run": id })));
        assert_eq!(stopped["stopped"], json!(true));
        assert_eq!(stopped["done"], json!(true));
        let again = ok(stop_in(&registry, &json!({ "run": id })));
        assert_eq!(again["stopped"], json!(false));
    }

    #[test]
    fn a_run_keeps_the_tail_of_its_output_within_the_cap() {
        let (_dir, ws) = scratch("run-cap");
        let registry = run_registry();
        let started = ok(start_in(
            &registry,
            &ws,
            &json!({ "command": "head -c 2500000 /dev/zero | tr '\\0' a" }),
        ));
        let id = started["run"].as_str().unwrap().to_string();
        let reply = settle(&registry, &id);
        let kept = RUN_OUTPUT_CAP as u64;
        let total = 2_500_000u64;
        let offset = reply["offset"].as_u64().unwrap();
        assert!(
            offset >= total - kept && offset <= total - kept + 3,
            "offset {offset}"
        );
        let output = reply["output"].as_str().unwrap();
        assert!(output.len() <= CHUNK && output.bytes().all(|b| b == b'a'));
        assert_eq!(
            reply["next"].as_u64().unwrap(),
            offset + output.len() as u64
        );
        assert!(poll_in(&registry, &json!({ "run": id, "offset": total + 1 })).is_err());
    }

    #[test]
    fn at_most_four_runs_are_held_and_a_finished_one_makes_room() {
        let (_dir, ws) = scratch("run-limit");
        let registry = run_registry();
        let mut ids = Vec::new();
        for _ in 0..MAX_RUNS {
            let started = ok(start_in(&registry, &ws, &json!({ "command": "sleep 60" })));
            ids.push(started["run"].as_str().unwrap().to_string());
        }
        assert!(matches!(
            start_in(&registry, &ws, &json!({ "command": "true" })),
            Err(ToolError::LimitExceeded { .. })
        ));
        ok(stop_in(&registry, &json!({ "run": ids[0] })));
        let fifth = ok(start_in(&registry, &ws, &json!({ "command": "sleep 60" })));
        ids.push(fifth["run"].as_str().unwrap().to_string());
        assert!(
            lookup(&registry, &ids[0]).is_err(),
            "the finished run made room"
        );
        for id in &ids[1..] {
            ok(stop_in(&registry, &json!({ "run": id })));
        }
    }

    #[test]
    fn the_ide_tools_are_served_by_name() {
        let (_dir, ws) = scratch("served");
        let reply: Value =
            serde_json::from_str(&tools::serve(&ws, "ide_checkpoints", &json!({})).unwrap())
                .unwrap();
        assert_eq!(reply["checkpoints"], json!([]));
        assert!(matches!(
            tools::serve(&ws, "ide_nope", &json!({})),
            Err(ToolError::UnknownTool(_))
        ));
    }
}
