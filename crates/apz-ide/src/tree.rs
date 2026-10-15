//! Directory entries as the hub lists them, one page at a time, and the helpers
//! the file tree needs: children in display order, and the changed-files filter.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diff::{ChangeSet, Status};

/// Entries per page of a tree listing.
pub const PAGE_SIZE: usize = 500;

/// What an entry is. Symlinks are listed, never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// One path in a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Relative to the workspace root, `/`-separated, no leading slash.
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    /// The content hash. Files only; `None` for directories and symlinks.
    pub hash: Option<String>,
    /// Whether the content is binary (the IDE will not open it as text).
    pub binary: bool,
}

impl Entry {
    /// The last path segment.
    pub fn name(&self) -> &str {
        name_of(&self.path)
    }
}

/// One page of a directory listing. `next` is the cursor for the following page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreePage {
    pub entries: Vec<Entry>,
    pub next: Option<String>,
}

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The direct children of `dir` among `entries`, directories first, then by name
/// (case-insensitively, then exactly). `""` is the root. Only the entries given
/// are looked at, so pass every page you have.
pub fn children<'a>(entries: &'a [Entry], dir: &str) -> Vec<&'a Entry> {
    let dir = dir.trim_matches('/');
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut out: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            e.path
                .strip_prefix(&prefix)
                .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
        })
        .collect();
    out.sort_by(|a, b| display_order(a, b));
    out
}

fn display_order(a: &Entry, b: &Entry) -> Ordering {
    let (na, nb) = (a.name(), b.name());
    (a.kind != EntryKind::Dir)
        .cmp(&(b.kind != EntryKind::Dir))
        .then_with(|| na.to_lowercase().cmp(&nb.to_lowercase()))
        .then_with(|| na.cmp(nb))
}

/// The entries a change set touches, and their ancestor directories, sorted by path.
///
/// A file the change set removes is not in `entries`, so it comes back as a
/// `File` (size 0, no hash, `binary` as the diff says), and any ancestor that is
/// missing comes back as a `Dir`. A rename removes its old path, so that comes back too.
pub fn changed_filter(entries: &[Entry], changes: &ChangeSet) -> Vec<Entry> {
    // Changed paths (added, modified, deleted, renamed-to, renamed-from) with their binary flag.
    let mut files: BTreeMap<&str, bool> = BTreeMap::new();
    let mut dirs: BTreeSet<&str> = BTreeSet::new();
    for change in &changes.files {
        files.insert(&change.path, change.binary);
        if change.status == Status::Renamed
            && let Some(old) = &change.old_path
        {
            files.entry(old).or_insert(change.binary);
        }
    }
    for path in files.keys().copied() {
        for (i, _) in path.match_indices('/') {
            dirs.insert(&path[..i]);
        }
    }

    let mut out: BTreeMap<String, Entry> = BTreeMap::new();
    for e in entries {
        if files.contains_key(e.path.as_str()) || dirs.contains(e.path.as_str()) {
            out.insert(e.path.clone(), e.clone());
        }
    }
    for (path, binary) in &files {
        out.entry((*path).to_string()).or_insert_with(|| Entry {
            path: (*path).to_string(),
            kind: EntryKind::File,
            size: 0,
            hash: None,
            binary: *binary,
        });
    }
    for path in &dirs {
        out.entry((*path).to_string()).or_insert_with(|| Entry {
            path: (*path).to_string(),
            kind: EntryKind::Dir,
            size: 0,
            hash: None,
            binary: false,
        });
    }
    out.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;

    fn file(path: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::File,
            size: 10,
            hash: Some("h".into()),
            binary: false,
        }
    }

    fn dir(path: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::Dir,
            size: 0,
            hash: None,
            binary: false,
        }
    }

    #[test]
    fn changed_filter_keeps_only_what_the_diff_touches() {
        // A changed file is kept and the rest is dropped.
        {
            let entries = vec![
                dir("src"),
                file("src/main.rs"),
                file("Cargo.toml"),
                file("a.txt"),
            ];
            let changes = parse_unified(include_str!("../tests/fixtures/modify.diff")).unwrap();
            let kept = changed_filter(&entries, &changes);
            let got: Vec<&str> = kept.iter().map(|e| e.path.as_str()).collect();
            assert_eq!(got, ["a.txt"]);
            assert_eq!(kept[0].kind, EntryKind::File);
            assert!(
                kept[0].hash.is_some(),
                "a kept entry is the listed entry, not a synthesised one"
            );
        }

        // The ancestor dirs of a changed file are kept.
        {
            let entries = vec![
                dir("src"),
                dir("src/ui"),
                file("src/ui/app.rs"),
                file("src/main.rs"),
                dir("docs"),
                file("docs/guide.md"),
            ];
            let changes = parse_unified(
                "diff --git a/src/ui/app.rs b/src/ui/app.rs\n--- a/src/ui/app.rs\n+++ b/src/ui/app.rs\n@@ -1 +1 @@\n-a\n+b\n",
            )
            .unwrap();
            let kept = changed_filter(&entries, &changes);
            let got: Vec<&str> = kept.iter().map(|e| e.path.as_str()).collect();
            assert_eq!(got, ["src", "src/ui", "src/ui/app.rs"]);
            assert_eq!(kept[0].kind, EntryKind::Dir);
        }

        // Removed files and missing dirs are shown.
        {
            // Multi-file fixture: deleted.txt is gone from the tree, and nothing lists it.
            let changes = parse_unified(include_str!("../tests/fixtures/multi_file.diff")).unwrap();
            let entries = vec![file("a.txt"), file("Cargo.toml")];
            let kept = changed_filter(&entries, &changes);
            let got: Vec<(&str, EntryKind, bool)> = kept
                .iter()
                .map(|e| (e.path.as_str(), e.kind, e.binary))
                .collect();
            assert!(got.contains(&("a.txt", EntryKind::File, false)));
            assert!(got.contains(&("deleted.txt", EntryKind::File, false)));
            assert!(
                got.contains(&("old.txt", EntryKind::File, false)),
                "a rename removes its old path"
            );
            assert!(got.contains(&("bin.dat", EntryKind::File, true)));
            assert!(!got.iter().any(|(p, _, _)| *p == "Cargo.toml"));
            assert!(
                kept.windows(2).all(|w| w[0].path < w[1].path),
                "sorted by path"
            );
            let synthesised = kept.iter().find(|e| e.path == "deleted.txt").unwrap();
            assert_eq!(synthesised.size, 0);
            assert_eq!(synthesised.hash, None);
        }

        // The missing ancestors of a deleted file are synthesised.
        {
            let nested = parse_unified(
                "diff --git a/d/e/f.txt b/d/e/f.txt\ndeleted file mode 100644\n--- a/d/e/f.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n",
            )
            .unwrap();
            let kept = changed_filter(&[dir("d")], &nested);
            let got: Vec<(&str, EntryKind)> =
                kept.iter().map(|e| (e.path.as_str(), e.kind)).collect();
            assert_eq!(
                got,
                [
                    ("d", EntryKind::Dir),
                    ("d/e", EntryKind::Dir),
                    ("d/e/f.txt", EntryKind::File)
                ]
            );
        }
    }

    #[test]
    fn tree_page_round_trips_through_json() {
        let page = TreePage {
            entries: vec![dir("src"), file("a.txt")],
            next: Some("cursor-2".into()),
        };
        let v = serde_json::to_value(&page).unwrap();
        assert_eq!(v["entries"][0]["kind"], "dir");
        assert_eq!(v["next"], "cursor-2");
        let back: TreePage = serde_json::from_value(v).unwrap();
        assert_eq!(back, page);
    }
}
