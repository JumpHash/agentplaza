//! `agentplaza ide`: the IDE's workspaces from the terminal. A workspace is an agent's `/workspace`
//! (`agent:<cid>`) or a project repository at one branch (`repo:<name>@<ref>`). Paths are relative
//! to the workspace root. Secret files are never read or written, and `.plaza/` is read-only.

use std::collections::VecDeque;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_ide::diff::{parse_unified, ChangeSet};
use apz_ide::path::{check_dir, check_read, check_write};
use apz_ide::{LineKind, WorkspaceId};
use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, confirm, Ctx};

/// Entries a recursive `ls` lists before it stops.
const WALK_CAP: usize = 2000;
/// How often a run's output is polled.
const POLL: Duration = Duration::from_millis(500);

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The workspaces the IDE can open: agents' VMs and project repositories.
    Workspaces,
    /// The files and folders at a path. `-r` lists every folder below it too (capped).
    Ls {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// The folder, relative to the root (default: the root).
        #[arg(default_value = "")]
        path: String,
        /// Walk every folder below the path, not only its children.
        #[arg(short, long)]
        recursive: bool,
    },
    /// Print a text file. `--hash` prints its content hash instead.
    Cat {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// The file's path, relative to the root.
        path: String,
        /// Print the content hash (what `put --expected` takes) instead of the text.
        #[arg(long)]
        hash: bool,
    },
    /// Edit a file in $VISUAL or $EDITOR, then save it. A version changed on the hub asks what to do.
    Edit {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// The file's path, relative to the root.
        path: String,
        /// On a conflict, keep your version without asking.
        #[arg(long)]
        force: bool,
    },
    /// Write a file's text from `--file` (`-` reads stdin).
    Put {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// The file's path, relative to the root.
        path: String,
        /// The new text: a file, or `-` for stdin.
        #[arg(long)]
        file: PathBuf,
        /// Write only if the file still has this hash (from `cat --hash`).
        #[arg(long, value_name = "HASH", conflicts_with = "new")]
        expected: Option<String>,
        /// Create the file; refuse if it already exists.
        #[arg(long)]
        new: bool,
    },
    /// What changed: since the work started (default), or since a checkpoint.
    Diff {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// `work_start` or `checkpoint:N`.
        #[arg(long, default_value = "work_start")]
        base: String,
        /// Only the changed files and their line counts.
        #[arg(long)]
        stat: bool,
    },
    /// The checkpoints of a workspace.
    Checkpoints {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
    },
    /// Put the files back to a checkpoint.
    Restore {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// The checkpoint's number.
        n: u64,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Search the text of the files in a workspace.
    Search {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// The text to find.
        query: String,
        /// Only the files whose path matches this glob.
        #[arg(long)]
        glob: Option<String>,
    },
    /// Run one command in a workspace and stream its output. Ctrl+C stops it.
    Run {
        /// The workspace: agent:<cid> or repo:<name>@<ref>.
        workspace: String,
        /// Stop the command after this many seconds (default 300).
        #[arg(long, default_value_t = 300)]
        timeout: u32,
        /// The command and its arguments, after `--`.
        #[arg(last = true, required = true, num_args = 1..)]
        command: Vec<String>,
    },
    /// The IDE's audit log, newest first.
    Audit {
        /// Only this workspace.
        #[arg(long)]
        workspace: Option<String>,
        /// How many entries (default 50).
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
}

// ---- Paths and requests -----------------------------------------------------------------------

/// The workspace id as one path segment: parsed here (so a bad id fails before any call), then encoded.
fn workspace_seg(workspace: &str) -> Result<String, String> {
    let id = WorkspaceId::from_str(workspace).map_err(|e| format!("{workspace}: {e}"))?;
    Ok(ctx::seg(&id.to_string()))
}

/// The path of a workspace's resource (`rest` is appended, e.g. "/file").
fn ws_path(workspace: &str, rest: &str) -> Result<String, String> {
    Ok(format!("/v1/ide/workspaces/{}{rest}", workspace_seg(workspace)?))
}

/// One call that keeps the status, so a 404 or a 409 can be read. Transport failures are errors.
fn raw(ctx: &Ctx, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    apz_http::client::request_json(&ctx.client.target, method, path, body, ctx.client.timeout)
        .map_err(|e| format!("{} {path}: {e}", ctx.client.base()))
}

/// The body of a 2xx answer; otherwise the hub's `error` text.
fn ok_or_error(method: &str, path: &str, status: u16, body: Value) -> Result<Value, String> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    Err(body
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{method} {path}: HTTP {status}")))
}

/// The objects of a list the hub returns: a bare array, or an object holding one under `key`.
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.as_array().or_else(|| v.get(key).and_then(Value::as_array)).map(Vec::as_slice).unwrap_or_default()
}

