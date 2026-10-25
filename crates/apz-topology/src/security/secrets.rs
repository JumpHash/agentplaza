//! The secrets scan of a release's tree (PLAN 22.8): the hub's own detector (`apz-secrets`, the shapes the hooks refuse),
//! run over every line of every text file in the tree. A hit names the file, the line and the shape, never the value.

use std::fs;
use std::path::Path;

use super::{Hit, Outcome};

/// Directories the walk does not enter: version control, build output and installed packages.
const SKIPPED: &[&str] = &[".git", "target", "node_modules"];
/// Files larger than this are not read (a release's sources are small).
const MAX_BYTES: u64 = 1 << 20;

/// The secrets scan of the tree under `root`, as its stage (`secrets`). A tree that cannot be read is a skip.
pub fn stage(root: &Path) -> Outcome {
    match scan_tree(root) {
        Ok(hits) => {
            let files = format!("{} hits", hits.len());
            Outcome::ran("secrets", hits, files)
        }
        Err(why) => Outcome::skipped("secrets", format!("the tree could not be read: {why}")),
    }
}

/// Every secret-shaped line under `root`, in path order.
pub fn scan_tree(root: &Path) -> Result<Vec<Hit>, String> {
    let mut hits = Vec::new();
    walk(root, root, &mut hits)?;
    Ok(hits)
}

fn walk(root: &Path, dir: &Path, hits: &mut Vec<Hit>) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let name = entry.file_name();
            if !SKIPPED.iter().any(|skip| name == *skip) {
                walk(root, &path, hits)?;
            }
        } else if kind.is_file() {
            scan_file(root, &path, hits);
        }
    }
    Ok(())
}

fn scan_file(root: &Path, path: &Path, hits: &mut Vec<Hit>) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() > MAX_BYTES {
        return;
    }
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    // A binary file (a NUL byte in its first part) is not text to scan.
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    // The subject names the file with `/` on every host, so a finding reads the same on Windows.
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    for (index, line) in text.lines().enumerate() {
        if let Some(shape) = apz_secrets::shape(line) {
            hits.push(Hit {
                id: "secret".into(),
                severity: "high".into(),
                subject: format!("{relative}:{}", index + 1),
                detail: format!("a value shaped like a {shape} secret"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seeded_key_in_a_release_tree_is_found_by_file_and_line() {
        let tree = tempfile::tempdir().unwrap();
        fs::create_dir_all(tree.path().join("config")).unwrap();
        fs::write(
            tree.path().join("config/app.env"),
            "NAME=app\nAWS_KEY=AKIAIOSFODNN7EXAMPLE\n",
        )
        .unwrap();
        fs::write(tree.path().join("src.rs"), "fn main() {}\n").unwrap();
        let outcome = stage(tree.path());
        assert!(outcome.ran);
        assert_eq!(outcome.hits.len(), 1, "{:?}", outcome.hits);
        assert_eq!(outcome.hits[0].subject, "config/app.env:2");
        assert!(!format!("{:?}", outcome.hits).contains("IOSFODNN7"));
    }

    #[test]
    fn a_clean_tree_has_no_hits_and_the_skipped_directories_are_not_read() {
        let tree = tempfile::tempdir().unwrap();
        fs::write(tree.path().join("lib.rs"), "pub fn token() {}\n").unwrap();
        fs::create_dir_all(tree.path().join("target")).unwrap();
        fs::write(
            tree.path().join("target/leftover.txt"),
            "AKIAIOSFODNN7EXAMPLE\n",
        )
        .unwrap();
        let outcome = stage(tree.path());
        assert!(outcome.ran);
        assert!(outcome.hits.is_empty(), "{:?}", outcome.hits);
    }

    #[test]
    fn a_missing_tree_is_a_skip_not_a_clean_scan() {
        let outcome = stage(Path::new("/nonexistent/apz-release-tree"));
        assert!(!outcome.ran);
        assert!(outcome.reason.contains("could not be read"));
    }
}
