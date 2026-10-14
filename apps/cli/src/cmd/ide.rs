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
    /// An interactive shell in an agent's VM, over the hub's terminal socket. Ctrl+] then d detaches and
    /// keeps the shell running; `--attach` picks it up again and `--kill` ends it. Unix terminals only.
    Term {
        /// The workspace: agent:<cid>. A repo has no VM, so it has no shell.
        workspace: String,
        /// The folder to start the shell in, relative to the workspace root.
        #[arg(long, conflicts_with_all = ["attach", "kill"])]
        cwd: Option<String>,
        /// Re-attach to a running terminal (its id is printed when you detach).
        #[arg(long, value_name = "TERM", conflicts_with = "kill")]
        attach: Option<String>,
        /// Where the output resumes after `--attach`, in bytes (default 0: the oldest output the guest keeps).
        #[arg(long, requires = "attach")]
        offset: Option<u64>,
        /// End a running terminal instead of opening a shell.
        #[arg(long, value_name = "TERM")]
        kill: Option<String>,
        /// Without a terminal on stdin: send stdin a line at a time, with no raw mode.
        #[arg(long)]
        no_tty: bool,
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
        Cmd::Term { workspace, cwd, attach, offset, kill, no_tty } => {
            let code = term_command(ctx, &workspace, cwd, attach, offset, kill, no_tty)?;
            if code != 0 {
                let _ = std::io::stdout().flush();
                std::process::exit(code);
            }
            Ok(())
        }
    }
}

// ---- Terminal -----------------------------------------------------------------------------------

/// `ide term` runs on Unix terminals only: its raw mode and window size are termios.
#[cfg(unix)]
fn term_command(
    ctx: &Ctx,
    workspace: &str,
    cwd: Option<String>,
    attach: Option<String>,
    offset: Option<u64>,
    kill: Option<String>,
    no_tty: bool,
) -> Result<i32, String> {
    term::run(ctx, workspace, cwd, attach, offset, kill, no_tty)
}

#[cfg(not(unix))]
fn term_command(
    _ctx: &Ctx,
    _workspace: &str,
    _cwd: Option<String>,
    _attach: Option<String>,
    _offset: Option<u64>,
    _kill: Option<String>,
    _no_tty: bool,
) -> Result<i32, String> {
    Err("ide term needs a Unix terminal (Linux or macOS); it cannot run on Windows".into())
}

