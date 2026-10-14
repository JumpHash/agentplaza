//! `agentplaza skills`: the skill library. A skill is a folder: a SKILL.md and the files it needs. Each save is a
//! version, checked and scored; the live version reaches the agents it is bound to (a role, or every agent through
//! the plaza binding). Import takes a zip or a folder of skills, GitHub takes a repository path, and export writes
//! the zip back. The same hub routes the web's skill editor uses.
//!
//! A folder's files are sent as the hub takes them: hidden files and symbolic links are left out, and a file over
//! 5 MB is skipped with a note. A folder over 200 files or 20 MB is refused, since a partial skill is not what was
//! asked for.

use std::fs::Metadata;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::cmd::models::items;
use crate::cmd::prompts::{call, call_status, dash, get_bytes, json_reply, print_diff_lines, print_events, print_report, read_text, str_list, version_text, count_op};
use crate::ctx::{self, confirm, Ctx};

/// The largest file a version may hold, and the most files and bytes in all (the hub's limits).
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const MAX_FILES: usize = 200;
/// The largest zip an import may be (the hub's upload limit), and the time an upload may take.
const MAX_UPLOAD: u64 = 100 * 1024 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Text up to this size is sent with its text for the checks (the hub's own limit).
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The skills, with their live version's score and their bindings.
    List {
        /// The archived skills instead of the live ones.
        #[arg(long)]
        archived: bool,
    },
    /// One skill: its fields and versions. With --version, that version's findings.
    Show {
        /// The skill's id.
        id: String,
        /// A version to show in full.
        #[arg(long)]
        version: Option<u32>,
    },
    /// One file of a version, to stdout (its bytes as they are).
    File {
        /// The skill's id.
        id: String,
        /// The version number.
        version: u32,
        /// The file's path inside the skill, e.g. scripts/run.sh.
        path: String,
    },
    /// Create a skill at version 1 from a SKILL.md (its other files come with `save`).
    Create {
        /// The title.
        #[arg(long)]
        title: String,
        /// The id, a slug. Default: the title's slug.
        #[arg(long)]
        id: Option<String>,
        /// Tags, comma-separated.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// A binding: role:<role>, or plaza. Repeatable.
        #[arg(long = "bind", value_name = "BINDING")]
        bind: Vec<String>,
        /// Bind it to every agent (the plaza binding).
        #[arg(long)]
        every: bool,
        /// The SKILL.md file, or - for stdin.
        #[arg(long)]
        file: PathBuf,
    },
    /// Change a skill's title or tags. Only the flags given change.
    Edit {
        /// The skill's id.
        id: String,
        #[arg(long)]
        title: Option<String>,
        /// The tags, comma-separated (replaces the list).
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    /// Save a new version from a folder: its SKILL.md and every other file in it. --base is the version you started
    /// from (default: the latest); a newer version since then refuses the save.
    Save {
        /// The skill's id.
        id: String,
        /// The skill's folder (it holds SKILL.md).
        dir: PathBuf,
        /// A note on what changed.
        #[arg(long)]
        note: Option<String>,
        /// The version you started from.
        #[arg(long)]
        base: Option<u32>,
    },
    /// Replace a skill's bindings: --bind for each role or plaza, --every for every agent, --clear for none.
    Bindings {
        /// The skill's id.
        id: String,
        /// A binding: role:<role>, or plaza. Repeatable.
        #[arg(long = "bind", value_name = "BINDING")]
        bind: Vec<String>,
        /// Bind it to every agent (the plaza binding).
        #[arg(long)]
        every: bool,
        /// Remove every binding.
        #[arg(long, conflicts_with_all = ["bind", "every"])]
        clear: bool,
    },
    /// Make a version the live one (default: the latest). A version with errors is refused.
    Publish {
        /// The skill's id.
        id: String,
        /// The version number.
        version: Option<u32>,
    },
    /// Take the live version down. The skill keeps its versions.
    Unpublish {
        /// The skill's id.
        id: String,
    },
    /// Archive a skill: it leaves the live set. Asks first unless --yes.
    Archive {
        /// The skill's id.
        id: String,
        /// Archive without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Bring an archived skill back.
    Restore {
        /// The skill's id.
        id: String,
    },
    /// The diff between two versions: SKILL.md line by line, and the files added, removed or changed.
    Diff {
        /// The skill's id.
        id: String,
        /// The older version.
        from: u32,
        /// The newer version.
        to: u32,
    },
    /// The skill's history: each change, who made it, and when.
    History {
        /// The skill's id.
        id: String,
    },
    /// Score a folder before saving it: the score and its findings, with the file and line of each.
    Check {
        /// The skill's folder (it holds SKILL.md).
        dir: PathBuf,
    },
    /// The rules every skill is scored against.
    Rules,
    /// Import skills from a zip (or a .skill file) or a folder: each folder with a SKILL.md. Previews the skills
    /// found, then imports the picks after you confirm.
    Import {
        /// The zip, or a folder of skills.
        path: PathBuf,
        /// The skills to import, by their folder (comma-separated). Default: each one without errors that is new.
        #[arg(long, value_delimiter = ',')]
        pick: Vec<String>,
        /// A binding for the imported skills: role:<role>, or plaza. Repeatable.
        #[arg(long = "bind", value_name = "BINDING")]
        bind: Vec<String>,
        /// Bind them to every agent (the plaza binding).
        #[arg(long)]
        every: bool,
        /// Import without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Preview the skills under a GitHub URL (a repository, a branch or a folder in it), then import the picks.
    GithubPreview {
        /// The GitHub URL.
        url: String,
        /// The skills to import, by their folder (comma-separated).
        #[arg(long, value_delimiter = ',')]
        pick: Vec<String>,
        /// A binding for the imported skills: role:<role>, or plaza. Repeatable.
        #[arg(long = "bind", value_name = "BINDING")]
        bind: Vec<String>,
        /// Bind them to every agent (the plaza binding).
        #[arg(long)]
        every: bool,
        /// Import without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Whether a GitHub-imported skill's ref points at a newer commit.
    GithubCheck {
        /// The skill's id.
        id: String,
    },
    /// Import the newest commit of a GitHub-imported skill as a new version (when it moved). Asks first unless --yes.
    GithubUpdate {
        /// The skill's id.
        id: String,
        /// Update without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Export a skill's version (default: the live one, else the latest) as a zip, or several skills with --ids.
    Export {
        /// The skill's id.
        #[arg(required_unless_present = "ids", conflicts_with = "ids")]
        id: Option<String>,
        /// Several skills' live versions in one zip, by id (comma-separated).
        #[arg(long, conflicts_with = "id")]
        ids: Option<String>,
        /// The version to export (with an ID).
        #[arg(long, conflicts_with = "ids")]
        version: Option<u32>,
        /// The zip file to write.
        #[arg(short = 'o', long, value_name = "FILE")]
        out: PathBuf,
        /// Overwrite the file when it exists.
        #[arg(long)]
        force: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { archived } => list(ctx, archived),
        Cmd::Show { id, version } => show(ctx, &id, version),
        Cmd::File { id, version, path } => {
            let bytes = get_bytes(ctx, &format!("{}/versions/{version}/files/{}", skill_path(&id), encode_path(&path)))?;
            std::io::stdout().write_all(&bytes).map_err(|e| e.to_string())
        }
        Cmd::Create { title, id, tags, bind, every, file } => {
            let md = read_text(&file)?;
            let bindings = bindings_list(bind, every)?;
            let made = ctx.client.post("/v1/skills", &create_body(&title, id.as_deref(), &clean_tags(&tags), &bindings, &md))?;
            ctx.out(&made, |m| println!("created {} (v1)", ctx::s(&m["skill"], "id")));
            Ok(())
        }
        Cmd::Edit { id, title, tags } => {
            if title.is_none() && tags.is_none() {
                return Err("give --title or --tags".into());
            }
            let detail = ctx.client.get(&skill_path(&id))?;
            let body = edit_body(&detail["skill"], title.as_deref(), tags.as_deref());
            let updated = ctx.client.put(&skill_path(&id), &body)?;
            ctx.out(&updated, |_| println!("updated {id}"));
            Ok(())
        }
        Cmd::Save { id, dir, note, base } => save(ctx, &id, &dir, note.as_deref(), base),
        Cmd::Bindings { id, bind, every, clear } => {
            if !clear && bind.is_empty() && !every {
                return Err("give --bind, --every or --clear: the list is replaced".into());
            }
            let list = if clear { Vec::new() } else { bindings_list(bind, every)? };
            let updated = ctx.client.put(&format!("{}/bindings", skill_path(&id)), &bindings_body(&list))?;
            ctx.out(&updated, |u| println!("{id} is bound to {}", bindings_text(&u["skill"]["bindings"])));
            Ok(())
        }
        Cmd::Publish { id, version } => {
            let version = match version {
                Some(v) => v,
                None => latest_version(ctx, &id)?,
            };
            let reply = ctx.client.post(&format!("{}/publish", skill_path(&id)), &publish_body(version))?;
            ctx.out(&reply, |_| println!("published v{version} of {id}"));
            Ok(())
        }
        Cmd::Unpublish { id } => {
            let reply = call(ctx, "POST", &format!("{}/unpublish", skill_path(&id)), None)?;
            ctx.out(&reply, |_| println!("unpublished {id}"));
            Ok(())
        }
        Cmd::Archive { id, yes } => {
            if !confirm(yes, &format!("Archive skill {id}? It leaves the live set until it is restored."))? {
                return Err("cancelled".into());
            }
            let reply = call(ctx, "POST", &format!("{}/archive", skill_path(&id)), None)?;
            ctx.out(&reply, |_| println!("archived {id}"));
            Ok(())
        }
        Cmd::Restore { id } => {
            let reply = call(ctx, "POST", &format!("{}/restore", skill_path(&id)), None)?;
            ctx.out(&reply, |_| println!("restored {id}"));
            Ok(())
        }
        Cmd::Diff { id, from, to } => diff(ctx, &id, from, to),
        Cmd::History { id } => {
            let history = ctx.client.get(&format!("{}/history", skill_path(&id)))?;
            ctx.out(&history, |h| print_events(&h["events"]));
            Ok(())
        }
        Cmd::Check { dir } => {
            let folder = read_folder(&dir)?;
            note_skipped(&folder.notes);
            let files: Vec<Value> = folder.files.iter().map(check_file_json).collect();
            let report = ctx.client.post("/v1/skills/check", &json!({ "skill_md": folder.skill_md, "files": files }))?;
            ctx.out(&report, print_check);
            Ok(())
        }
        Cmd::Rules => {
            let rules = ctx.client.get("/v1/skills/rules")?;
            ctx.out(&rules, print_rules);
            Ok(())
        }
        Cmd::Import { path, pick, bind, every, yes } => {
            let (bytes, notes) = if path.is_dir() {
                dir_zip(&path)?
            } else {
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                (bytes, Vec::new())
            };
            note_skipped(&notes);
            let preview = post_zip(ctx, "/v1/skills/import/preview", &bytes)?;
            import_picked(ctx, &preview, &pick, bind, every, yes)
        }
        Cmd::GithubPreview { url, pick, bind, every, yes } => {
            let preview = ctx.client.post("/v1/skills/github/preview", &json!({ "url": url }))?;
            import_picked(ctx, &preview, &pick, bind, every, yes)
        }
        Cmd::GithubCheck { id } => {
            let r = ctx.client.post(&format!("{}/github/check", skill_path(&id)), &json!({}))?;
            ctx.out(&r, |r| {
                let state = if r["update_available"].as_bool() == Some(true) { "a newer commit is out" } else { "up to date" };
                println!("current {} · latest {} · {state}", short(ctx::s(r, "current")), short(ctx::s(r, "latest")));
            });
            Ok(())
        }
        Cmd::GithubUpdate { id, yes } => {
            if !confirm(yes, &format!("Import the newest commit of {id} as a new version, if its ref moved."))? {
                return Err("cancelled".into());
            }
            let r = ctx.client.post(&format!("{}/github/update", skill_path(&id)), &json!({}))?;
            ctx.out(&r, |r| {
                if r["created"].as_bool() == Some(true) {
                    println!("updated {id} to {}", version_text(&r["version"]["version"]));
                } else {
                    println!("no change: {id} is already at the latest commit");
                }
            });
            Ok(())
        }
        Cmd::Export { id, ids, version, out, force } => export(ctx, id.as_deref(), ids.as_deref(), version, &out, force),
    }
}

// ---- Paths, bodies and small helpers ----------------------------------------------------------

/// A skill's path.
fn skill_path(id: &str) -> String {
    format!("/v1/skills/{}", ctx::seg(id))
}

/// A file path inside a skill, each segment encoded and the slashes kept.
fn encode_path(path: &str) -> String {
    path.split('/').map(ctx::seg).collect::<Vec<_>>().join("/")
}

/// The export path: one version of one skill, or the live versions of several (`ids` keeps its commas).
fn export_path(id: Option<&str>, ids: Option<&str>, version: u32) -> String {
    match (id, ids) {
        (Some(id), _) => format!("{}/versions/{version}/export", skill_path(id)),
        (None, Some(ids)) => format!("/v1/skills/export?ids={}", ids.split(',').filter(|s| !s.is_empty()).map(ctx::seg).collect::<Vec<_>>().join(",")),
        (None, None) => String::new(),
    }
}

/// A binding the hub takes: `plaza`, or `role:<role>` with a lower-case role id.
fn valid_binding(b: &str) -> bool {
    b == "plaza" || b.strip_prefix("role:").is_some_and(|r| !r.is_empty() && r.len() <= 64 && r.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'))
}

/// The bindings asked for: each given one, plus `plaza` for --every, checked, sorted and without repeats.
fn bindings_list(bind: Vec<String>, every: bool) -> Result<Vec<String>, String> {
    let mut out = bind;
    if every {
        out.push("plaza".into());
    }
    if let Some(bad) = out.iter().find(|b| !valid_binding(b)) {
        return Err(format!("{bad} is not a binding: plaza, or role:<role>"));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// A skill's bindings as text, or "unbound".
fn bindings_text(v: &Value) -> String {
    let list = str_list(v);
    if list.is_empty() {
        "unbound".into()
    } else {
        list
    }
}

/// Tags without blanks, each trimmed.
fn clean_tags(tags: &[String]) -> Vec<String> {
    tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

/// The first twelve characters of a commit id.
fn short(sha: &str) -> String {
    sha.chars().take(12).collect()
}

/// A skill's source as text: "built", "upload", or the GitHub repository, path and commit.
fn source_text(source: &Value) -> String {
    match ctx::s(source, "kind") {
        "" => "—".into(),
        "github" => format!("github {} {} @{}", ctx::s(source, "repo"), dash(ctx::s(source, "path").to_string()), short(ctx::s(source, "commit"))),
        other => other.to_string(),
    }
}

/// The newest version number of a skill, read from its detail.
fn latest_version(ctx: &Ctx, id: &str) -> Result<u32, String> {
    let detail = ctx.client.get(&skill_path(id))?;
    detail["skill"]["latest_version"].as_u64().map(|v| v as u32).ok_or_else(|| format!("{id}: no latest version"))
}

/// The version an export takes when none is given: the live one, else the latest.
fn live_or_latest(ctx: &Ctx, id: &str) -> Result<u32, String> {
    let detail = ctx.client.get(&skill_path(id))?;
    let s = &detail["skill"];
    s["active_version"].as_u64().or(s["latest_version"].as_u64()).map(|v| v as u32).ok_or_else(|| format!("{id}: no version"))
}

fn note_skipped(notes: &[String]) {
    for note in notes {
        eprintln!("skipped {note}");
    }
}

// ---- Bodies -----------------------------------------------------------------------------------

fn create_body(title: &str, id: Option<&str>, tags: &[String], bindings: &[String], skill_md: &str) -> Value {
    let mut out = json!({ "title": title, "tags": tags, "bindings": bindings, "skill_md": skill_md, "files": [], "note": "" });
    if let Some(id) = id {
        out["id"] = json!(id);
    }
    out
}

/// The edit body: the current title and tags, with the given ones replacing them.
fn edit_body(current: &Value, title: Option<&str>, tags: Option<&[String]>) -> Value {
    json!({
        "title": title.unwrap_or(ctx::s(current, "title")),
        "tags": tags.map(clean_tags).map(|t| json!(t)).unwrap_or_else(|| current["tags"].clone()),
    })
}

fn bindings_body(bindings: &[String]) -> Value {
    json!({ "bindings": bindings })
}

fn publish_body(version: u32) -> Value {
    json!({ "version": version })
}

/// A file as a version takes it: its path, its executable bit, and its bytes in base64.
fn file_json(f: &Entry) -> Value {
    json!({ "path": f.path, "executable": f.executable, "content_b64": base64(&f.bytes) })
}

/// A new version's body: SKILL.md, the files, the note and the version it was made from.
fn version_body(skill_md: &str, files: &[Entry], note: &str, base: u32) -> Value {
    json!({ "skill_md": skill_md, "files": files.iter().map(file_json).collect::<Vec<_>>(), "note": note, "base_version": base })
}

/// One file as the check takes it: its path, size, executable bit, and its text when it is text of a checkable size.
fn check_file_json(f: &Entry) -> Value {
    let text = (f.bytes.len() as u64 <= MAX_TEXT_BYTES).then(|| String::from_utf8(f.bytes.clone()).ok()).flatten();
    json!({ "path": f.path, "size": f.bytes.len(), "executable": f.executable, "text": text })
}

fn commit_body(preview_id: &str, picks: &[String], bindings: &[String]) -> Value {
    json!({ "preview_id": preview_id, "picks": picks, "bindings": bindings })
}

// ---- Folders ----------------------------------------------------------------------------------

/// One file to send: its path inside the skill, its executable bit, and its bytes.
struct Entry {
    path: String,
    executable: bool,
    bytes: Vec<u8>,
}

/// A file found by the walk, not yet read.
struct Found {
    path: String,
    full: PathBuf,
    size: u64,
    executable: bool,
}

/// What the walk found: the files, and the notes of what it could not take (symbolic links).
struct Walk {
    files: Vec<Found>,
    notes: Vec<String>,
}

/// A skill folder read for a save or a check: SKILL.md, the files that go with it, and the notes of what was left out.
struct Folder {
    skill_md: String,
    files: Vec<Entry>,
    notes: Vec<String>,
}

/// Why a file of a folder is not sent.
#[derive(Debug, PartialEq, Eq)]
enum Skip {
    Hidden,
    TooLarge,
}

/// A path with a dot-named part: a hidden file or folder.
fn is_hidden(rel: &str) -> bool {
    rel.split('/').any(|part| part.starts_with('.'))
}

/// Why a file is left out of what is sent: it is hidden, or larger than the hub takes. None when it is sent.
fn skip_reason(rel: &str, size: u64) -> Option<Skip> {
    if is_hidden(rel) {
        Some(Skip::Hidden)
    } else if size > MAX_FILE_BYTES {
        Some(Skip::TooLarge)
    } else {
        None
    }
}

/// A path under `root` as `/`-separated segments. A name that is not UTF-8 is refused.
fn relative(root: &Path, full: &Path) -> Result<String, String> {
    let rel = full.strip_prefix(root).map_err(|_| format!("{}: outside the folder", full.display()))?;
    let mut parts = Vec::new();
    for part in rel.components() {
        parts.push(part.as_os_str().to_str().ok_or_else(|| format!("{}: a name that is not UTF-8", full.display()))?);
    }
    Ok(parts.join("/"))
}

#[cfg(unix)]
fn is_executable(meta: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &Metadata) -> bool {
    false
}

/// Every file under `root`, sorted by path. Hidden folders are not entered, and symbolic links are not followed
/// (each is noted); the caller decides about the files it found.
fn walk(root: &Path) -> Result<Walk, String> {
    let mut out = Walk { files: Vec::new(), notes: Vec::new() };
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&dir)
            .and_then(|read| read.collect::<Result<_, _>>())
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let full = entry.path();
            let rel = relative(root, &full)?;
            let kind = entry.file_type().map_err(|e| format!("{}: {e}", full.display()))?;
            if kind.is_symlink() {
                out.notes.push(format!("{rel}: a symbolic link"));
            } else if kind.is_dir() {
                if !is_hidden(&rel) {
                    dirs.push(full);
                }
            } else if kind.is_file() {
                let meta = entry.metadata().map_err(|e| format!("{}: {e}", full.display()))?;
                out.files.push(Found { path: rel, full, size: meta.len(), executable: is_executable(&meta) });
            }
        }
    }
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// A skill folder for a save or a check: its SKILL.md, and the other files the hub takes, read. Refused when the
/// folder holds more than the hub's file count or total size.
fn read_folder(root: &Path) -> Result<Folder, String> {
    let md_path = root.join("SKILL.md");
    let skill_md = std::fs::read_to_string(&md_path).map_err(|e| format!("{}: {e}", md_path.display()))?;
    let Walk { files, mut notes } = walk(root)?;
    let mut kept = Vec::new();
    for f in files {
        if f.path == "SKILL.md" {
            continue;
        }
        match skip_reason(&f.path, f.size) {
            Some(Skip::Hidden) => {}
            Some(Skip::TooLarge) => notes.push(format!("{}: larger than 5 MB", f.path)),
            None => kept.push(f),
        }
    }
    if kept.len() > MAX_FILES {
        return Err(format!("{}: a skill holds at most {MAX_FILES} files, and this folder has {}", root.display(), kept.len()));
    }
    let total: u64 = kept.iter().map(|f| f.size).sum();
    if total > MAX_TOTAL_BYTES {
        return Err(format!("{}: a skill is at most 20 MB, and this folder is {} MB", root.display(), total / (1024 * 1024)));
    }
    let files = kept.into_iter().map(|f| read_entry(&f)).collect::<Result<Vec<_>, _>>()?;
    Ok(Folder { skill_md, files, notes })
}

fn read_entry(f: &Found) -> Result<Entry, String> {
    let bytes = std::fs::read(&f.full).map_err(|e| format!("{}: {e}", f.full.display()))?;
    Ok(Entry { path: f.path.clone(), executable: f.executable, bytes })
}

/// A whole folder as a zip for an import: every file the hub takes, paths relative to the folder. The notes say
/// what was left out.
fn dir_zip(root: &Path) -> Result<(Vec<u8>, Vec<String>), String> {
    let Walk { files, mut notes } = walk(root)?;
    let mut entries = Vec::new();
    let mut total = 0u64;
    for f in files {
        match skip_reason(&f.path, f.size) {
            Some(Skip::Hidden) => {}
            Some(Skip::TooLarge) => notes.push(format!("{}: larger than 5 MB", f.path)),
            None => {
                total += f.size;
                entries.push(f);
            }
        }
    }
    if total > MAX_UPLOAD {
        return Err(format!("{}: the folder is larger than 100 MB", root.display()));
    }
    let packed = entries.iter().map(read_entry).collect::<Result<Vec<_>, _>>()?;
    Ok((stored_zip(&packed)?, notes))
}

// ---- Encoders for the upload ------------------------------------------------------------------

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(BASE64[(n >> 18) as usize & 63] as char);
        out.push(BASE64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { BASE64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { BASE64[n as usize & 63] as char } else { '=' });
    }
    out
}

const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

/// CRC-32 (the zip's checksum).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in bytes {
        crc = CRC_TABLE[((crc ^ u32::from(*b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// A zip of the entries, each stored (no compression), as the hub's import reads it. The date is 1980-01-01, the
/// zip's epoch, so the archive is the same for the same files.
fn stored_zip(entries: &[Entry]) -> Result<Vec<u8>, String> {
    const DOS_DATE: u16 = 0x0021;
    const UTF8_NAMES: u16 = 0x0800;
    let count = u16::try_from(entries.len()).map_err(|_| "too many files for one zip".to_string())?;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let name = e.path.as_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| format!("{}: the name is too long", e.path))?;
        let size = u32::try_from(e.bytes.len()).map_err(|_| format!("{}: too large for a zip entry", e.path))?;
        let crc = crc32(&e.bytes);
        let offset = u32::try_from(out.len()).map_err(|_| "the zip is too large".to_string())?;
        let mode: u32 = if e.executable { 0o755 } else { 0o644 };
        let attrs = (0o100000 | mode) << 16;
        // The local header, the name and the bytes.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&UTF8_NAMES.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&e.bytes);
        // Its central directory entry.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&UTF8_NAMES.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&attrs.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let central_at = u32::try_from(out.len()).map_err(|_| "the zip is too large".to_string())?;
    let central_size = u32::try_from(central.len()).map_err(|_| "the zip is too large".to_string())?;
    out.extend_from_slice(&central);
    // The end of the central directory.
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

// ---- Commands ---------------------------------------------------------------------------------

fn list(ctx: &Ctx, archived: bool) -> Result<(), String> {
    let path = if archived { "/v1/skills?archived=true" } else { "/v1/skills" };
    let list = ctx.client.get(path)?;
    ctx.out(&list, |v| {
        let rows: Vec<Vec<String>> = items(&v["skills"])
            .map(|s| {
                vec![
                    ctx::s(s, "id").to_string(),
                    ctx::s(s, "title").to_string(),
                    version_text(&s["active_version"]),
                    version_text(&s["latest_version"]),
                    dash(ctx::text(s, "score")),
                    bindings_text(&s["bindings"]),
                    ctx::s(&s["source"], "kind").to_string(),
                ]
            })
            .collect();
        ctx::table(&["id", "title", "live", "latest", "score", "bindings", "source"], &rows);
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str, version: Option<u32>) -> Result<(), String> {
    match version {
        None => {
            let detail = ctx.client.get(&skill_path(id))?;
            ctx.out(&detail, print_detail);
        }
        Some(v) => {
            let view = ctx.client.get(&format!("{}/versions/{v}", skill_path(id)))?;
            ctx.out(&view, print_version);
        }
    }
    Ok(())
}

fn print_detail(d: &Value) {
    let s = &d["skill"];
    let live = if s["active_version"].is_null() { "not published".to_string() } else { version_text(&s["active_version"]) };
    let mut pairs = vec![
        ("id", ctx::s(s, "id").to_string()),
        ("name", dash(ctx::s(s, "name").to_string())),
        ("title", ctx::s(s, "title").to_string()),
        ("description", dash(ctx::s(s, "description").to_string())),
        ("tags", dash(str_list(&s["tags"]))),
        ("bindings", bindings_text(&s["bindings"])),
        ("live", live),
        ("latest", version_text(&s["latest_version"])),
        ("best", version_text(&s["best_version"])),
        ("score", dash(ctx::text(s, "score"))),
        ("source", source_text(&s["source"])),
        ("updated", ctx::when(&s["updated_ms"])),
    ];
    if !s["archived_ms"].is_null() {
        pairs.push(("archived", ctx::when(&s["archived_ms"])));
    }
    ctx::fields(&pairs);
    println!();
    let rows: Vec<Vec<String>> = items(&d["versions"])
        .map(|v| {
            vec![
                version_text(&v["version"]),
                ctx::text(v, "score"),
                ctx::text(v, "errors"),
                ctx::text(v, "warnings"),
                ctx::text(v, "file_count"),
                ctx::s(v, "author").to_string(),
                ctx::when(&v["created_ms"]),
                dash(ctx::s(v, "note").to_string()),
            ]
        })
        .collect();
    ctx::table(&["version", "score", "errors", "warnings", "files", "author", "created", "note"], &rows);
}

fn print_version(v: &Value) {
    ctx::fields(&[
        ("version", version_text(&v["version"])),
        ("author", ctx::s(v, "author").to_string()),
        ("created", ctx::when(&v["created_ms"])),
        ("note", dash(ctx::s(v, "note").to_string())),
        ("files", items(&v["files"]).count().to_string()),
    ]);
    if v["stale"].as_bool() == Some(true) {
        println!("scored under older rules; today's score is {}", ctx::text(&v["recheck"], "score"));
    }
    println!();
    print_report(v);
}

fn print_check(r: &Value) {
    ctx::fields(&[
        ("name", dash(ctx::text(r, "name"))),
        ("description", dash(ctx::text(r, "description"))),
        ("files", ctx::text(&r["stats"], "files")),
        ("body lines", ctx::text(&r["stats"], "body_lines")),
        ("est tokens", ctx::text(&r["stats"], "est_tokens")),
    ]);
    println!();
    print_report(r);
}

fn print_rules(r: &Value) {
    println!("rules version {}", ctx::text(r, "rules_version"));
    let rows: Vec<Vec<String>> = items(&r["rules"])
        .map(|rule| vec![ctx::s(rule, "id").to_string(), ctx::s(rule, "severity").to_string(), ctx::s(rule, "title").to_string()])
        .collect();
    ctx::table(&["rule", "severity", "title"], &rows);
}

fn save(ctx: &Ctx, id: &str, dir: &Path, note: Option<&str>, base: Option<u32>) -> Result<(), String> {
    let folder = read_folder(dir)?;
    note_skipped(&folder.notes);
    let base = match base {
        Some(v) => v,
        None => latest_version(ctx, id)?,
    };
    let body = version_body(&folder.skill_md, &folder.files, note.unwrap_or(""), base);
    let path = format!("{}/versions", skill_path(id));
    let reply = match call_status(ctx, "POST", &path, Some(&body)) {
        Ok(reply) => reply,
        Err((409, _)) => return Err("a newer version exists; reload with `skills show`".into()),
        Err((_, message)) => return Err(message),
    };
    let files = folder.files.len();
    ctx.out(&reply, |r| {
        if r["created"].as_bool() == Some(true) {
            println!("saved {} of {id} ({files} files)", version_text(&r["version"]["version"]));
        } else {
            println!("no change: {} already has these files", version_text(&r["version"]["version"]));
        }
    });
    Ok(())
}

fn diff(ctx: &Ctx, id: &str, from: u32, to: u32) -> Result<(), String> {
    let d = ctx.client.get(&format!("{}/diff?from={from}&to={to}", skill_path(id)))?;
    ctx.out(&d, |d| {
        println!("v{from} → v{to}: {} added, {} removed", count_op(&d["skill_md"], "add"), count_op(&d["skill_md"], "del"));
        print_diff_lines(&d["skill_md"]);
        println!();
        let files = &d["files"];
        if [&files["added"], &files["removed"], &files["changed"]].iter().all(|l| items(l).next().is_none()) {
            println!("files: no change");
        } else {
            println!("files: added {} · removed {} · changed {}", dash(str_list(&files["added"])), dash(str_list(&files["removed"])), dash(str_list(&files["changed"])));
        }
    });
    Ok(())
}

/// The preview of an import: where it came from, then one row per skill found.
fn print_preview(p: &Value) {
    println!("from {}", source_text(&p["source"]));
    let rows: Vec<Vec<String>> = items(&p["skills"])
        .map(|s| {
            let r = &s["report"];
            vec![
                ctx::s(s, "path").to_string(),
                dash(ctx::s(s, "name").to_string()),
                ctx::s(s, "title").to_string(),
                ctx::text(r, "score"),
                ctx::text(r, "errors"),
                ctx::text(r, "warnings"),
                ctx::text(s, "files"),
                dash(ctx::text(s, "exists")),
                (if importable(s) { "yes" } else { "no" }).to_string(),
            ]
        })
        .collect();
    ctx::table(&["path", "name", "title", "score", "errors", "warnings", "files", "exists", "import"], &rows);
}

/// A skill in a preview is picked by default when it has no errors and no live skill of its name.
fn importable(s: &Value) -> bool {
    s["report"]["errors"].as_u64() == Some(0) && s["exists"].is_null()
}

/// The folders to import: the given picks (each must be in the preview), else the importable ones.
fn picks_for(preview: &Value, pick: &[String]) -> Result<Vec<String>, String> {
    let found: Vec<&Value> = items(&preview["skills"]).collect();
    if pick.is_empty() {
        let picks: Vec<String> = found.iter().filter(|s| importable(s)).map(|s| ctx::s(s, "path").to_string()).collect();
        if picks.is_empty() {
            return Err("nothing to import: every skill has an error or already exists; name the skills with --pick".into());
        }
        return Ok(picks);
    }
    let mut picks: Vec<String> = Vec::new();
    for p in pick {
        if !found.iter().any(|s| ctx::s(s, "path") == p) {
            return Err(format!("{p} is not a skill in this preview"));
        }
        if !picks.contains(p) {
            picks.push(p.clone());
        }
    }
    Ok(picks)
}

/// Previews are shown, the picks are confirmed, and then they are imported (all or none, as the hub commits them).
fn import_picked(ctx: &Ctx, preview: &Value, pick: &[String], bind: Vec<String>, every: bool, yes: bool) -> Result<(), String> {
    ctx.out(preview, print_preview);
    let picks = picks_for(preview, pick)?;
    let bindings = bindings_list(bind, every)?;
    let bound = if bindings.is_empty() { "unbound".to_string() } else { format!("bound to {}", bindings.join(", ")) };
    if !confirm(yes, &format!("Import {} ({bound}).", picks.join(", ")))? {
        return Err("cancelled".into());
    }
    let preview_id = ctx::s(preview, "preview_id");
    let made = ctx.client.post("/v1/skills/import/commit", &commit_body(preview_id, &picks, &bindings))?;
    ctx.out(&made, |m| {
        for s in items(&m["skills"]) {
            println!("imported {} as {}", ctx::s(s, "title"), ctx::s(s, "id"));
        }
    });
    Ok(())
}

/// A zip POSTed as it is (the hub reads the bytes, not JSON), with the answer read as JSON.
fn post_zip(ctx: &Ctx, path: &str, bytes: &[u8]) -> Result<Value, String> {
    let reply = apz_http::client::request(&ctx.client.target, "POST", path, Some(bytes), UPLOAD_TIMEOUT)
        .map_err(|e| format!("{} {path}: {e}", ctx.client.base()))?;
    json_reply(path, &reply)
}

fn export(ctx: &Ctx, id: Option<&str>, ids: Option<&str>, version: Option<u32>, out: &Path, force: bool) -> Result<(), String> {
    let path = match (id, ids) {
        (Some(id), _) => {
            let v = match version {
                Some(v) => v,
                None => live_or_latest(ctx, id)?,
            };
            export_path(Some(id), None, v)
        }
        (None, Some(ids)) => export_path(None, Some(ids), 0),
        (None, None) => return Err("give a skill ID, or --ids a,b".into()),
    };
    if !force && out.exists() {
        return Err(format!("{} exists: pass --force to overwrite it", out.display()));
    }
    let bytes = get_bytes(ctx, &path)?;
    write_out(out, &bytes, force)?;
    ctx.out(&json!({ "file": out.display().to_string(), "bytes": bytes.len() }), |_| {
        println!("exported {} bytes to {}", bytes.len(), out.display());
    });
    Ok(())
}

/// Writes the file; without --force it is created new, so an existing file is never replaced.
fn write_out(out: &Path, bytes: &[u8], force: bool) -> Result<(), String> {
    let opened = if force {
        std::fs::File::create(out)
    } else {
        std::fs::OpenOptions::new().write(true).create_new(true).open(out)
    };
    let mut file = opened.map_err(|e| format!("{}: {e}", out.display()))?;
    file.write_all(bytes).map_err(|e| format!("{}: {e}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    /// The `skills` command alone, so its subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    /// `skills ARGS`, parsed.
    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["skills"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    fn entry(path: &str, bytes: &[u8], executable: bool) -> Entry {
        Entry { path: path.into(), executable, bytes: bytes.to_vec() }
    }

    #[test]
    fn the_commands_are_all_there() {
        let cmd = Probe::command();
        for name in [
            "list", "show", "file", "create", "edit", "save", "bindings", "publish", "unpublish", "archive", "restore", "diff", "history",
            "check", "rules", "import", "github-preview", "github-check", "github-update", "export",
        ] {
            assert!(cmd.find_subcommand(name).is_some(), "{name} is missing");
        }
    }

    #[test]
    fn the_required_arguments_are_checked() {
        assert!(parse(&["save", "x", "dir"]).is_ok());
        assert!(parse(&["save", "x"]).is_err(), "save takes the folder");
        assert!(parse(&["create", "--title", "T", "--file", "SKILL.md", "--every", "--bind", "role:builder"]).is_ok());
        assert!(parse(&["create", "--title", "T"]).is_err(), "create takes a SKILL.md");
        assert!(parse(&["file", "x", "1", "scripts/run.sh"]).is_ok());
        assert!(parse(&["diff", "x", "1"]).is_err());
    }

    #[test]
    fn export_takes_one_skill_or_several_and_an_output() {
        assert!(parse(&["export", "x", "-o", "x.zip"]).is_ok());
        assert!(parse(&["export", "--ids", "a,b", "-o", "all.zip"]).is_ok());
        assert!(parse(&["export", "x", "--ids", "a", "-o", "f.zip"]).is_err(), "one or the other");
        assert!(parse(&["export", "x"]).is_err(), "an output file is required");
        assert!(parse(&["export", "--ids", "a", "-o", "f.zip", "--version", "2"]).is_err(), "a version is for one skill");
        assert!(parse(&["export", "x", "-o", "f.zip", "--force"]).is_ok());
    }

    #[test]
    fn bindings_are_replaced_or_cleared_not_both() {
        assert!(parse(&["bindings", "x", "--bind", "role:builder", "--every"]).is_ok());
        assert!(parse(&["bindings", "x", "--clear"]).is_ok());
        assert!(parse(&["bindings", "x", "--clear", "--bind", "plaza"]).is_err());
    }

    #[test]
    fn export_paths_keep_the_commas_between_ids() {
        assert_eq!(export_path(Some("pdf"), None, 3), "/v1/skills/pdf/versions/3/export");
        assert_eq!(export_path(None, Some("a,b"), 0), "/v1/skills/export?ids=a,b");
        assert_eq!(export_path(None, Some("a b,c"), 0), "/v1/skills/export?ids=a%20b,c");
    }

    #[test]
    fn a_file_path_keeps_its_slashes_and_encodes_each_segment() {
        assert_eq!(encode_path("scripts/run.sh"), "scripts/run.sh");
        assert_eq!(encode_path("notes/a b.md"), "notes/a%20b.md");
        assert_eq!(skill_path("pdf-forms"), "/v1/skills/pdf-forms");
    }

    #[test]
    fn the_bindings_are_checked_and_sorted() {
        assert_eq!(bindings_list(vec!["role:builder".into(), "plaza".into(), "role:builder".into()], false).unwrap(), ["plaza", "role:builder"]);
        assert_eq!(bindings_list(vec!["role:builder".into()], true).unwrap(), ["plaza", "role:builder"]);
        assert!(bindings_list(vec!["everyone".into()], false).is_err());
        assert!(bindings_list(vec!["role:".into()], false).is_err());
        assert!(bindings_list(vec!["role:Builder".into()], false).is_err(), "role ids are lower-case");
        assert!(bindings_list(Vec::new(), false).unwrap().is_empty());
    }

    #[test]
    fn an_edit_changes_only_the_title_or_the_tags_given() {
        let current = json!({ "title": "Old", "tags": ["x"] });
        assert_eq!(edit_body(&current, Some("New"), None), json!({ "title": "New", "tags": ["x"] }));
        assert_eq!(edit_body(&current, None, Some(&["a".to_string(), " ".to_string()])), json!({ "title": "Old", "tags": ["a"] }));
    }

    #[test]
    fn the_create_commit_and_bindings_bodies_have_the_hubs_fields() {
        let created = create_body("PDF forms", None, &["pdf".into()], &["plaza".into()], "# SKILL\n");
        assert_eq!(created["title"], "PDF forms");
        assert_eq!(created["files"], json!([]));
        assert!(created.get("id").is_none());
        assert_eq!(create_body("T", Some("t"), &[], &[], "x")["id"], "t");
        assert_eq!(commit_body("p1", &["a".into()], &["plaza".into()]), json!({ "preview_id": "p1", "picks": ["a"], "bindings": ["plaza"] }));
        assert_eq!(bindings_body(&[]), json!({ "bindings": [] }));
        assert_eq!(publish_body(4), json!({ "version": 4 }));
    }

    #[test]
    fn a_version_carries_each_file_in_base64() {
        let files = vec![entry("scripts/run.sh", b"echo hi", true)];
        let body = version_body("# S\n", &files, "why", 2);
        assert_eq!(body["base_version"], 2);
        assert_eq!(body["note"], "why");
        assert_eq!(body["files"][0]["path"], "scripts/run.sh");
        assert_eq!(body["files"][0]["executable"], true);
        assert_eq!(body["files"][0]["content_b64"], base64(b"echo hi"));
    }

    #[test]
    fn a_check_file_carries_its_text_only_when_it_is_text() {
        let text = check_file_json(&entry("a.md", b"hello", false));
        assert_eq!(text["text"], "hello");
        assert_eq!(text["size"], 5);
        let binary = check_file_json(&entry("a.bin", &[0xff, 0xfe, 0x00], false));
        assert!(binary["text"].is_null());
    }

    #[test]
    fn base64_matches_the_standard_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_stored_zip_ends_with_its_directory_and_counts_its_entries() {
        let zip = stored_zip(&[entry("SKILL.md", b"# S\n", false), entry("scripts/run.sh", b"echo", true)]).unwrap();
        assert_eq!(&zip[..4], &[0x50, 0x4b, 0x03, 0x04], "starts with a local header");
        let eocd = zip.len() - 22;
        assert_eq!(&zip[eocd..eocd + 4], &[0x50, 0x4b, 0x05, 0x06], "ends with the end of the central directory");
        assert_eq!(u16::from_le_bytes([zip[eocd + 10], zip[eocd + 11]]), 2, "two entries");
        assert!(zip.windows(b"scripts/run.sh".len()).any(|w| w == b"scripts/run.sh"));
        assert!(stored_zip(&[]).unwrap().len() == 22, "an empty zip is the end record alone");
    }

    #[test]
    fn a_hidden_file_and_a_large_one_are_left_out() {
        assert!(is_hidden(".env"));
        assert!(is_hidden("scripts/.cache/x"));
        assert!(!is_hidden("scripts/run.sh"));
        assert_eq!(skip_reason(".env", 3), Some(Skip::Hidden));
        assert_eq!(skip_reason("data/big.bin", MAX_FILE_BYTES + 1), Some(Skip::TooLarge));
        assert_eq!(skip_reason("data/ok.bin", MAX_FILE_BYTES), None, "a file of exactly 5 MB is taken");
        assert_eq!(skip_reason("a/b.md", 10), None);
    }

    #[test]
    fn a_folder_is_read_without_its_skill_file_or_its_hidden_parts() {
        let root = std::env::temp_dir().join(format!("apz-cli-skills-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("SKILL.md"), "# S\n").unwrap();
        std::fs::write(root.join("sub/a.txt"), "a").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        let folder = read_folder(&root).unwrap();
        assert_eq!(folder.skill_md, "# S\n");
        let paths: Vec<&str> = folder.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["sub/a.txt"]);
        assert!(folder.notes.is_empty());
        let (zip, _) = dir_zip(&root).unwrap();
        assert!(zip.windows(b"sub/a.txt".len()).any(|w| w == b"sub/a.txt"));
        assert!(!zip.windows(b".env".len()).any(|w| w == b".env"), "a hidden file is not zipped");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_without_a_skill_file_is_refused() {
        let root = std::env::temp_dir().join(format!("apz-cli-skills-nomd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notes.txt"), "x").unwrap();
        assert!(read_folder(&root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_preview_pick_is_importable_only_without_errors_or_a_live_twin() {
        let fine = json!({ "path": "a", "report": { "errors": 0 }, "exists": null });
        let broken = json!({ "path": "b", "report": { "errors": 2 }, "exists": null });
        let twin = json!({ "path": "c", "report": { "errors": 0 }, "exists": "pdf" });
        assert!(importable(&fine));
        assert!(!importable(&broken));
        assert!(!importable(&twin));
        let preview = json!({ "skills": [fine, broken, twin] });
        assert_eq!(picks_for(&preview, &[]).unwrap(), ["a"]);
        assert_eq!(picks_for(&preview, &["b".into()]).unwrap(), ["b"], "a named pick is taken as asked");
        assert!(picks_for(&preview, &["zzz".into()]).is_err());
        assert!(picks_for(&json!({ "skills": [json!({ "path": "b", "report": { "errors": 1 }, "exists": null })] }), &[]).is_err());
    }

    #[test]
    fn the_source_of_a_skill_reads_as_its_origin() {
        assert_eq!(source_text(&json!({ "kind": "built" })), "built");
        assert_eq!(source_text(&json!({ "kind": "github", "repo": "o/r", "path": "skills/pdf", "commit": "0123456789abcdef" })), "github o/r skills/pdf @0123456789ab");
        assert_eq!(source_text(&Value::Null), "—");
    }
}
