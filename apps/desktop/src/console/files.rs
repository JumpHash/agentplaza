//! Files from the owner's machine. `attach` sends one file to the hub as an attachment for a message (`PUT
//! /v1/files`), and `upload_to_project` puts dropped files or folders into a project's repository (`PUT
//! /v1/projects/{id}/uploads` for each, then `POST /v1/projects/{id}/files`). No UI here: the composers and the
//! project screens call these from a task. What never belongs in a repository (.git, node_modules, .DS_Store,
//! Thumbs.db, __MACOSX) is left out and counted, as the web's file drop does.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::console::client::{Client, enc};

/// The largest file an attachment takes, as the hub's route does.
const MAX_ATTACHMENT: u64 = 100 << 20;
/// The largest file a project upload takes.
const MAX_UPLOAD: u64 = 200 << 20;
/// The most files one project change takes.
const MAX_FILES: usize = 5000;
/// Parts of a path that never go into a repository.
const IGNORED_PARTS: [&str; 5] = [".git", "node_modules", ".DS_Store", "Thumbs.db", "__MACOSX"];

/// A file the hub holds as an attachment, ready to go with a message.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub kind: String,
}

/// The file record the hub answers an attachment's upload with.
#[derive(Deserialize)]
struct Stored {
    id: String,
    title: Option<String>,
    size: u64,
    content_type: Option<String>,
}

/// The answer to one raw upload into a project's repository.
#[derive(Deserialize)]
struct Uploaded {
    hash: String,
}

/// The type a file is sent as, by its extension. Anything unknown is an opaque byte stream.
pub fn content_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" => "text/html",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// A byte count as people read it: 812 B, 14 KB, 3.2 MB.
pub fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    if bytes < 1024 * 1024 {
        return format!("{} KB", (bytes as f64 / 1024.0).round() as u64);
    }
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb < 10.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{} MB", mb.round() as u64)
    }
}

/// Sends `path` to the hub as an attachment: its bytes, under its name and type. The hub keeps it private until a
/// message carries it.
pub fn attach(client: &Client, path: &Path) -> Result<Attachment, String> {
    let name = file_name(path)?;
    let bytes = read_within(path, &name, MAX_ATTACHMENT)?;
    if bytes.is_empty() {
        return Err(format!("{name} is empty."));
    }
    let kind = content_type(&name);
    // The hub stores the file under the type its request names; without one it would take the bytes for JSON.
    let mut c = client.clone();
    c.target
        .headers
        .push(("Content-Type".to_string(), kind.to_string()));
    let stored: Stored = c.put_bytes(&format!("/v1/files?name={}", enc(&name)), &bytes)?;
    Ok(Attachment {
        id: stored.id,
        name: stored.title.unwrap_or(name),
        size: stored.size,
        kind: stored.content_type.unwrap_or_else(|| kind.to_string()),
    })
}

/// Whether a repository-relative path has a part that never goes into a repository.
pub fn ignored(rel: &str) -> bool {
    rel.split('/').any(|part| IGNORED_PARTS.contains(&part))
}

/// The files to send for dropped `paths`, each as its repository path (a loose file as its name, a folder's files
/// under the folder's own name: `shop/src/main.rs`), and how many were left out. A left-out folder counts once.
pub fn collect(paths: &[PathBuf]) -> (Vec<(String, PathBuf)>, usize) {
    let mut kept = Vec::new();
    let mut skipped = 0;
    for path in paths {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            skipped += 1;
            continue;
        };
        if ignored(&name) {
            skipped += 1;
        } else if path.is_dir() {
            walk(path, &name, &mut kept, &mut skipped);
        } else {
            kept.push((name, path.clone()));
        }
    }
    (kept, skipped)
}

/// Sends dropped `paths` into project `project`'s repository as one change, and answers the hub's answer with a
/// `skipped` count added. `title` names the change; the hub gives it a default when it is None.
pub fn upload_to_project(
    client: &Client,
    project: &str,
    paths: &[PathBuf],
    title: Option<&str>,
) -> Result<Value, String> {
    let (files, skipped) = collect(paths);
    if files.is_empty() {
        return Err(if skipped > 0 {
            "Nothing to upload: what was dropped never goes into a repository.".to_string()
        } else {
            "Nothing to upload.".to_string()
        });
    }
    if files.len() > MAX_FILES {
        return Err(format!(
            "A change takes at most {MAX_FILES} files; drop fewer."
        ));
    }
    let mut c = client.clone();
    c.target.headers.push((
        "Content-Type".to_string(),
        "application/octet-stream".to_string(),
    ));
    let project = enc(project);
    let mut list = Vec::with_capacity(files.len());
    for (rel, path) in &files {
        let bytes = read_within(path, rel, MAX_UPLOAD)?;
        let up: Uploaded = c.put_bytes(&format!("/v1/projects/{project}/uploads"), &bytes)?;
        list.push(json!({ "path": rel, "hash": up.hash }));
    }
    let mut body = json!({ "files": list });
    if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
        body["title"] = json!(title);
    }
    let mut answer: Value = client.post(&format!("/v1/projects/{project}/files"), &body)?;
    if let Some(map) = answer.as_object_mut() {
        map.insert("skipped".to_string(), json!(skipped));
    }
    Ok(answer)
}

