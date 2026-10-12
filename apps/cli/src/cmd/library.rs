//! `agentplaza library`: The library: shared files and images.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the files: deliverables and shared files.
    List {
        /// Only `deliverable` or `shared`.
        #[arg(long)]
        kind: Option<String>,
    },
    /// Show one file's details.
    Show {
        /// The file's id.
        id: String,
    },
    /// Download a file's bytes. Never overwrites: pick another path if it exists.
    Download {
        /// The file's id.
        id: String,
        /// Where to write it (default: its name in the current directory).
        #[arg(long, short)]
        to: Option<PathBuf>,
    },
    /// Delete a file from the library. Asks first unless `--yes`.
    Delete {
        /// The file's id.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { kind } => list(ctx, kind),
        Cmd::Show { id } => show(ctx, &id),
        Cmd::Download { id, to } => download(ctx, &id, to),
        Cmd::Delete { id, yes } => delete(ctx, &id, yes),
    }
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn list(ctx: &Ctx, kind: Option<String>) -> Result<(), String> {
    let path = match kind.as_deref() {
        None => "/v1/files".to_string(),
        Some(k @ ("deliverable" | "shared")) => format!("/v1/files?kind={k}"),
        Some(_) => return Err("--kind is deliverable or shared".into()),
    };
    let value = ctx.client.get(&path)?;
    ctx.out(&value, |v| {
        let rows: Vec<Vec<String>> = array(v)
            .iter()
            .map(|f| ["id", "kind", "name", "size", "author", "audience"].iter().map(|k| ctx::text(f, k)).collect())
            .collect();
        ctx::table(&["id", "kind", "name", "size", "author", "audience"], &rows);
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str) -> Result<(), String> {
    let value = ctx.client.get(&format!("/v1/files/{}", ctx::seg(id)))?;
    ctx.out(&value, |v| {
        ctx::fields(&[
            ("id", ctx::text(v, "id")),
            ("kind", ctx::text(v, "kind")),
            ("name", ctx::text(v, "name")),
            ("title", ctx::text(v, "title")),
            ("description", ctx::text(v, "description")),
            ("type", ctx::text(v, "content_type")),
            ("size", ctx::text(v, "size")),
            ("author", ctx::text(v, "author")),
            ("task", ctx::text(v, "task")),
            ("audience", ctx::text(v, "audience")),
            ("allowed", ctx::text(v, "allowed")),
            ("store", ctx::text(v, "store")),
            ("sha256", ctx::text(v, "sha256")),
            ("created", ctx::text(v, "created_ms")),
        ]);
    });
    Ok(())
}

/// The file's own name without any directory part (an agent chose it).
fn safe_name(name: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("download")
        .to_string()
}

fn download(ctx: &Ctx, id: &str, to: Option<PathBuf>) -> Result<(), String> {
    let path = match to {
        Some(path) => path,
        None => {
            let meta = ctx.client.get(&format!("/v1/files/{}", ctx::seg(id)))?;
            PathBuf::from(safe_name(ctx::s(&meta, "name")))
        }
    };
    let reply = apz_http::client::request(&ctx.client.target, "GET", &format!("/v1/files/{}/content", ctx::seg(id)), None, ctx.client.timeout)
        .map_err(|e| format!("{}: {e}", ctx.client.base()))?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("download {id}: HTTP {}", reply.status));
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("{}: {e} (downloads never overwrite; pick --to)", path.display()))?;
    file.write_all(&reply.body).map_err(|e| format!("{}: {e}", path.display()))?;
    let saved = json!({ "id": id, "path": path.display().to_string(), "bytes": reply.body.len() });
    ctx.out(&saved, |v| println!("saved {} ({} bytes)", ctx::s(v, "path"), reply.body.len()));
    Ok(())
}

/// Asks on a terminal; without one, the caller must pass `--yes`.
fn confirm(question: &str) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err(format!("{question} Pass --yes to confirm without a terminal."));
    }
    print!("{question} Proceed? [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn delete(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !yes && !confirm(&format!("Delete file {id} from the library."))? {
        println!("kept file {id}");
        return Ok(());
    }
    let value = ctx.client.delete(&format!("/v1/files/{}", ctx::seg(id)))?;
    ctx.out(&value, |_| println!("deleted file {id}"));
    Ok(())
}