/// The first of `keys` that holds something, as text.
fn first_of(v: &Value, keys: &[&str]) -> String {
    keys.iter().map(|k| ctx::text(v, k)).find(|t| !t.is_empty()).unwrap_or_default()
}

/// A file's record from the hub (its hash and text), or None when the file does not exist.
fn file_record(ctx: &Ctx, workspace: &str, path: &str) -> Result<Option<Value>, String> {
    let api = format!("{}?path={}", ws_path(workspace, "/file")?, ctx::seg(path));
    let (status, body) = raw(ctx, "GET", &api, None)?;
    if status == 404 {
        return Ok(None);
    }
    ok_or_error("GET", &api, status, body).map(Some)
}

/// The text of a file record; binary files are not edited or shown as text.
fn text_of(record: &Value, path: &str) -> Result<String, String> {
    record
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{path} is not a text file"))
}

/// The content hash of a file record, if it has one.
fn hash_of(record: &Value) -> Option<String> {
    record.get("hash").and_then(Value::as_str).map(str::to_string)
}

/// What a save came back as: written, or refused because the hub's version moved on.
#[derive(Debug, PartialEq, Eq)]
enum Save {
    Saved(Value),
    /// The hub's current hash (None when the file is gone).
    Conflict(Option<String>),
}

/// Reads a save's answer: a 2xx is saved, a 409 `conflict` carries the hub's current hash,
/// and anything else is an error.
fn classify_save(api: &str, status: u16, body: Value) -> Result<Save, String> {
    if status == 409 && body.get("error").and_then(Value::as_str) == Some("conflict") {
        return Ok(Save::Conflict(body.get("current_hash").and_then(Value::as_str).map(str::to_string)));
    }
    ok_or_error("PUT", api, status, body).map(Save::Saved)
}

/// Writes a file's text, expecting the hash it was read at (None: no check).
fn save(ctx: &Ctx, workspace: &str, path: &str, text: &str, expected: Option<&str>) -> Result<Save, String> {
    let api = ws_path(workspace, "/file")?;
    let body = json!({ "path": path, "text": text, "expected_hash": expected });
    let (status, value) = raw(ctx, "PUT", &api, Some(&body))?;
    classify_save(&api, status, value)
}

// ---- Conflicts ----------------------------------------------------------------------------------

/// What a conflicting save does: `--force` keeps the user's version, an interactive terminal asks,
/// and anything else refuses (the edit is kept on disk for the user).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnConflict {
    KeepMine,
    Ask,
    Refuse,
}

fn on_conflict(force: bool, terminal: bool) -> OnConflict {
    if force {
        OnConflict::KeepMine
    } else if terminal {
        OnConflict::Ask
    } else {
        OnConflict::Refuse
    }
}

/// The answer to the conflict question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// Take the hub's version into the editor again; your version is kept beside it.
    Reload,
    /// Save your version over the hub's.
    Keep,
    Abort,
}

/// `r`/`k`/`a` (or the full words), any case. Anything else is not an answer.
fn parse_resolution(answer: &str) -> Option<Resolution> {
    match answer.trim().to_ascii_lowercase().as_str() {
        "r" | "reload" => Some(Resolution::Reload),
        "k" | "keep" => Some(Resolution::Keep),
        "a" | "abort" => Some(Resolution::Abort),
        _ => None,
    }
}

/// Asks until the answer is one of the three. End of input is an abort.
fn ask_resolution() -> Result<Resolution, String> {
    loop {
        print!("the file changed on the hub: [r]eload and re-edit / [k]eep mine / [a]bort: ");
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(Resolution::Abort);
        }
        if let Some(resolution) = parse_resolution(&line) {
            return Ok(resolution);
        }
    }
}

// ---- Editing ------------------------------------------------------------------------------------

/// A private directory holding the file being edited, under its own name so the editor picks the
/// right syntax. It is removed only when the edit is saved or dropped; otherwise the user's text
/// stays on disk and its path is reported.
struct Scratch {
    dir: PathBuf,
    name: String,
    file: PathBuf,
}

impl Scratch {
    fn new(path: &str) -> Result<Scratch, String> {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("apz-ide-{}-{nanos}", std::process::id()));
        apz_os::private::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join(&name);
        Ok(Scratch { dir, name, file })
    }

    fn write(&self, text: &str) -> Result<(), String> {
        apz_os::private::write(&self.file, text.as_bytes()).map_err(|e| format!("{}: {e}", self.file.display()))
    }

    fn read(&self) -> Result<String, String> {
        std::fs::read_to_string(&self.file).map_err(|e| format!("{}: {e}", self.file.display()))
    }

    /// Keeps `text` as `<name>.mine` beside the edit, and returns its path.
    fn keep_mine(&self, text: &str) -> Result<PathBuf, String> {
        let mine = self.dir.join(format!("{}.mine", self.name));
        apz_os::private::write(&mine, text.as_bytes()).map_err(|e| format!("{}: {e}", mine.display()))?;
        Ok(mine)
    }

    /// Removes the edit's directory.
    fn discard(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The editor command line: `$VISUAL`, else `$EDITOR`, else `vi`.
fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_string())
}