/// The file's name, as the hub and the composer show it.
fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} has no file name", path.display()))
}

/// A file's bytes, when it is a file of at most `max` bytes; `label` names it in the message.
fn read_within(path: &Path, label: &str, max: u64) -> Result<Vec<u8>, String> {
    let meta = fs::metadata(path).map_err(|e| format!("{label} could not be read: {e}"))?;
    if meta.is_dir() {
        return Err(format!("{label} is a folder, not a file."));
    }
    if meta.len() > max {
        return Err(format!("{label} is over {} MB.", max >> 20));
    }
    fs::read(path).map_err(|e| format!("{label} could not be read: {e}"))
}

/// Adds the files under `dir` to `kept` as `rel/...`, leaving out (and counting) what never goes into a repository.
/// Symbolic links are left out too, so a link cycle cannot loop.
fn walk(dir: &Path, rel: &str, kept: &mut Vec<(String, PathBuf)>, skipped: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        *skipped += 1;
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let child = format!("{rel}/{}", entry.file_name().to_string_lossy());
        let Ok(kind) = entry.file_type() else {
            *skipped += 1;
            continue;
        };
        if ignored(&child) || kind.is_symlink() {
            *skipped += 1;
        } else if kind.is_dir() {
            walk(&entry.path(), &child, kept, skipped);
        } else if kind.is_file() {
            kept.push((child, entry.path()));
        } else {
            *skipped += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique folder under the system's temp dir, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir()
                .join(format!("apz-files-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).expect("a scratch folder");
            Scratch(dir)
        }

        fn write(&self, rel: &str, body: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
            fs::write(&path, body).expect("a file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn content_type_by_extension() {
        assert_eq!(content_type("photo.PNG"), "image/png");
        assert_eq!(content_type("a.jpeg"), "image/jpeg");
        assert_eq!(content_type("a.jpg"), "image/jpeg");
        assert_eq!(content_type("notes.md"), "text/markdown");
        assert_eq!(content_type("data.csv"), "text/csv");
        assert_eq!(content_type("page.html"), "text/html");
        assert_eq!(content_type("bundle.zip"), "application/zip");
        assert_eq!(content_type("tool.tar"), "application/octet-stream");
        assert_eq!(content_type("README"), "application/octet-stream");
    }

    #[test]
    fn size_text_reads_like_people_do() {
        assert_eq!(size_text(812), "812 B");
        assert_eq!(size_text(14 * 1024), "14 KB");
        assert_eq!(size_text(3 * 1024 * 1024 + 300 * 1024), "3.3 MB");
        assert_eq!(size_text(40 * 1024 * 1024), "40 MB");
    }

    #[test]
    fn ignored_parts_never_go_in() {
        assert!(ignored(".git/config"));
        assert!(ignored("app/node_modules/left-pad/index.js"));
        assert!(ignored("docs/.DS_Store"));
        assert!(ignored("Thumbs.db"));
        assert!(ignored("__MACOSX/a"));
        assert!(!ignored("src/main.rs"));
        assert!(!ignored("gitignore.txt"));
        assert!(!ignored("node_modules_notes/a.md"));
    }

    #[test]
    fn collect_keeps_folders_under_their_name_and_counts_the_rest() {
        let scratch = Scratch::new("collect");
        scratch.write("shop/src/main.rs", "fn main() {}");
        scratch.write("shop/.git/HEAD", "ref");
        scratch.write("shop/node_modules/x/index.js", "1");
        scratch.write("shop/.DS_Store", "x");
        let loose = scratch.write("loose.txt", "hello");
        let junk = scratch.write(".DS_Store", "x");

        let (kept, skipped) = collect(&[scratch.0.join("shop"), loose, junk]);
        let mut names: Vec<String> = kept.into_iter().map(|(rel, _)| rel).collect();
        names.sort();
        assert_eq!(names, vec!["loose.txt", "shop/src/main.rs"]);
        // The folder's .DS_Store, .git and node_modules, and the loose .DS_Store.
        assert_eq!(skipped, 4);
    }

    #[test]
    fn collect_skips_a_folder_named_like_junk() {
        let scratch = Scratch::new("junk-folder");
        scratch.write("node_modules/a/b.js", "1");
        let (kept, skipped) = collect(&[scratch.0.join("node_modules")]);
        assert!(kept.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn a_folder_is_not_an_attachment() {
        let scratch = Scratch::new("folder");
        scratch.write("shop/a.txt", "1");
        let err = read_within(&scratch.0.join("shop"), "shop", MAX_ATTACHMENT).unwrap_err();
        assert!(err.contains("is a folder"), "{err}");
    }

    #[test]
    fn nothing_to_upload_fails_before_any_request() {
        let client = Client::new("http://127.0.0.1:9", None, None);
        let scratch = Scratch::new("nothing");
        let junk = scratch.write(".DS_Store", "x");
        let err = upload_to_project(&client, "p1", &[junk], None).unwrap_err();
        assert!(err.contains("never goes into a repository"), "{err}");
        assert!(upload_to_project(&client, "p1", &[], Some("Title")).is_err());
    }
}
