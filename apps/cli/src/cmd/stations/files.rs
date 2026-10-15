//! `agentplaza stations sync` and `agentplaza stations files`: the work folder each workstation keeps in sync with the
//! hub, both ways (docs/design/workstations.md, sections 7 and 8). The sync's state (files, last sync, conflicts, and
//! whether it is paused), its resume, the mirror's files (list, get, put, rm) and a file's delivery into the Library.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use serde_json::{Value, json};

use super::{dash, items, yes_no};
use crate::cmd::prompts::{call_status, json_reply};
use crate::ctx::{self, Ctx, confirm, seg};

/// A file moved to or from a workstation can be large: its transfer may take this long.
const TRANSFER: Duration = Duration::from_secs(600);
const FILE_HEAD: [&str; 5] = ["path", "size", "modified", "version", "conflict of"];
const CONFLICT_HEAD: [&str; 4] = ["copy", "of", "size", "modified"];

#[derive(Subcommand, Debug)]
pub enum SyncCmd {
    /// Resume a paused sync (`POST .../sync/resume`). `--apply-deletions` also applies the deletions it held back.
    Resume {
        /// Apply the deletions the paused sync held back.
        #[arg(long)]
        apply_deletions: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum FilesCmd {
    /// Download a file of the work folder: the last version the hub holds.
    Get {
        /// The file's path inside the work folder.
        path: String,
        /// Where to save it (default: its name, in this folder). Never overwrites a file.
        #[arg(short, long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Upload a local file to the work folder; the next pass pushes it to the station.
    Put {
        /// The file's path inside the work folder.
        path: String,
        /// The local file to upload.
        file: PathBuf,
    },
    /// Remove a file from the work folder; the next pass removes it on the station. Asks first unless --yes.
    Rm {
        /// The file's path inside the work folder.
        path: String,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Put a file into the plaza Library as a deliverable.
    Deliver {
        /// The file's path inside the work folder.
        path: String,
        /// The Library title (default: the file's name).
        #[arg(long)]
        title: Option<String>,
        /// A description for the Library.
        #[arg(long)]
        description: Option<String>,
        /// The task the file is a result of.
        #[arg(long)]
        task: Option<String>,
    },
}

/// `stations sync <id>`: the state. `sync <id> resume` resumes a paused sync.
pub fn sync(ctx: &Ctx, id: &str, action: Option<SyncCmd>) -> Result<(), String> {
    match action {
        None => {
            let state = ctx.client.get(&format!("/v1/stations/{}/sync", seg(id)))?;
            ctx.out(&state, |s| {
                if let Some(banner) = paused_banner(s) {
                    println!("{banner}\n");
                }
                ctx::fields(&sync_fields(s));
                let conflicts = items(&s["conflicts"]);
                if !conflicts.is_empty() {
                    println!("\nconflicts (both copies are kept)");
                    let rows: Vec<Vec<String>> = conflicts.iter().map(conflict_row).collect();
                    ctx::table(&CONFLICT_HEAD, &rows);
                }
            });
            Ok(())
        }
        Some(SyncCmd::Resume { apply_deletions }) => resume(ctx, id, apply_deletions),
    }
}

/// `stations sync <id> resume`. A hub without the route answers 404; that is said in words, not as a crash.
fn resume(ctx: &Ctx, id: &str, apply_deletions: bool) -> Result<(), String> {
    let path = format!("/v1/stations/{}/sync/resume", seg(id));
    let body = json!({ "apply_deletions": apply_deletions });
    match call_status(ctx, "POST", &path, Some(&body)) {
        Ok(answer) => {
            ctx.out(&answer, |_| {
                let deletions = if apply_deletions {
                    " (deletions applied)"
                } else {
                    ""
                };
                println!("sync resumed on {id}{deletions}");
            });
            Ok(())
        }
        Err((404, message)) => Err(format!(
            "{message}: the hub may not have the sync resume route yet ({path} answered 404)"
        )),
        Err((_, message)) => Err(message),
    }
}

/// `stations files <id> [PREFIX]` lists the mirror; the subcommands get, put, rm and deliver one file.
pub fn files(
    ctx: &Ctx,
    id: &str,
    prefix: Option<&str>,
    action: Option<FilesCmd>,
) -> Result<(), String> {
    match action {
        None => list(ctx, id, prefix),
        Some(action) => {
            if prefix.is_some() {
                return Err("a PREFIX lists the files; give it without a file subcommand".into());
            }
            run(ctx, id, action)
        }
    }
}

fn list(ctx: &Ctx, id: &str, prefix: Option<&str>) -> Result<(), String> {
    let mut path = format!("/v1/stations/{}/files", seg(id));
    if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
        path.push_str(&format!("?prefix={}", query_value(prefix)));
    }
    let files = ctx.client.get(&path)?;
    ctx.out(&files, |v| {
        let rows: Vec<Vec<String>> = items(v).iter().map(file_row).collect();
        ctx::table(&FILE_HEAD, &rows);
    });
    Ok(())
}

fn run(ctx: &Ctx, id: &str, action: FilesCmd) -> Result<(), String> {
    match action {
        FilesCmd::Get { path, out } => get(ctx, id, &path, out),
        FilesCmd::Put { path, file } => put(ctx, id, &path, &file),
        FilesCmd::Rm { path, yes } => rm(ctx, id, &path, yes),
        FilesCmd::Deliver {
            path,
            title,
            description,
            task,
        } => deliver(ctx, id, &path, title, description, task),
    }
}

fn get(ctx: &Ctx, id: &str, path: &str, out: Option<PathBuf>) -> Result<(), String> {
    let url = file_url(id, path)?;
    let target = out.unwrap_or_else(|| PathBuf::from(file_name(path)));
    let bytes = transfer(ctx, "GET", &url, None)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|e| {
            format!(
                "{}: {e} (downloads never overwrite; pick -o)",
                target.display()
            )
        })?;
    file.write_all(&bytes)
        .map_err(|e| format!("{}: {e}", target.display()))?;
    let saved = json!({ "station": id, "path": path, "saved": target.display().to_string(), "bytes": bytes.len() });
    ctx.out(&saved, |v| {
        println!(
            "saved {} ({})",
            ctx::s(v, "saved"),
            crate::cmd::lab::size_text(bytes.len() as f64)
        )
    });
    Ok(())
}

fn put(ctx: &Ctx, id: &str, path: &str, file: &Path) -> Result<(), String> {
    let url = file_url(id, path)?;
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let reply = apz_http::client::request(&ctx.client.target, "PUT", &url, Some(&bytes), TRANSFER)
        .map_err(|e| format!("{} {url}: {e}", ctx.client.base()))?;
    let answer = json_reply(&url, &reply)?;
    ctx.out(&answer, |_| {
        println!(
            "uploaded {path} to {id} ({}); the next pass pushes it to the station",
            crate::cmd::lab::size_text(bytes.len() as f64)
        )
    });
    Ok(())
}

fn rm(ctx: &Ctx, id: &str, path: &str, yes: bool) -> Result<(), String> {
    let url = file_url(id, path)?;
    if !confirm(
        yes,
        &format!("Remove {path} from {id}? The next pass removes it on the station."),
    )? {
        println!("kept {path}");
        return Ok(());
    }
    let gone = ctx.client.delete(&url)?;
    ctx.out(&gone, |_| println!("removed {path} from {id}"));
    Ok(())
}

fn deliver(
    ctx: &Ctx,
    id: &str,
    path: &str,
    title: Option<String>,
    description: Option<String>,
    task: Option<String>,
) -> Result<(), String> {
    let url = format!("{}/deliver", file_url(id, path)?);
    let mut body = json!({});
    if let Some(title) = title {
        body["title"] = json!(title);
    }
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if let Some(task) = task {
        body["task"] = json!(task);
    }
    let made = ctx.client.post(&url, &body)?;
    ctx.out(&made, |v| {
        println!(
            "delivered {path} to the Library as {} ({}, {})",
            ctx::s(v, "id"),
            ctx::s(v, "name"),
            crate::cmd::lab::size_text(v["size"].as_u64().unwrap_or(0) as f64)
        )
    });
    Ok(())
}

// ---- Shared with the state and the TUI --------------------------------------------------------

/// The sync's state as `stations sync` prints it. `paused` and `pending_deletions` appear only when the hub says so.
pub fn sync_fields(s: &Value) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("linked", yes_no(&s["linked"])),
        ("files", ctx::text(s, "files")),
        (
            "bytes",
            crate::cmd::lab::size_text(s["bytes"].as_u64().unwrap_or(0) as f64),
        ),
        ("last sync", last_sync(s)),
        ("conflicts", items(&s["conflicts"]).len().to_string()),
    ];
    if let Some(paused) = s.get("paused").and_then(Value::as_bool) {
        fields.push(("paused", yes_no(&json!(paused))));
        if paused {
            fields.push(("reason", dash(ctx::text(s, "reason"))));
        }
    }
    if let Some(pending) = s.get("pending_deletions").filter(|v| !v.is_null()) {
        fields.push(("pending deletions", count_of(pending)));
    }
    fields
}