/// The program and its arguments from an editor command line (`code --wait`).
fn split_command(line: &str) -> Option<(String, Vec<String>)> {
    let mut parts = line.split_whitespace().map(str::to_string);
    let program = parts.next()?;
    Some((program, parts.collect()))
}

/// Opens `file` in the editor and waits for it to exit.
fn open_editor(file: &Path) -> Result<(), String> {
    let line = editor_command();
    let (program, args) = split_command(&line).ok_or_else(|| "the editor setting is empty".to_string())?;
    let status = std::process::Command::new(&program)
        .args(&args)
        .arg(file)
        .status()
        .map_err(|e| format!("cannot start the editor {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the editor {program} exited with {status}"))
    }
}

/// Fetches the file, lets the user edit it, and saves it if it changed. A changed version on the hub
/// is handled by `on_conflict`: keep yours (`--force`), ask (reload, keep, or abort), or refuse.
fn edit(ctx: &Ctx, workspace: &str, path: &str, force: bool) -> Result<(), String> {
    let path = check_write(path).map_err(|e| format!("{path}: {e}"))?;
    let record = file_record(ctx, workspace, &path)?.ok_or_else(|| format!("{path}: no such file"))?;
    let mut base = text_of(&record, &path)?;
    let mut expected = hash_of(&record);
    let scratch = Scratch::new(&path)?;
    scratch.write(&base)?;
    let kept = scratch.file.display().to_string();
    open_editor(&scratch.file).map_err(|e| format!("{e}; the file is at {kept}"))?;
    let mut body = scratch.read()?;
    loop {
        if body == base {
            scratch.discard();
            println!("no changes to {path}");
            return Ok(());
        }
        match save(ctx, workspace, &path, &body, expected.as_deref()) {
            Err(e) => return Err(format!("{e}; your edit is kept at {kept}")),
            Ok(Save::Saved(saved)) => {
                scratch.discard();
                ctx.out(&saved, |s| println!("saved {path} ({})", ctx::s(s, "hash")));
                return Ok(());
            }
            Ok(Save::Conflict(current)) => match on_conflict(force, std::io::stdin().is_terminal()) {
                OnConflict::KeepMine => {
                    eprintln!("{path} changed on the hub; keeping your version over it");
                    expected = current;
                }
                OnConflict::Refuse => {
                    return Err(format!(
                        "{path} changed on the hub since you opened it; your edit is kept at {kept}. Edit again, or pass --force to keep yours"
                    ))
                }
                OnConflict::Ask => match ask_resolution()? {
                    Resolution::Keep => expected = current,
                    Resolution::Abort => return Err(format!("aborted; your edit is kept at {kept}")),
                    Resolution::Reload => {
                        let mine = scratch.keep_mine(&body)?;
                        let fresh = file_record(ctx, workspace, &path)?.ok_or_else(|| {
                            format!("{path} was removed on the hub; your version is kept at {}", mine.display())
                        })?;
                        base = text_of(&fresh, &path)?;
                        expected = hash_of(&fresh);
                        scratch.write(&base)?;
                        println!("reloaded {path}; your version is kept at {}", mine.display());
                        open_editor(&scratch.file).map_err(|e| format!("{e}; the file is at {kept}"))?;
                        body = scratch.read()?;
                    }
                },
            },
        }
    }
}

// ---- Put ----------------------------------------------------------------------------------------

/// The text for `put`: from the file, or all of stdin for `-`.
fn read_input(file: &Path) -> Result<String, String> {
    if file == Path::new("-") {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("reading stdin: {e}"))?;
        return Ok(text);
    }
    std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))
}