#[cfg(unix)]
mod term {
    use std::io::{self, BufRead, IsTerminal, Read, Write};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, TryRecvError};
    use std::sync::{Mutex, PoisonError, TryLockError};
    use std::time::{Duration, Instant};

    use apz_http::ws::{self, Limits, Message, Socket};
    use apz_ide::WorkspaceId;
    use apz_ide::path::check_dir;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::{Value, json};

    use super::workspace_seg;
    use crate::ctx::Ctx;

    /// The subprotocol the terminal socket asks for.
    const PROTOCOL: &str = "apz.ide.terminal.v1";
    /// How long the hub has to answer an open, an attach or a kill.
    const ANSWER: Duration = Duration::from_secs(30);
    /// How long one socket wait lasts before the keys, signals and resizes are looked at again.
    const TICK: Duration = Duration::from_millis(20);
    /// A keepalive ping after this long without a frame from the client (the hub closes a silent socket
    /// after ten minutes).
    const KEEPALIVE: Duration = Duration::from_secs(5 * 60);
    /// How long output is still shown after the keys end, so a piped command's output is not cut short.
    const INPUT_GRACE: Duration = Duration::from_secs(2);
    /// The escape key: Ctrl+].
    const ESCAPE: u8 = 0x1d;
    /// The size used when the terminal reports none.
    const DEFAULT_SIZE: (u16, u16) = (80, 24);
    /// The largest size the hub takes, in columns or rows.
    const MAX_SIZE: u16 = 1000;

    /// Set by SIGWINCH: the window was resized.
    static WINCH: AtomicBool = AtomicBool::new(false);
    /// Set by SIGHUP or SIGTERM: this process is asked to leave. The shell keeps running.
    static STOP: AtomicBool = AtomicBool::new(false);
    /// The terminal settings from before raw mode, so every exit path can put them back.
    static SAVED: Mutex<Option<libc::termios>> = Mutex::new(None);

    /// `agentplaza ide term WS`: opens a shell, re-attaches to one, or kills one. Returns the exit code.
    pub fn run(
        ctx: &Ctx,
        workspace: &str,
        cwd: Option<String>,
        attach: Option<String>,
        offset: Option<u64>,
        kill: Option<String>,
        no_tty: bool,
    ) -> Result<i32, String> {
        let id = WorkspaceId::from_str(workspace).map_err(|e| format!("{workspace}: {e}"))?;
        if !matches!(id, WorkspaceId::Agent(_)) {
            return Err(format!("{workspace}: a terminal is for agent workspaces; a repo has no VM to open a shell in"));
        }
        let path = format!("/v1/ide/terminal/{}", workspace_seg(workspace)?);
        if let Some(term) = kill {
            kill_term(ctx, &path, &term)?;
            println!("killed {term}");
            return Ok(0);
        }
        let cwd = match cwd {
            Some(cwd) => Some(check_dir(&cwd).map_err(|e| format!("{cwd}: {e}"))?),
            None => None,
        };
        check_stdin(no_tty)?;
        let mut socket = connect(ctx, &path)?;
        let (term, start) = match attach {
            Some(term) => {
                let offset = offset.unwrap_or(0);
                send_frame(&mut socket, &attach_frame(&term, offset))?;
                (await_opened(&mut socket)?, offset)
            }
            None => {
                let (cols, rows) = current_size();
                send_frame(&mut socket, &open_frame(cols, rows, cwd.as_deref()))?;
                (await_opened(&mut socket)?, 0)
            }
        };
        interactive(socket, workspace, &term, start, no_tty)
    }

    /// Input must come from a terminal, unless `--no-tty` asks for plain lines. Checked before the
    /// socket opens, so a refused command never leaves a shell behind.
    fn check_stdin(no_tty: bool) -> Result<(), String> {
        if no_tty || io::stdin().is_terminal() {
            return Ok(());
        }
        Err("stdin is not a terminal: run ide term from a terminal, or pass --no-tty for plain line mode".into())
    }

    fn connect(ctx: &Ctx, path: &str) -> Result<Socket, String> {
        ws::connect(&ctx.client.target, path, Some(PROTOCOL), Duration::from_secs(10), Limits::default())
            .map_err(|e| format!("{} {path}: {e}", ctx.client.base()))
    }

    // ---- Frames -------------------------------------------------------------------------------------

    /// A frame the client sends. `cwd` is left out when it is empty (the workspace root).
    fn open_frame(cols: u16, rows: u16, cwd: Option<&str>) -> Value {
        let mut frame = json!({ "t": "open", "cols": cols, "rows": rows });
        if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
            frame["cwd"] = json!(cwd);
        }
        frame
    }

    fn attach_frame(term: &str, offset: u64) -> Value {
        json!({ "t": "attach", "term": term, "offset": offset })
    }

    fn input_frame(bytes: &[u8]) -> Value {
        json!({ "t": "in", "data": STANDARD.encode(bytes) })
    }

    fn size_frame(cols: u16, rows: u16) -> Value {
        json!({ "t": "resize", "cols": cols, "rows": rows })
    }

    fn kill_frame() -> Value {
        json!({ "t": "kill" })
    }

    fn ping_frame() -> Value {
        json!({ "t": "ping" })
    }

    /// A frame the hub sends.
    #[derive(Debug, PartialEq, Eq)]
    enum TermFrame {
        Opened { term: String },
        Out { data: Vec<u8>, offset: u64 },
        Lost { bytes: u64 },
        Exit { code: Option<i64> },
        Error { message: String },
        Pong,
    }

    fn parse_term_frame(text: &str) -> Result<TermFrame, String> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| format!("the hub sent a frame that is not JSON: {e}"))?;
        match value.get("t").and_then(Value::as_str).unwrap_or_default() {
            "opened" => Ok(TermFrame::Opened { term: field_str(&value, "term")?.to_string() }),
            "out" => Ok(TermFrame::Out {
                data: STANDARD
                    .decode(field_str(&value, "data")?)
                    .map_err(|e| format!("\"data\" is not base64: {e}"))?,
                offset: field_u64(&value, "offset")?,
            }),
            "lost" => Ok(TermFrame::Lost { bytes: field_u64(&value, "bytes")? }),
            "exit" => Ok(TermFrame::Exit { code: value.get("code").and_then(Value::as_i64) }),
            "error" => Ok(TermFrame::Error { message: field_str(&value, "message")?.to_string() }),
            "pong" => Ok(TermFrame::Pong),
            other => Err(format!("the hub sent no frame named {other:?}")),
        }
    }

    fn field_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
        value.get(key).and_then(Value::as_str).ok_or_else(|| format!("\"{key}\" must be a string"))
    }

    fn field_u64(value: &Value, key: &str) -> Result<u64, String> {
        value.get(key).and_then(Value::as_u64).ok_or_else(|| format!("\"{key}\" must be a whole number"))
    }

    fn send_frame(socket: &mut Socket, frame: &Value) -> Result<(), String> {
        socket.send(&Message::Text(frame.to_string())).map_err(|e| e.to_string())
    }

    /// The first answer to an open or an attach: the terminal's id, or the hub's refusal.
    fn await_opened(socket: &mut Socket) -> Result<String, String> {
        let deadline = Instant::now() + ANSWER;
        while Instant::now() < deadline {
            let Some(message) = socket.recv(Duration::from_millis(500)).map_err(|e| e.to_string())? else {
                continue;
            };
            if let Message::Text(text) = message {
                match parse_term_frame(&text)? {
                    TermFrame::Opened { term } => return Ok(term),
                    TermFrame::Error { message } => return Err(message),
                    _ => {}
                }
            }
        }
        Err("the hub did not answer in time".into())
    }

    /// Ends a terminal: attaches to it, sends `kill`, and waits for the hub to close the socket.
    fn kill_term(ctx: &Ctx, path: &str, term: &str) -> Result<(), String> {
        let mut socket = connect(ctx, path)?;
        send_frame(&mut socket, &attach_frame(term, 0))?;
        await_opened(&mut socket)?;
        send_frame(&mut socket, &kill_frame())?;
        let deadline = Instant::now() + ANSWER;
        while Instant::now() < deadline {
            match socket.recv(Duration::from_millis(500)) {
                Ok(Some(Message::Text(text))) => {
                    if let Ok(TermFrame::Error { message }) = parse_term_frame(&text) {
                        return Err(message);
                    }
                }
                Ok(_) => {}
                // The hub closes the socket once the terminal is killed.
                Err(err) if err.kind() == io::ErrorKind::ConnectionAborted => return Ok(()),
                Err(err) => return Err(err.to_string()),
            }
        }
        Err("the hub did not confirm the kill".into())
    }

    // ---- Keys -----------------------------------------------------------------------------------------

    /// Ctrl+] then `d` detaches. Ctrl+] then Ctrl+] sends one Ctrl+]; Ctrl+] then any other key sends both.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Escape {
        pending: bool,
    }

    /// What one read of keys came to: the bytes for the shell, and whether the detach key came (the keys
    /// after it are dropped).
    #[derive(Debug, PartialEq, Eq)]
    struct Keys {
        send: Vec<u8>,
        detach: bool,
    }

    impl Escape {
        fn feed(&mut self, input: &[u8]) -> Keys {
            let mut send = Vec::with_capacity(input.len() + 1);
            for &byte in input {
                if std::mem::take(&mut self.pending) {
                    match byte {
                        b'd' => return Keys { send, detach: true },
                        ESCAPE => send.push(ESCAPE),
                        other => {
                            send.push(ESCAPE);
                            send.push(other);
                        }
                    }
                } else if byte == ESCAPE {
                    self.pending = true;
                } else {
                    send.push(byte);
                }
            }
            Keys { send, detach: false }
        }
    }

    /// Reads the keys on a thread, since a blocking read cannot share the loop with the socket. Each
    /// read's bytes go down the channel: a chunk of keys, or with `lines` one line at a time.
    fn spawn_keys(lines: bool) -> Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdin = io::stdin().lock();
            loop {
                let mut chunk = Vec::new();
                let read = if lines {
                    stdin.read_until(b'\n', &mut chunk)
                } else {
                    let mut buf = [0u8; 4096];
                    let read = stdin.read(&mut buf);
                    if let Ok(n) = &read {
                        chunk.extend_from_slice(&buf[..*n]);
                    }
                    read
                };
                match read {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(chunk).is_err() {
                            break;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });
        rx
    }

    // ---- Terminal settings and size -------------------------------------------------------------------

    /// The size the hub takes: a size of zero (a terminal that reports none) becomes the default, and
    /// anything over the hub's cap is cut to it.
    fn fit_size(cols: u16, rows: u16) -> (u16, u16) {
        let fit = |n: u16, default: u16| if n == 0 { default } else { n.min(MAX_SIZE) };
        (fit(cols, DEFAULT_SIZE.0), fit(rows, DEFAULT_SIZE.1))
    }

    fn window_size(fd: libc::c_int) -> Option<(u16, u16)> {
        let mut size = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: TIOCGWINSZ writes one winsize into the struct it is given.
        let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size as *mut libc::winsize) } == 0;
        ok.then_some((size.ws_col, size.ws_row))
    }

    /// The size of the local terminal (its output, else its input), as the hub takes it.
    fn current_size() -> (u16, u16) {
        let (cols, rows) = [libc::STDOUT_FILENO, libc::STDIN_FILENO]
            .into_iter()
            .find_map(window_size)
            .unwrap_or((0, 0));
        fit_size(cols, rows)
    }

    /// Puts stdin's terminal into raw mode (no echo, no line editing, no signals from keys, no output
    /// processing) and keeps the settings it had.
    fn enter_raw() -> Result<(), String> {
        // SAFETY: an all-zero termios is plain integers; tcgetattr fills it in on success.
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: tcgetattr writes the settings of stdin into `original`.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut original) } != 0 {
            return Err(format!("stdin: {}", io::Error::last_os_error()));
        }
        let mut raw = original;
        raw.c_iflag &=
            !(libc::IGNBRK | libc::BRKINT | libc::PARMRK | libc::ISTRIP | libc::INLCR | libc::IGNCR | libc::ICRNL | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: applies the settings read from this terminal, with only the raw flags changed.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
            return Err(format!("stdin: {}", io::Error::last_os_error()));
        }
        *SAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(original);
        // A panic must not leave the user's terminal raw: the hook restores it, then reports as before.
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_tty();
            before(info);
        }));
        Ok(())
    }

    /// Puts the settings from before raw mode back, if raw mode was entered. Safe to call twice.
    fn restore_tty() {
        let saved = match SAVED.try_lock() {
            Ok(mut guard) => guard.take(),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().take(),
            Err(TryLockError::WouldBlock) => None,
        };
        if let Some(original) = saved {
            // SAFETY: restores the settings that were read before raw mode.
            let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &original) };
        }
    }

    extern "C" fn on_winch(_signal: libc::c_int) {
        WINCH.store(true, Ordering::SeqCst);
    }

    extern "C" fn on_stop(_signal: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    /// SIGWINCH sends the new size; SIGHUP and SIGTERM end this command, leaving the shell running.
    fn catch_session_signals() {
        // SAFETY: the handlers only store to an atomic, which a signal handler may do.
        unsafe {
            libc::signal(libc::SIGWINCH, on_winch as libc::sighandler_t);
            libc::signal(libc::SIGHUP, on_stop as libc::sighandler_t);
            libc::signal(libc::SIGTERM, on_stop as libc::sighandler_t);
        }
    }

    // ---- The session ----------------------------------------------------------------------------------

    /// How an attached session ended.
    #[derive(Debug, PartialEq, Eq)]
    enum End {
        /// The shell ended with this code (None when the guest gave none).
        Exited(Option<i64>),
        /// The client left (detach key, input ended, or a signal): the shell keeps running.
        Detached,
        /// The socket ended or broke before the shell did; the shell may still be running.
        Lost(String),
    }

    /// One attached terminal: the socket, the keys, and where the output stands.
    struct Bridge {
        socket: Socket,
        input: Receiver<Vec<u8>>,
        escape: Escape,
        /// One past the last output byte shown: where a re-attach resumes.
        next_offset: u64,
        last_sent: Instant,
        /// Raw mode is on: notices end with a CR LF, and window size changes are sent.
        raw: bool,
        /// The size is sent once at the start of the session.
        resize_due: bool,
        /// When the keys ended (input closed), if they have.
        input_done: Option<Instant>,
        eol: &'static str,
    }

    impl Bridge {
        /// One pass of the loop: signals, keys, keepalive, then one socket frame or the wait for one.
        /// `Ok(Some(_))` ends the session.
        fn step(&mut self) -> Result<Option<End>, String> {
            if STOP.load(Ordering::SeqCst) {
                return Ok(Some(End::Detached));
            }
            // Both are read: the first resize is due at the start, later ones on SIGWINCH.
            let due = std::mem::take(&mut self.resize_due) | WINCH.swap(false, Ordering::SeqCst);
            if due && self.raw {
                let (cols, rows) = current_size();
                self.send(&size_frame(cols, rows))?;
            }
            if let Some(end) = self.keys()? {
                return Ok(Some(end));
            }
            if self.input_done.is_some_and(|at| at.elapsed() >= INPUT_GRACE) {
                return Ok(Some(End::Detached));
            }
            if self.last_sent.elapsed() >= KEEPALIVE {
                self.send(&ping_frame())?;
            }
            let message = match self.socket.recv(TICK) {
                Ok(Some(message)) => message,
                Ok(None) => return Ok(None),
                Err(err) => return Ok(Some(End::Lost(err.to_string()))),
            };
            let Message::Text(text) = message else {
                return Ok(None);
            };
            match parse_term_frame(&text) {
                Ok(TermFrame::Out { data, offset }) => {
                    show(&data)?;
                    self.next_offset = offset + data.len() as u64;
                    Ok(None)
                }
                Ok(TermFrame::Lost { bytes }) => {
                    self.notice(&format!("{bytes} bytes of output were lost"));
                    Ok(None)
                }
                Ok(TermFrame::Exit { code }) => Ok(Some(End::Exited(code))),
                Ok(TermFrame::Error { message }) => {
                    self.notice(&message);
                    Ok(None)
                }
                Ok(TermFrame::Opened { .. } | TermFrame::Pong) => Ok(None),
                Err(message) => {
                    self.notice(&message);
                    Ok(None)
                }
            }
        }

        /// The keys waiting from the terminal, sent as one input frame per read. The detach key ends it.
        fn keys(&mut self) -> Result<Option<End>, String> {
            loop {
                match self.input.try_recv() {
                    Ok(bytes) => {
                        let keys = self.escape.feed(&bytes);
                        if !keys.send.is_empty() {
                            self.send(&input_frame(&keys.send))?;
                        }
                        if keys.detach {
                            return Ok(Some(End::Detached));
                        }
                    }
                    Err(TryRecvError::Empty) => return Ok(None),
                    Err(TryRecvError::Disconnected) => {
                        self.input_done.get_or_insert_with(Instant::now);
                        return Ok(None);
                    }
                }
            }
        }

        fn send(&mut self, frame: &Value) -> Result<(), String> {
            send_frame(&mut self.socket, frame)?;
            self.last_sent = Instant::now();
            Ok(())
        }

        /// A dim line on stderr (the terminal is raw here, so it ends with a CR LF).
        fn notice(&self, text: &str) {
            let mut err = io::stderr();
            let _ = write!(err, "\x1b[2m{text}\x1b[0m{}", self.eol);
            let _ = err.flush();
        }
    }

    fn show(data: &[u8]) -> Result<(), String> {
        let mut out = io::stdout().lock();
        out.write_all(data).map_err(|e| format!("writing the terminal: {e}"))?;
        out.flush().map_err(|e| format!("writing the terminal: {e}"))
    }

    /// Runs the attached session until it ends, then puts the terminal back and reports how it ended.
    fn interactive(socket: Socket, workspace: &str, term: &str, offset: u64, no_tty: bool) -> Result<i32, String> {
        let raw = !no_tty;
        if raw {
            enter_raw()?;
        }
        catch_session_signals();
        let mut bridge = Bridge {
            socket,
            input: spawn_keys(no_tty),
            escape: Escape::default(),
            next_offset: offset,
            last_sent: Instant::now(),
            raw,
            resize_due: raw,
            input_done: None,
            eol: if raw { "\r\n" } else { "\n" },
        };
        let end = loop {
            match bridge.step() {
                Ok(Some(end)) => break end,
                Ok(None) => {}
                Err(message) => break End::Lost(message),
            }
        };
        restore_tty();
        match end {
            End::Exited(code) => Ok(exit_status(code)),
            End::Detached => {
                bridge.socket.close(1000, "detached");
                eprintln!("detached: {term} keeps running");
                eprintln!("{}", detach_hints(workspace, term, bridge.next_offset));
                Ok(0)
            }
            End::Lost(message) => {
                bridge.socket.close(1000, "lost");
                eprintln!("the terminal socket ended: {message}; if the shell is still running:");
                eprintln!("{}", detach_hints(workspace, term, bridge.next_offset));
                Ok(1)
            }
        }
    }

    /// How to come back to a shell that is still running: re-attach at the offset, or end it.
    fn detach_hints(workspace: &str, term: &str, offset: u64) -> String {
        format!(
            "re-attach: agentplaza ide term {workspace} --attach {term} --offset {offset}\nend it: agentplaza ide term {workspace} --kill {term}"
        )
    }

    /// The process exit code for a shell's code: a signal's negative code becomes 128 plus the signal.
    fn exit_status(code: Option<i64>) -> i32 {
        match code {
            Some(code) if code < 0 => (128 - code).clamp(0, 255) as i32,
            Some(code) => code.clamp(0, 255) as i32,
            None => 1,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_client_frames_have_the_documented_shape() {
            assert_eq!(open_frame(80, 24, None), json!({"t": "open", "cols": 80, "rows": 24}));
            assert_eq!(open_frame(80, 24, Some("src")), json!({"t": "open", "cols": 80, "rows": 24, "cwd": "src"}));
            assert_eq!(open_frame(80, 24, Some("")), json!({"t": "open", "cols": 80, "rows": 24}), "an empty cwd is the root");
            assert_eq!(attach_frame("term-1", 5), json!({"t": "attach", "term": "term-1", "offset": 5}));
            assert_eq!(input_frame(b"ls\r"), json!({"t": "in", "data": "bHMN"}));
            assert_eq!(kill_frame(), json!({"t": "kill"}));
            assert_eq!(ping_frame(), json!({"t": "ping"}));
        }

        #[test]
        fn keys_survive_the_base64_round_trip() {
            let bytes: Vec<u8> = (0u8..=255).collect();
            let frame = input_frame(&bytes);
            let back = STANDARD.decode(frame["data"].as_str().unwrap_or_default()).unwrap_or_default();
            assert_eq!(back, bytes);
        }

        #[test]
        fn the_hubs_frames_parse() {
            assert_eq!(
                parse_term_frame(r#"{"t":"opened","term":"term-1"}"#),
                Ok(TermFrame::Opened { term: "term-1".into() })
            );
            assert_eq!(
                parse_term_frame(r#"{"t":"out","data":"aGk=","offset":7}"#),
                Ok(TermFrame::Out { data: b"hi".to_vec(), offset: 7 })
            );
            assert_eq!(parse_term_frame(r#"{"t":"lost","bytes":12}"#), Ok(TermFrame::Lost { bytes: 12 }));
            assert_eq!(parse_term_frame(r#"{"t":"exit","code":3}"#), Ok(TermFrame::Exit { code: Some(3) }));
            assert_eq!(parse_term_frame(r#"{"t":"exit","code":null}"#), Ok(TermFrame::Exit { code: None }));
            assert_eq!(
                parse_term_frame(r#"{"t":"error","message":"no agent 9"}"#),
                Ok(TermFrame::Error { message: "no agent 9".into() })
            );
            assert_eq!(parse_term_frame(r#"{"t":"pong"}"#), Ok(TermFrame::Pong));
        }

        #[test]
        fn a_bad_hub_frame_is_an_error_not_a_guess() {
            assert!(parse_term_frame("not json").is_err());
            assert!(parse_term_frame(r#"{"t":"nope"}"#).is_err());
            assert!(parse_term_frame(r#"{"t":"out","data":"!!","offset":0}"#).is_err(), "data is base64");
            assert!(parse_term_frame(r#"{"t":"out","data":"aGk="}"#).is_err(), "an out needs its offset");
            assert!(parse_term_frame(r#"{"t":"lost"}"#).is_err());
        }

        #[test]
        fn ctrl_bracket_then_d_detaches_and_keeps_the_keys_before_it() {
            let mut escape = Escape::default();
            assert_eq!(escape.feed(b"ls"), Keys { send: b"ls".to_vec(), detach: false });
            assert_eq!(escape.feed(&[ESCAPE, b'd']), Keys { send: vec![], detach: true });
            let mut escape = Escape::default();
            assert_eq!(escape.feed(b"a\x1dd"), Keys { send: b"a".to_vec(), detach: true });
        }

        #[test]
        fn the_detach_sequence_can_be_split_across_reads() {
            let mut escape = Escape::default();
            assert_eq!(escape.feed(&[ESCAPE]), Keys { send: vec![], detach: false });
            assert_eq!(escape.feed(b"d"), Keys { send: vec![], detach: true });
        }

        #[test]
        fn a_plain_d_is_only_a_key_and_ctrl_bracket_otherwise_sends_both() {
            let mut escape = Escape::default();
            assert_eq!(escape.feed(b"d"), Keys { send: b"d".to_vec(), detach: false });
            assert_eq!(escape.feed(&[ESCAPE, b'x']), Keys { send: vec![ESCAPE, b'x'], detach: false });
            assert_eq!(escape.feed(&[ESCAPE, ESCAPE]), Keys { send: vec![ESCAPE], detach: false });
            assert_eq!(escape.feed(b"d"), Keys { send: b"d".to_vec(), detach: false }, "the escape is over");
            assert_eq!(escape.feed(&[ESCAPE, ESCAPE, b'd']), Keys { send: vec![ESCAPE, b'd'], detach: false });
        }

        #[test]
        fn the_size_message_carries_a_size_the_hub_takes() {
            assert_eq!(fit_size(120, 40), (120, 40));
            assert_eq!(fit_size(0, 0), DEFAULT_SIZE, "a terminal that reports no size gets 80 by 24");
            assert_eq!(fit_size(5000, 50), (MAX_SIZE, 50));
            let (cols, rows) = fit_size(120, 40);
            assert_eq!(size_frame(cols, rows), json!({"t": "resize", "cols": 120, "rows": 40}));
        }

        #[test]
        fn a_shell_exit_code_is_the_process_exit_code() {
            assert_eq!(exit_status(Some(0)), 0);
            assert_eq!(exit_status(Some(3)), 3);
            assert_eq!(exit_status(Some(-9)), 137, "a signal is 128 plus its number");
            assert_eq!(exit_status(None), 1, "no code from the guest is a failure");
        }

        #[test]
        fn the_hints_name_the_terminal_and_the_offset_to_resume_at() {
            let hints = detach_hints("agent:12", "term-1", 42);
            assert!(hints.contains("--attach term-1 --offset 42"), "{hints}");
            assert!(hints.contains("--kill term-1"), "{hints}");
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

    #[test]
    fn the_term_command_parses_its_forms() {
        assert!(parse(&["term", "agent:1"]).is_ok());
        assert!(parse(&["term", "agent:1", "--cwd", "src", "--no-tty"]).is_ok());
        assert!(parse(&["term", "agent:1", "--attach", "term-1", "--offset", "4"]).is_ok());
        assert!(parse(&["term", "agent:1", "--kill", "term-1"]).is_ok());
        assert!(parse(&["term"]).is_err(), "a workspace is required");
        assert!(parse(&["term", "agent:1", "--offset", "4"]).is_err(), "an offset needs --attach");
        assert!(parse(&["term", "agent:1", "--kill", "t", "--attach", "t"]).is_err(), "kill and attach conflict");
        assert!(parse(&["term", "agent:1", "--kill", "t", "--cwd", "src"]).is_err(), "kill takes no folder");
    }
}