/// The banner the sync's pause shows, when the hub says the sync is paused.
pub fn paused_banner(s: &Value) -> Option<String> {
    if s.get("paused").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let reason = ctx::text(s, "reason");
    Some(if reason.is_empty() {
        "sync paused (run `agentplaza stations sync ID resume` to resume it)".to_string()
    } else {
        format!("sync paused: {reason} (run `agentplaza stations sync ID resume` to resume it)")
    })
}

/// One conflict copy's cells: the copy's path, the file it is a copy of, its size and its time.
pub fn conflict_row(f: &Value) -> Vec<String> {
    vec![
        ctx::text(f, "path"),
        dash(ctx::text(f, "conflict_of")),
        crate::cmd::lab::size_text(f["size"].as_u64().unwrap_or(0) as f64),
        ctx::when(&f["mtime_ms"]),
    ]
}

/// A file of the mirror: its path, size, time, version and the file it is a copy of.
pub fn file_row(f: &Value) -> Vec<String> {
    vec![
        ctx::text(f, "path"),
        crate::cmd::lab::size_text(f["size"].as_u64().unwrap_or(0) as f64),
        ctx::when(&f["mtime_ms"]),
        ctx::text(f, "version"),
        dash(ctx::text(f, "conflict_of")),
    ]
}

