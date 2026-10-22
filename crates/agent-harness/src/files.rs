//! The workspace: the agent's writable `/workspace` volume, and the path rules every tool applies to a path in it.
//!
//! The guest's init mounts the plaza's writable volume at [`WORKSPACE_ROOT`] (`/workspace`) before it starts the
//! harness, and nothing else in the guest is writable, so this volume is where an agent's work lives. The tools
//! that work on it are plain `std::fs` calls in the harness process — the v2 tools in [`crate::v2`], and
//! [`download`] and [`conversation`] here — and the hub drives them: a tool arrives as a `tool_call` and leaves as
//! a `tool_result` (see [`crate::keepalive`]).
//!
//! # Path discipline
//!
//! The hub drives these tools, but neither the path nor the content is trusted to stay in the sandbox: the volume
//! is the boundary, and every tool re-proves it. A path is refused, with the reason, when it is
//!
//! * absolute (`/etc/passwd`),
//! * carries a `..` component (`../x`, `a/../../x`), or
//! * is a symlink — or passes through one — whose *resolved* path lands outside the workspace, or inside `.plaza`.
//!
//! The escape check is on the resolved path, not the spelling: before any read, write or rename, the deepest
//! existing ancestor of the target is canonicalized (which follows every link on the way) and must lie inside the
//! canonical workspace root. The directory walks of `find` and `search` never follow a symlink at all, so a link
//! cannot walk a search out of the volume. (This is not a defence against another process racing the workspace's
//! own contents between the check and the call: the tools are the only writers of this volume.)
//!
//! # Caps
//!
//! The caps of each tool are the constants of its module in [`crate::v2`]. A request over a cap is refused with
//! [`ToolError::LimitExceeded`]; a *result* cut short by a bound says so, and says where to continue.

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

/// How many leading bytes of a file are searched for a NUL to call it binary.
pub(crate) const BINARY_PROBE_BYTES: usize = 8 * 1024;

/// The largest file any file tool will act on: 1 MiB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The largest single `write_file`/`append_file` payload, and the largest
/// `edit_file` replacement: 256 KiB.
pub const MAX_WRITE_BYTES: u64 = 256 * 1024;

/// The agent's conversation log, relative to the workspace root: the file
/// [`conversation`] appends one line to for every message a peer sends this
/// agent.
pub const MESSAGES_LOG: &str = "messages.log";