/// Writes a file. `--new` refuses an existing file; `--expected` refuses a changed one; with neither,
/// the file's current hash is read first and the write goes over that version.
fn put(ctx: &Ctx, workspace: &str, path: &str, file: &Path, expected: Option<String>, new: bool) -> Result<(), String> {
    let path = check_write(path).map_err(|e| format!("{path}: {e}"))?;
    let text = read_input(file)?;
    let expected = if new {
        if file_record(ctx, workspace, &path)?.is_some() {
            return Err(format!("{path} already exists; pass --expected HASH to overwrite it"));
        }
        None
    } else if let Some(hash) = expected {
        if hash.trim().is_empty() {
            return Err("--expected takes a hash, not an empty value".into());
        }
        Some(hash)
    } else {
        file_record(ctx, workspace, &path)?.as_ref().and_then(hash_of)
    };
    match save(ctx, workspace, &path, &text, expected.as_deref())? {
        Save::Saved(saved) => {
            ctx.out(&saved, |s| println!("wrote {path} ({})", ctx::s(s, "hash")));
            Ok(())
        }
        Save::Conflict(current) => Err(format!(
            "{path} changed on the hub; its hash is now {}. Nothing was written",
            current.as_deref().unwrap_or("none (the file is gone)")
        )),
    }
}

// ---- Diff ---------------------------------------------------------------------------------------

/// `work_start` or `checkpoint:N`, as the hub takes them.
fn parse_base(base: &str) -> Result<String, String> {
    if base == "work_start" {
        return Ok(base.to_string());
    }
    let number_after = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    if base.strip_prefix("checkpoint:").is_some_and(number_after) {
        return Ok(base.to_string());
    }
    Err(format!("--base is work_start or checkpoint:N, not {base}"))
}

/// Colour is for a terminal, and `NO_COLOR` turns it off.
fn use_colour(stdout_is_terminal: bool, no_colour_set: bool) -> bool {
    stdout_is_terminal && !no_colour_set
}

