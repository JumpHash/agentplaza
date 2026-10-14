//! `agentplaza library`: The library: shared files and images.

use std::io::Write;
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};

/// The buckets the library filters by (the Library screen's groups).
const BUCKETS: [&str; 5] = ["video", "image", "audio", "documents", "other"];
/// The columns of `library list`.
const COLUMNS: [&str; 8] = [
    "id", "kind", "name", "title", "size", "author", "task", "audience",
];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the files: deliverables and shared files.
    List {
        /// Only `deliverable` or `shared`.
        #[arg(long)]
        kind: Option<String>,
        /// Only files whose title or name holds this text (case-insensitive).
        #[arg(long)]
        search: Option<String>,
        /// Only one type: video, image, audio, documents or other.
        #[arg(long = "type", value_parser = clap::builder::PossibleValuesParser::new(BUCKETS))]
        file_type: Option<String>,
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
        Cmd::List {
            kind,
            search,
            file_type,
        } => list(ctx, kind, search, file_type),
        Cmd::Show { id } => show(ctx, &id),
        Cmd::Download { id, to } => download(ctx, &id, to),
        Cmd::Delete { id, yes } => delete(ctx, &id, yes),
    }
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The bucket a file's content type falls in: the groups the web's library filters by.
fn bucket_of(content_type: &str) -> &'static str {
    let t = content_type.to_lowercase();
    if t.starts_with("video/") {
        "video"
    } else if t.starts_with("image/") {
        "image"
    } else if t.starts_with("audio/") {
        "audio"
    } else if t == "application/pdf"
        || t.starts_with("text/")
        || t.contains("json")
        || t.contains("xml")
        || t.contains("word")
        || t.contains("officedocument")
        || t.contains("opendocument")
        || t.contains("rtf")
    {
        "documents"
    } else {
        "other"
    }
}

/// Whether a file's title or name holds `search` (case-insensitive); a blank search holds all.
fn matches_search(file: &Value, search: &str) -> bool {
    let needle = search.trim().to_lowercase();
    needle.is_empty()
        || ctx::s(file, "title").to_lowercase().contains(&needle)
        || ctx::s(file, "name").to_lowercase().contains(&needle)
}

/// Whether a file is in the bucket asked for (no bucket asked for holds all).
fn matches_bucket(file: &Value, bucket: Option<&str>) -> bool {
    bucket.is_none_or(|b| bucket_of(ctx::s(file, "content_type")) == b)
}

fn list(
    ctx: &Ctx,
    kind: Option<String>,
    search: Option<String>,
    file_type: Option<String>,
) -> Result<(), String> {
    let path = match kind.as_deref() {
        None => "/v1/files".to_string(),
        Some(k @ ("deliverable" | "shared")) => format!("/v1/files?kind={k}"),
        Some(_) => return Err("--kind is deliverable or shared".into()),
    };
    let value = ctx.client.get(&path)?;
    let search = search.unwrap_or_default();
    let files: Vec<Value> = array(&value)
        .iter()
        .filter(|f| matches_search(f, &search) && matches_bucket(f, file_type.as_deref()))
        .cloned()
        .collect();
    ctx.out(&json!(files), |_| {
        let rows: Vec<Vec<String>> = files
            .iter()
            .map(|f| COLUMNS.iter().map(|k| ctx::text(f, k)).collect())
            .collect();
        ctx::table(&COLUMNS, &rows);
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
            ("created", ctx::when(&v["created_ms"])),
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
    let reply = apz_http::client::request(
        &ctx.client.target,
        "GET",
        &format!("/v1/files/{}/content", ctx::seg(id)),
        None,
        ctx.client.timeout,
    )
    .map_err(|e| format!("{}: {e}", ctx.client.base()))?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("download {id}: HTTP {}", reply.status));
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| {
            format!(
                "{}: {e} (downloads never overwrite; pick --to)",
                path.display()
            )
        })?;
    file.write_all(&reply.body)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let saved = json!({ "id": id, "path": path.display().to_string(), "bytes": reply.body.len() });
    ctx.out(&saved, |v| {
        println!("saved {} ({} bytes)", ctx::s(v, "path"), reply.body.len())
    });
    Ok(())
}

fn delete(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(yes, &format!("Delete file {id} from the library."))? {
        println!("kept file {id}");
        return Ok(());
    }
    let value = ctx.client.delete(&format!("/v1/files/{}", ctx::seg(id)))?;
    ctx.out(&value, |_| println!("deleted file {id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_fall_into_the_web_buckets() {
        assert_eq!(bucket_of("video/mp4"), "video");
        assert_eq!(bucket_of("IMAGE/PNG"), "image");
        assert_eq!(bucket_of("audio/mpeg"), "audio");
        assert_eq!(bucket_of("application/pdf"), "documents");
        assert_eq!(bucket_of("text/markdown"), "documents");
        assert_eq!(bucket_of("application/json"), "documents");
        assert_eq!(
            bucket_of("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            "documents"
        );
        assert_eq!(
            bucket_of("application/vnd.oasis.opendocument.text"),
            "documents"
        );
        assert_eq!(bucket_of("application/zip"), "other");
        assert_eq!(bucket_of(""), "other");
    }

    #[test]
    fn search_matches_title_or_name_without_case() {
        let file = json!({ "name": "Report.PDF", "title": "Q3 results", "content_type": "application/pdf" });
        assert!(matches_search(&file, "q3"));
        assert!(matches_search(&file, "  report "));
        assert!(matches_search(&file, ""));
        assert!(!matches_search(&file, "budget"));
        let untitled = json!({ "name": "notes.txt", "title": null });
        assert!(matches_search(&untitled, "NOTES"));
    }

    #[test]
    fn type_filter_holds_only_its_bucket() {
        let image = json!({ "content_type": "image/png" });
        assert!(matches_bucket(&image, None));
        assert!(matches_bucket(&image, Some("image")));
        assert!(!matches_bucket(&image, Some("documents")));
    }
}