/// The most bytes one message's `text` may occupy: the plaza's 2 KiB direct-
/// message cap, which this tool re-checks because the hub's arguments are not
/// trusted to have stopped at the door.
pub const MAX_MESSAGE_BYTES: usize = 2048;

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
pub(crate) fn read_text(path: &Path, label: &str) -> Result<String, ToolError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::v2;

    /// A fresh scratch workspace, and a scratch directory beside it that is *outside* it, for the escape tests.
    fn scratch(name: &str) -> (Workspace, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "apz-harness-files-{}-{name}-{unique}",
            std::process::id()
        ));
        let inside = base.join("workspace");
        let outside = base.join("outside");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&inside).expect("a scratch workspace");
        fs::create_dir_all(&outside).expect("a scratch directory beside it");
        (Workspace::at(inside), outside)
    }

    /// The reply of a v2 call, parsed.
    fn reply(workspace: &Workspace, tool: &str, args: Value) -> Value {
        let text = v2::serve(workspace, 1, tool, &args)
            .unwrap_or_else(|err| panic!("{tool} {args} should be served: {err}"));
        serde_json::from_str(&text).expect("a v2 reply is JSON")
    }

    /// The refusal a v2 call's first file entry carries.
    fn entry_error(workspace: &Workspace, tool: &str, args: Value) -> String {
        let reply = reply(workspace, tool, args);
        reply["files"][0]["error"]
            .as_str()
            .unwrap_or_else(|| panic!("the entry should be refused: {reply}"))
            .to_string()
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

    #[test]
    fn an_absolute_path_is_denied_with_its_reason_by_every_path_taking_tool() {
        let (workspace, _outside) = scratch("absolute");
        assert!(
            entry_error(
                &workspace,
                "read",
                json!({ "files": [{ "path": "/etc/passwd" }] })
            )
            .contains("absolute")
        );
        let edit = json!({ "create": [{ "path": "/etc/passwd", "content": "x" }] });
        assert!(entry_error(&workspace, "edit", edit).contains("absolute"));
        let found = v2::serve(
            &workspace,
            1,
            "find",
            &json!({ "glob": "*", "path": "/etc" }),
        )
        .expect_err("refused");
        assert!(found.to_string().contains("absolute"), "{found}");
        let searched = v2::serve(
            &workspace,
            1,
            "search",
            &json!({ "pattern": "x", "path": "/etc" }),
        )
        .expect_err("refused");
        assert!(searched.to_string().contains("absolute"), "{searched}");
    }

    #[test]
    fn a_parent_component_is_denied_however_it_is_spelled() {
        let (workspace, _outside) = scratch("parent");
        write_inside(&workspace, "notes.txt", b"inside\n");
        for path in ["../notes.txt", "a/../../notes.txt", "./../x", "a/..", ".."] {
            let error = entry_error(&workspace, "read", json!({ "files": [{ "path": path }] }));
            assert!(error.starts_with("denied:"), "{path}: {error}");
            assert!(error.contains("\"..\""), "{path}: {error}");
        }
    }

    #[test]
    fn a_dangling_link_is_refused_before_any_write_creates_its_target() {
        // `resolve` canonicalizes the deepest EXISTING ancestor, so a link whose target does not exist yet passes
        // the walk, and a write would then create the target outside the workspace. Every write must refuse it.
        let (workspace, outside) = scratch("symlink-dangling");
        let target = outside.join("planted.txt");
        symlink(&target, workspace.root().join("esc")).expect("a dangling symlink");
        for args in [
            json!({ "create": [{ "path": "esc", "content": "escaped\n" }] }),
            json!({ "edits": [{ "path": "esc", "old": "x", "new": "y" }] }),
        ] {
            let error = entry_error(&workspace, "edit", args);
            assert!(
                error.starts_with("denied:") && error.contains("symlink"),
                "{error}"
            );
        }
        assert!(
            !target.exists(),
            "no write may have created the link's target"
        );

        // A dangling directory link on the way: the write would make the target folder.
        symlink(outside.join("missing"), workspace.root().join("d"))
            .expect("a dangling directory link");
        let error = entry_error(
            &workspace,
            "edit",
            json!({ "create": [{ "path": "d/x.txt", "content": "x" }] }),
        );
        assert!(error.contains("symlink"), "{error}");
        assert!(!outside.join("missing").exists());
    }

    #[test]
    fn a_symlink_out_of_the_workspace_or_into_plaza_is_refused_by_every_tool() {
        let (workspace, outside) = scratch("symlink-out");
        fs::write(outside.join("secret.txt"), b"host data\n").expect("the outside file");
        symlink(
            outside.join("secret.txt"),
            workspace.root().join("link.txt"),
        )
        .expect("a link out");
        symlink(&outside, workspace.root().join("outsidedir")).expect("a link to an outside dir");

        let error = entry_error(
            &workspace,
            "read",
            json!({ "files": [{ "path": "link.txt" }] }),
        );
        assert!(
            error.starts_with("denied:") && error.contains("outside the workspace"),
            "{error}"
        );
        let error = entry_error(
            &workspace,
            "edit",
            json!({ "create": [{ "path": "outsidedir/new.txt", "content": "x" }] }),
        );
        assert!(error.contains("outside the workspace"), "{error}");
        assert!(!outside.join("new.txt").exists());
        // Search does not follow a link out of the volume.
        let searched = reply(&workspace, "search", json!({ "pattern": "host" }));
        assert_eq!(
            searched["matches"].as_array().map(Vec::len),
            Some(0),
            "{searched}"
        );
    }

    #[test]
    fn a_symlink_into_the_harness_state_is_refused_by_every_tool() {
        let (workspace, _outside) = scratch("symlink-plaza");
        write_inside(&workspace, ".plaza/state.txt", b"harness state\n");
        symlink(workspace.root().join(".plaza"), workspace.root().join("p"))
            .expect("a link to the state folder");
        symlink(
            workspace.root().join(".plaza/state.txt"),
            workspace.root().join("state"),
        )
        .expect("a link to a state file");

        let error = entry_error(
            &workspace,
            "read",
            json!({ "files": [{ "path": "p/state.txt" }] }),
        );
        assert!(error.contains("harness's own state"), "{error}");
        let error = entry_error(
            &workspace,
            "read",
            json!({ "files": [{ "path": "state" }] }),
        );
        assert!(error.contains("harness's own state"), "{error}");
        let error = entry_error(
            &workspace,
            "edit",
            json!({ "create": [{ "path": "p/new.txt", "content": "x" }] }),
        );
        assert!(error.contains("harness's own state"), "{error}");
        let error = entry_error(
            &workspace,
            "edit",
            json!({ "edits": [{ "path": "state", "old": "harness", "new": "y" }] }),
        );
        assert!(error.contains("harness's own state"), "{error}");
        let searched = reply(&workspace, "search", json!({ "pattern": "harness" }));
        assert_eq!(
            searched["matches"].as_array().map(Vec::len),
            Some(0),
            "{searched}"
        );
        assert_eq!(read_back(&workspace, ".plaza/state.txt"), "harness state\n");
        assert!(!workspace.root().join(".plaza/new.txt").exists());
    }

    #[test]
    fn a_missing_workspace_is_unavailable_not_a_panic() {
        let workspace = Workspace::at("/no/such/plaza/workspace");
        let error = entry_error(
            &workspace,
            "read",
            json!({ "files": [{ "path": "a.txt" }] }),
        );
        assert!(error.starts_with("unavailable: "), "{error}");
        let err =
            v2::serve(&workspace, 1, "find", &json!({ "glob": "*" })).expect_err("unavailable");
        assert!(matches!(err, ToolError::Unavailable { .. }), "{err:?}");
    }

    #[test]
    fn a_conversation_message_is_one_escaped_line_in_the_agents_log() {
        let (workspace, _outside) = scratch("conversation");
        let served: Value = serde_json::from_str(
            &conversation(
                &workspace,
                &json!({ "from_role": "builder", "text": "hi\nforged line" }),
            )
            .expect("a message"),
        )
        .expect("JSON");
        assert_eq!(served["received"], true);
        assert_eq!(served["lines"], 1);
        let log = read_back(&workspace, MESSAGES_LOG);
        assert_eq!(log.matches('\n').count(), 1, "{log:?}");
        assert!(log.contains(" from builder: "), "{log}");
        assert!(!log.contains("\nforged line"), "{log}");
    }

    #[test]
    fn a_conversation_message_over_the_cap_is_refused_and_writes_nothing() {
        let (workspace, _outside) = scratch("conversation-cap");
        let text = "x".repeat(MAX_MESSAGE_BYTES + 1);
        let err = conversation(&workspace, &json!({ "from_role": "builder", "text": text }))
            .expect_err("over the cap");
        assert!(matches!(err, ToolError::LimitExceeded { .. }), "{err:?}");
        assert!(!workspace.root().join(MESSAGES_LOG).exists());
    }
}
