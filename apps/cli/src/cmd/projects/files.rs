//! `projects upload`: files and folders into a project's repository, as one change of the owner's
//! (`crates/apid/src/project_files.rs`). Each file's bytes go up first (`PUT .../uploads`, one per file, raw), then one
//! `POST .../files` names them by path and hash and lands them on main. Nothing lands unless every file went up.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::cmd::prompts::json_reply;
use crate::ctx::{self, Ctx};

/// A file's bytes can be large: its upload may take this long.
const UPLOAD: Duration = Duration::from_secs(600);
/// The most files one change takes (the hub's limit).
const MAX_FILES: usize = 5000;
/// Names that are never sent: git's own folder, dependencies, and the Finder's file.
const SKIPPED: [&str; 3] = [".git", "node_modules", ".DS_Store"];
/// The longest path a repository takes (the hub's limit).
const PATH_MAX: usize = 400;

/// A file to send: where it is on this machine, and its path in the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub local: PathBuf,
    pub path: String,
}

/// The files to send, in order, and how many names were left out.
#[derive(Debug, Default)]
pub struct Collected {
    pub files: Vec<Found>,
    pub left_out: usize,
}

/// The files under `paths`. A file goes in under its name. A folder goes in under its own name, so its files' paths
/// start with that name, and its folders are walked the same way. Names in `SKIPPED` are left out (and counted), and
/// so are symbolic links to folders, which could loop.
pub fn collect(paths: &[PathBuf]) -> Result<Collected, String> {
    let mut out = Collected::default();
    for path in paths {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.is_dir() {
            // Canonical, so that `.` and `..` have a name.
            let root =
                std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name = name_of(&root)?;
            if SKIPPED.contains(&name.as_str()) {
                out.left_out += 1;
            } else {
                walk(&root, &name, &mut out)?;
            }
        } else if meta.is_file() {
            let name = name_of(path)?;
            if SKIPPED.contains(&name.as_str()) {
                out.left_out += 1;
            } else {
                out.files.push(Found {
                    local: path.clone(),
                    path: name,
                });
            }
        } else {
            return Err(format!("{}: not a file or a folder", path.display()));
        }
    }
    Ok(out)
}

/// The folder's entries under `prefix`, in name order.
fn walk(dir: &Path, prefix: &str, out: &mut Collected) -> Result<(), String> {
    let mut entries = std::fs::read_dir(dir)
        .and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let local = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("{}: the name is not text", local.display()))?;
        if SKIPPED.contains(&name.as_str()) {
            out.left_out += 1;
            continue;
        }
        let path = format!("{prefix}/{name}");
        check_name(&path)?;
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", local.display()))?;
        if kind.is_dir() {
            walk(&local, &path, out)?;
        } else if kind.is_file()
            || (kind.is_symlink() && std::fs::metadata(&local).is_ok_and(|m| m.is_file()))
        {
            out.files.push(Found { local, path });
        } else {
            out.left_out += 1;
        }
    }
    Ok(())
}

/// The name a path goes in under: its last part, which must be text.
fn name_of(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: the name is not text", path.display()))?;
    check_name(name)?;
    Ok(name.to_string())
}

/// A path the repository takes: no backslash, no control character, and at most 400 characters.
fn check_name(path: &str) -> Result<(), String> {
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err(format!(
            "{path:?}: a repository path has no backslash or control character"
        ));
    }
    if path.chars().count() > PATH_MAX {
        return Err(format!(
            "{path:?}: a repository path is at most {PATH_MAX} characters"
        ));
    }
    Ok(())
}

/// `projects upload ID PATH...`: sends the files, then lands them as one change.
pub fn run(
    ctx: &Ctx,
    id: &str,
    paths: &[PathBuf],
    folder: Option<&str>,
    title: Option<&str>,
) -> Result<(), String> {
    let found = collect(paths)?;
    if found.files.is_empty() {
        return Err(match found.left_out {
            0 => "no files to add".to_string(),
            left => format!("no files to add: {left} left out (.git, node_modules and .DS_Store)"),
        });
    }
    if found.files.len() > MAX_FILES {
        return Err(format!(
            "a change takes at most {MAX_FILES} files, and these are {}",
            found.files.len()
        ));
    }
    let base = format!("/v1/projects/{}", ctx::seg(id));
    let uploads = format!("{base}/uploads");
    let mut entries = Vec::with_capacity(found.files.len());
    for file in &found.files {
        let bytes =
            std::fs::read(&file.local).map_err(|e| format!("{}: {e}", file.local.display()))?;
        let reply =
            apz_http::client::request(&ctx.client.target, "PUT", &uploads, Some(&bytes), UPLOAD)
                .map_err(|e| format!("{} {}: {e}", ctx.client.base(), file.path))?;
        let uploaded = json_reply(&file.path, &reply)?;
        entries.push(json!({ "path": file.path, "hash": uploaded["hash"] }));
    }
    let mut body = json!({ "files": entries });
    if let Some(folder) = folder.map(str::trim).filter(|f| !f.is_empty()) {
        body["folder"] = json!(folder);
    }
    if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
        body["title"] = json!(title);
    }
    let landed = ctx.client.post(&format!("{base}/files"), &body)?;
    let count = landed["files"].as_u64().unwrap_or(found.files.len() as u64);
    ctx.out(&landed, |v| {
        println!(
            "Added {count} files to {} (change {})",
            ctx::text(v, "repo"),
            ctx::text(&v["landed"], "change")
        );
        if found.left_out > 0 {
            println!(
                "left out {} (.git, node_modules and .DS_Store)",
                found.left_out
            );
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_goes_in_under_its_own_name_and_the_names_left_out_are_counted() {
        let root = std::env::temp_dir().join(format!("apz-cli-upload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["site/css", "site/.git", "site/node_modules/left"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "site/index.html",
            "site/css/app.css",
            "site/.git/config",
            "site/node_modules/left/x.js",
            "site/.DS_Store",
            "README.md",
        ] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        let found = collect(&[root.join("site"), root.join("README.md")]).unwrap();
        let mut paths: Vec<&str> = found.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["README.md", "site/css/app.css", "site/index.html"]);
        assert_eq!(found.left_out, 3, ".git, node_modules and .DS_Store");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_named_like_a_skipped_name_is_left_out_too() {
        let root =
            std::env::temp_dir().join(format!("apz-cli-upload-named-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".DS_Store"), "x").unwrap();
        let found = collect(&[root.join(".DS_Store")]).unwrap();
        assert!(found.files.is_empty());
        assert_eq!(found.left_out, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_name_with_a_backslash_is_refused() {
        assert!(check_name("docs/a\\b.md").is_err());
        assert!(check_name("docs/a.md").is_ok());
        assert!(check_name(&"x".repeat(401)).is_err());
    }
}