/// The diff with ANSI colour: headers bold, hunk headers cyan, added lines green, removed red.
/// Text is returned unchanged when `colour` is off.
fn colour_diff(text: &str, colour: bool) -> String {
    if !colour {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut in_hunk = false;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let paint = if body.starts_with("diff ") {
            in_hunk = false;
            Some("1")
        } else if body.starts_with("@@") {
            in_hunk = true;
            Some("36")
        } else if in_hunk {
            if body.starts_with('+') {
                Some("32")
            } else if body.starts_with('-') {
                Some("31")
            } else {
                None
            }
        } else if body.starts_with("+++") || body.starts_with("---") || body.starts_with("index ") {
            Some("1")
        } else {
            None
        };
        match paint {
            Some(code) => out.push_str(&format!("\x1b[{code}m{body}\x1b[0m")),
            None => out.push_str(body),
        }
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// One line per changed file (path, added and removed lines), then the totals.
fn stat_lines(set: &ChangeSet) -> Vec<String> {
    let width = set.files.iter().map(|f| f.path.chars().count()).max().unwrap_or(0);
    let mut lines: Vec<String> = set
        .files
        .iter()
        .map(|f| {
            let counts = if f.binary {
                "binary".to_string()
            } else {
                let (added, removed) = f
                    .hunks
                    .iter()
                    .flat_map(|h| &h.lines)
                    .fold((0usize, 0usize), |(a, r), line| match line.kind {
                        LineKind::Add => (a + 1, r),
                        LineKind::Remove => (a, r + 1),
                        LineKind::Context | LineKind::NoNewline => (a, r),
                    });
                format!("+{added} -{removed}")
            };
            format!(" {:<width$} | {counts}", f.path, width = width)
        })
        .collect();
    let totals = set.stats();
    lines.push(format!(
        "{} files changed, {} insertions(+), {} deletions(-)",
        totals.files, totals.additions, totals.deletions
    ));
    lines
}

// ---- Checkpoints, search, audit -----------------------------------------------------------------

/// The checkpoints of a list answer: a bare array, or an object with `checkpoints`.
fn checkpoints_of(v: &Value) -> &[Value] {
    items(v, "checkpoints")
}

/// The folder's children (or every folder below it, when `recursive`), page after page, capped at
/// `WALK_CAP` entries. The flag says whether the cap stopped the walk.
fn walk(ctx: &Ctx, workspace: &str, root: &str, recursive: bool) -> Result<(Vec<Value>, bool), String> {
    let base = ws_path(workspace, "/tree")?;
    let mut queue = VecDeque::from([root.to_string()]);
    let mut out: Vec<Value> = Vec::new();
    while let Some(dir) = queue.pop_front() {
        let mut cursor: Option<String> = None;
        loop {
            let mut url = format!("{base}?path={}", ctx::seg(&dir));
            if let Some(c) = &cursor {
                url.push_str(&format!("&cursor={}", ctx::seg(c)));
            }
            let page = ctx.client.get(&url)?;
            for entry in items(&page, "entries") {
                if recursive && ctx::s(entry, "kind") == "dir" {
                    queue.push_back(ctx::s(entry, "path").to_string());
                }
                out.push(entry.clone());
                if out.len() >= WALK_CAP {
                    return Ok((out, true));
                }
            }
            cursor = page.get("next").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok((out, false))
}

// ---- Runs ---------------------------------------------------------------------------------------

/// Set by Ctrl+C during a run; the first press stops the run, a second one leaves at once.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_interrupt(_signal: libc::c_int) {
    if INTERRUPTED.swap(true, Ordering::SeqCst) {
        // SAFETY: `_exit` is async-signal-safe; a second Ctrl+C leaves without waiting for the run.
        unsafe { libc::_exit(130) };
    }
}

/// Catches Ctrl+C for the duration of a run.
#[cfg(unix)]
fn catch_interrupt() {
    INTERRUPTED.store(false, Ordering::SeqCst);
    // SAFETY: the handler only touches an atomic flag and calls `_exit`, both async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, on_interrupt as libc::sighandler_t);
    }
}

#[cfg(unix)]
fn release_interrupt() {
    // SAFETY: restores the default disposition that the process had before the run.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
}

// On Windows Ctrl+C ends this command; the run itself keeps going on the hub.
#[cfg(not(unix))]
fn catch_interrupt() {
    INTERRUPTED.store(false, Ordering::SeqCst);
}

#[cfg(not(unix))]
fn release_interrupt() {}

/// The process exit code of a finished run: 124 on timeout, the command's own code otherwise
/// (a signal's negative code becomes 128 plus the signal), and 130 when the user stopped it.
fn exit_code(snapshot: &Value, interrupted: bool) -> i32 {
    if snapshot.get("timed_out").and_then(Value::as_bool).unwrap_or(false) {
        return 124;
    }
    match snapshot.get("exit_code").and_then(Value::as_i64) {
        Some(code) if code < 0 => (128 - code).clamp(0, 255) as i32,
        Some(code) => code.clamp(0, 255) as i32,
        None if interrupted => 130,
        None => 1,
    }
}

/// Each argument quoted for a POSIX shell, so `sh -c 'a && b'` reaches the hub intact.
fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=:,@%+".contains(&b));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

fn shell_join(args: &[String]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

/// Polls a run's output from its offset until it is done, printing each new chunk (or, with `--json`,
/// collecting it). Ctrl+C asks the hub to stop the run once, and polling carries on to the end.
/// Returns the final snapshot and the output collected for `--json`.
fn stream(ctx: &Ctx, base: &str, run: &str) -> Result<(Value, String), String> {
    let mut offset: u64 = 0;
    let mut stopping = false;
    let mut captured = String::new();
    loop {
        let snap = ctx.client.get(&format!("{base}?offset={offset}"))?;
        let chunk = ctx::s(&snap, "output");
        if !chunk.is_empty() {
            if ctx.json {
                captured.push_str(chunk);
            } else {
                print!("{chunk}");
                let _ = std::io::stdout().flush();
            }
        }
        offset = snap.get("next").and_then(Value::as_u64).unwrap_or(offset + chunk.len() as u64);
        if snap.get("done").and_then(Value::as_bool).unwrap_or(false) {
            return Ok((snap, captured));
        }
        if !stopping && INTERRUPTED.load(Ordering::SeqCst) {
            stopping = true;
            eprintln!("stopping {run}");
            if let Err(e) = ctx.client.post(&format!("{base}/stop"), &json!({})) {
                eprintln!("agentplaza: stop: {e}");
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Starts a bounded command in a workspace, streams it, and returns the exit code it should end with.
fn run_command(ctx: &Ctx, workspace: &str, timeout: u32, command: &[String]) -> Result<i32, String> {
    if timeout == 0 {
        return Err("--timeout is at least 1 second".into());
    }
    let line = shell_join(command);
    let started = ctx.client.post(&ws_path(workspace, "/runs")?, &json!({ "command": line, "timeout_s": timeout }))?;
    let run = ctx::text(&started, "run");
    if run.is_empty() {
        return Err("the hub did not return a run id".into());
    }
    let base = format!("{}/runs/{}", ws_path(workspace, "")?, ctx::seg(&run));
    catch_interrupt();
    let finished = stream(ctx, &base, &run);
    release_interrupt();
    let (last, captured) = finished?;
    let interrupted = INTERRUPTED.load(Ordering::SeqCst);
    let code = exit_code(&last, interrupted);
    if ctx.json {
        let timed_out = last.get("timed_out").and_then(Value::as_bool).unwrap_or(false);
        ctx.out(
            &json!({ "run": run, "output": captured, "exit_code": code, "timed_out": timed_out }),
            |_| {},
        );
    } else if last.get("timed_out").and_then(Value::as_bool).unwrap_or(false) {
        eprintln!("timed out after {timeout} s");
    }
    Ok(code)
}

// ---- The command --------------------------------------------------------------------------------

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Workspaces => {
            let list = ctx.client.get("/v1/ide/workspaces")?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = items(v, "workspaces")
                    .iter()
                    .map(|w| {
                        vec![
                            ctx::s(w, "id").to_string(),
                            ctx::s(w, "kind").to_string(),
                            ctx::s(w, "owner").to_string(),
                            ctx::s(w, "name").to_string(),
                            ctx::text(w, "writable"),
                            ctx::s(w, "status").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "kind", "owner", "name", "writable", "status"], &rows);
            });
            Ok(())
        }
        Cmd::Ls { workspace, path, recursive } => {
            let dir = check_dir(&path).map_err(|e| format!("{path}: {e}"))?;
            let (entries, truncated) = walk(ctx, &workspace, &dir, recursive)?;
            let listing = json!({ "entries": entries, "truncated": truncated });
            ctx.out(&listing, |_| {
                let rows: Vec<Vec<String>> = entries
                    .iter()
                    .map(|e| vec![ctx::s(e, "kind").to_string(), ctx::text(e, "size"), ctx::s(e, "path").to_string()])
                    .collect();
                ctx::table(&["kind", "size", "path"], &rows);
            });
            if truncated && !ctx.json {
                eprintln!("stopped at {WALK_CAP} entries; list a folder below to see the rest");
            }
            Ok(())
        }
        Cmd::Cat { workspace, path, hash } => {
            let path = check_read(&path).map_err(|e| format!("{path}: {e}"))?;
            let record = file_record(ctx, &workspace, &path)?.ok_or_else(|| format!("{path}: no such file"))?;
            if ctx.json {
                ctx.out(&record, |_| {});
            } else if hash {
                println!("{}", ctx::s(&record, "hash"));
            } else {
                print!("{}", text_of(&record, &path)?);
                let _ = std::io::stdout().flush();
            }
            Ok(())
        }
        Cmd::Edit { workspace, path, force } => edit(ctx, &workspace, &path, force),
        Cmd::Put { workspace, path, file, expected, new } => put(ctx, &workspace, &path, &file, expected, new),
        Cmd::Diff { workspace, base, stat } => {
            let base = parse_base(&base)?;
            let url = format!("{}?base={}", ws_path(&workspace, "/changes")?, ctx::seg(&base));
            let changes = ctx.client.get(&url)?;
            let text = ctx::s(&changes, "diff");
            if stat {
                let set = parse_unified(text).map_err(|e| e.to_string())?;
                ctx.out(&changes, |_| {
                    if set.files.is_empty() {
                        println!("no changes since {base}");
                    } else {
                        for line in stat_lines(&set) {
                            println!("{line}");
                        }
                    }
                });
            } else {
                ctx.out(&changes, |_| {
                    if text.trim().is_empty() {
                        println!("no changes since {base}");
                    } else {
                        print!("{}", colour_diff(text, use_colour(std::io::stdout().is_terminal(), std::env::var_os("NO_COLOR").is_some())));
                    }
                });
            }
            Ok(())
        }
        Cmd::Checkpoints { workspace } => {
            let list = ctx.client.get(&ws_path(&workspace, "/checkpoints")?)?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = checkpoints_of(v)
                    .iter()
                    .map(|c| vec![ctx::text(c, "n"), ctx::s(c, "label").to_string(), ctx::when(&c["created_ms"]), ctx::text(c, "files")])
                    .collect();
                ctx::table(&["n", "label", "created", "files"], &rows);
            });
            Ok(())
        }
        Cmd::Restore { workspace, n, yes } => {
            let question = format!("Restore checkpoint {n} in {workspace}? Files go back to their state at that checkpoint.");
            if !confirm(yes, &question)? {
                println!("not restored");
                return Ok(());
            }
            let r = ctx.client.post(&format!("{}/{n}/restore", ws_path(&workspace, "/checkpoints")?), &json!({}))?;
            ctx.out(&r, |_| println!("restored checkpoint {n} in {workspace}"));
            Ok(())
        }
        Cmd::Search { workspace, query, glob } => {
            if query.trim().is_empty() {
                return Err("the query is empty".into());
            }
            let mut url = format!("{}?q={}", ws_path(&workspace, "/search")?, ctx::seg(&query));
            if let Some(glob) = glob.filter(|g| !g.trim().is_empty()) {
                url.push_str(&format!("&glob={}", ctx::seg(&glob)));
            }
            let res = ctx.client.get(&url)?;
            let truncated = res.get("truncated").and_then(Value::as_bool).unwrap_or(false);
            ctx.out(&res, |r| {
                let matches = items(r, "matches");
                if matches.is_empty() {
                    println!("no matches");
                }
                for m in matches {
                    println!("{}:{}  {}", ctx::s(m, "path"), ctx::text(m, "line"), ctx::s(m, "text"));
                }
            });
            if truncated && !ctx.json {
                eprintln!("more matches than shown; narrow the query or the glob");
            }
            Ok(())
        }
        Cmd::Run { workspace, timeout, command } => {
            let code = run_command(ctx, &workspace, timeout, &command)?;
            if code != 0 {
                let _ = std::io::stdout().flush();
                std::process::exit(code);
            }
            Ok(())
        }
        Cmd::Audit { workspace, limit } => {
            let mut url = format!("/v1/ide/audit?limit={limit}");
            if let Some(ws) = workspace {
                url.push_str(&format!("&workspace={}", workspace_seg(&ws)?));
            }
            let entries = ctx.client.get(&url)?;
            ctx.out(&entries, |v| {
                let rows: Vec<Vec<String>> = items(v, "entries")
                    .iter()
                    .map(|e| {
                        let at = if e["at"].is_null() { &e["created_ms"] } else { &e["at"] };
                        vec![
                            ctx::when(at),
                            first_of(e, &["action", "kind"]),
                            first_of(e, &["actor", "by"]),
                            first_of(e, &["workspace"]),
                            first_of(e, &["path"]),
                        ]
                    })
                    .collect();
                ctx::table(&["when", "action", "by", "workspace", "path"], &rows);
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Wrap {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        Wrap::try_parse_from(std::iter::once("ide").chain(args.iter().copied())).map(|w| w.cmd)
    }

    #[test]
    fn workspace_ids_are_encoded_as_one_segment() {
        assert_eq!(workspace_seg("agent:12").unwrap(), "agent%3A12");
        assert_eq!(workspace_seg("repo:demo@feature/x").unwrap(), "repo%3Ademo%40feature%2Fx");
        assert_eq!(ws_path("agent:0", "/tree").unwrap(), "/v1/ide/workspaces/agent%3A0/tree");
    }

    #[test]
    fn a_bad_workspace_id_fails_before_any_call() {
        assert!(workspace_seg("agent:012").is_err(), "no leading zeros");
        assert!(workspace_seg("repo:demo").is_err(), "a repo needs @ref");
        assert!(workspace_seg("home:1").is_err());
    }

    #[test]
    fn a_409_is_a_conflict_with_the_current_hash() {
        let body = json!({"error": "conflict", "current_hash": "h2"});
        assert_eq!(classify_save("/x", 409, body).unwrap(), Save::Conflict(Some("h2".into())));
        assert_eq!(classify_save("/x", 409, json!({"error": "conflict"})).unwrap(), Save::Conflict(None));
    }

    #[test]
    fn other_failures_are_errors_and_a_2xx_is_saved() {
        assert_eq!(classify_save("/x", 500, json!({"error": "boom"})).unwrap_err(), "boom");
        assert!(classify_save("/x", 409, json!({"error": "locked"})).is_err());
        assert_eq!(
            classify_save("/x", 200, json!({"hash": "h3"})).unwrap(),
            Save::Saved(json!({"hash": "h3"}))
        );
    }

    #[test]
    fn a_conflict_keeps_yours_with_force_asks_on_a_terminal_and_refuses_otherwise() {
        assert_eq!(on_conflict(true, false), OnConflict::KeepMine);
        assert_eq!(on_conflict(true, true), OnConflict::KeepMine);
        assert_eq!(on_conflict(false, true), OnConflict::Ask);
        assert_eq!(on_conflict(false, false), OnConflict::Refuse);
    }

    #[test]
    fn the_conflict_answers_are_reload_keep_or_abort() {
        assert_eq!(parse_resolution("r"), Some(Resolution::Reload));
        assert_eq!(parse_resolution(" Keep \n"), Some(Resolution::Keep));
        assert_eq!(parse_resolution("A"), Some(Resolution::Abort));
        assert_eq!(parse_resolution(""), None, "an empty answer asks again");
        assert_eq!(parse_resolution("y"), None);
    }

    #[test]
    fn editors_split_into_a_program_and_arguments() {
        assert_eq!(split_command("code --wait"), Some(("code".into(), vec!["--wait".into()])));
        assert_eq!(split_command("vi"), Some(("vi".into(), vec![])));
        assert_eq!(split_command("   "), None);
    }

    #[test]
    fn base_accepts_work_start_and_checkpoints_only() {
        assert_eq!(parse_base("work_start").unwrap(), "work_start");
        assert_eq!(parse_base("checkpoint:3").unwrap(), "checkpoint:3");
        assert!(parse_base("checkpoint:").is_err());
        assert!(parse_base("checkpoint:x").is_err());
        assert!(parse_base("branch:main").is_err());
    }

    #[test]
    fn colour_is_only_for_a_terminal_without_no_color() {
        assert!(!use_colour(false, false), "a pipe or a file gets plain text");
        assert!(!use_colour(true, true), "NO_COLOR turns it off");
        assert!(use_colour(true, false));
    }

    #[test]
    fn a_diff_is_unchanged_when_colour_is_off() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n context\n";
        assert_eq!(colour_diff(diff, false), diff);
    }

    #[test]
    fn colour_paints_the_hunk_lines_and_leaves_the_rest_alone() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n context\n";
        let painted = colour_diff(diff, true);
        assert!(painted.contains("\x1b[1m--- a/x\x1b[0m\n"), "{painted:?}");
        assert!(painted.contains("\x1b[31m-old\x1b[0m\n"));
        assert!(painted.contains("\x1b[32m+new\x1b[0m\n"));
        assert!(painted.contains("\x1b[36m@@ -1 +1 @@\x1b[0m\n"));
        assert!(painted.ends_with(" context\n"));
    }

    #[test]
    fn stat_lines_count_each_file_and_the_totals() {
        let diff = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n-one\n+uno\n two\n";
        let set = parse_unified(diff).unwrap();
        assert_eq!(
            stat_lines(&set),
            vec![
                " a.txt | +1 -1".to_string(),
                "1 files changed, 1 insertions(+), 1 deletions(-)".to_string()
            ]
        );
    }

    #[test]
    fn checkpoints_come_as_an_object_or_a_bare_array() {
        let wrapped = json!({"checkpoints": [{"n": 1}, {"n": 2}]});
        let bare = json!([{"n": 1}]);
        assert_eq!(checkpoints_of(&wrapped).len(), 2);
        assert_eq!(checkpoints_of(&bare).len(), 1);
        assert!(checkpoints_of(&json!({})).is_empty());
    }

    #[test]
    fn a_run_ends_with_the_command_code_124_on_timeout_or_130_when_stopped() {
        assert_eq!(exit_code(&json!({"done": true, "exit_code": 0}), false), 0);
        assert_eq!(exit_code(&json!({"done": true, "exit_code": 3, "timed_out": false}), false), 3);
        assert_eq!(exit_code(&json!({"done": true, "exit_code": null, "timed_out": true}), false), 124);
        assert_eq!(exit_code(&json!({"done": true, "exit_code": null}), true), 130);
        assert_eq!(exit_code(&json!({"done": true, "exit_code": -9}), false), 137);
    }

    #[test]
    fn command_arguments_are_quoted_for_a_shell() {
        let args: Vec<String> = ["cargo", "test", "-p", "apz-ide"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shell_join(&args), "cargo test -p apz-ide");
        let args: Vec<String> = ["sh", "-c", "a && b", "it's", ""].iter().map(|s| s.to_string()).collect();
        assert_eq!(shell_join(&args), "sh -c 'a && b' 'it'\\''s' ''");
    }

    #[test]
    fn items_read_both_list_shapes() {
        assert_eq!(items(&json!([1, 2]), "entries").len(), 2);
        assert_eq!(items(&json!({"entries": [1]}), "entries").len(), 1);
        assert!(items(&json!({"other": 1}), "entries").is_empty());
    }

    #[test]
    fn the_commands_parse_their_arguments() {
        assert!(parse(&["workspaces"]).is_ok());
        assert!(parse(&["ls", "agent:1"]).is_ok());
        assert!(parse(&["ls", "repo:demo@main", "src", "-r"]).is_ok());
        assert!(parse(&["cat", "agent:1", "a.txt", "--hash"]).is_ok());
        assert!(parse(&["put", "agent:1", "a.txt", "--file", "-", "--expected", "h"]).is_ok());
        assert!(parse(&["put", "agent:1", "a.txt", "--file", "-", "--new"]).is_ok());
        assert!(parse(&["put", "agent:1", "a.txt"]).is_err(), "--file is required");
        assert!(parse(&["put", "agent:1", "a.txt", "--file", "-", "--expected", "h", "--new"]).is_err(), "one or the other");
        assert!(parse(&["diff", "agent:1", "--base", "checkpoint:2", "--stat"]).is_ok());
        assert!(parse(&["restore", "agent:1"]).is_err(), "a restore needs its checkpoint number");
        assert!(parse(&["restore", "agent:1", "4", "--yes"]).is_ok());
        assert!(parse(&["search", "agent:1", "todo", "--glob", "*.rs"]).is_ok());
        assert!(parse(&["run", "agent:1", "--", "cargo", "test"]).is_ok());
        assert!(parse(&["run", "agent:1", "--timeout", "5", "--", "ls"]).is_ok());
        assert!(parse(&["run", "agent:1"]).is_err(), "a run needs its command after --");
        assert!(parse(&["audit", "--workspace", "agent:1", "--limit", "10"]).is_ok());
    }
}