/// The URL of one file of the mirror. Each segment is escaped on its own, so the slashes stay separators.
pub fn file_url(id: &str, path: &str) -> Result<String, String> {
    Ok(format!(
        "/v1/stations/{}/files/{}",
        seg(id),
        file_path(path)?
    ))
}

/// A path inside the work folder, escaped segment by segment. Leading and trailing slashes are dropped; an empty path
/// is refused.
pub fn file_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err("the path is empty: name a file inside the work folder".into());
    }
    Ok(trimmed.split('/').map(seg).collect::<Vec<_>>().join("/"))
}

/// The file's own name: the last segment of its path.
pub fn file_name(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A query value for the hub, which reads query strings as they are (no percent-decoding): slashes stay as they are.
fn query_value(text: &str) -> String {
    seg(text).replace("%2F", "/")
}

/// A transfer's bytes, or the refusal's text.
fn transfer(ctx: &Ctx, method: &str, url: &str, body: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let reply = apz_http::client::request(&ctx.client.target, method, url, body, TRANSFER)
        .map_err(|e| format!("{} {url}: {e}", ctx.client.base()))?;
    if (200..300).contains(&reply.status) {
        return Ok(reply.body);
    }
    Err(json_reply(url, &reply)
        .err()
        .unwrap_or_else(|| format!("{url}: HTTP {}", reply.status)))
}

fn last_sync(s: &Value) -> String {
    match s.get("last_sync_ms").and_then(Value::as_u64) {
        None | Some(0) => "never".to_string(),
        Some(_) => ctx::ago(&s["last_sync_ms"]),
    }
}

/// A count that the hub may send as a number or as a list.
fn count_of(v: &Value) -> String {
    match v {
        Value::Array(list) => list.len().to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_paths_keep_their_slashes_and_escape_each_segment() {
        assert_eq!(file_path("/docs/a b.txt/").unwrap(), "docs/a%20b.txt");
        assert_eq!(file_path("x/y").unwrap(), "x/y");
        assert!(file_path("/").is_err());
        assert_eq!(file_name("docs/a.txt"), "a.txt");
        assert_eq!(
            file_url("w1", "out/r.md").unwrap(),
            "/v1/stations/w1/files/out/r.md"
        );
        assert_eq!(query_value("docs/sub"), "docs/sub");
    }
}
