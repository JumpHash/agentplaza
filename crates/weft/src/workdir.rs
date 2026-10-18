//! A lane's working directory: the files, and `.weft/` beside them.
//!
//! `.weft/lane.json` says which hub, repo and change the directory belongs to, the base it was checked out from,
//! and the files of the last revision written here. `.weft/objects/` keeps fetched objects for diffs and restores.
//! `.weft/outbox/` holds snapshots that could not reach the hub yet, sent in order on the next command.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hash::Hash;
use crate::model::{Delta, Entry, Tree};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Lane {
    pub hub: String,
    pub repo: String,
    pub change: String,
    pub base: String,
    /// The revision the files were last written from or snapshotted as.
    pub at: String,
    pub base_files: BTreeMap<String, Entry>,
    pub files: BTreeMap<String, Entry>,
    /// The next draft number, as far as this directory knows.
    pub next_draft: u32,
    #[serde(default)]
    pub sparse: Vec<String>,
}

pub struct Workdir {
    pub root: PathBuf,
}

impl Workdir {
    /// The working directory at or above `start` that has a `.weft/`.
    pub fn find(start: &Path) -> Option<Workdir> {
        let mut at = Some(start);
        while let Some(dir) = at {
            if dir.join(".weft").join("lane.json").exists() {
                return Some(Workdir {
                    root: dir.to_path_buf(),
                });
            }
            at = dir.parent();
        }
        None
    }

    pub fn meta(&self) -> PathBuf {
        self.root.join(".weft")
    }

    pub fn lane(&self) -> Result<Lane, String> {
        let p = self.meta().join("lane.json");
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
    }

    pub fn save_lane(&self, lane: &Lane) -> Result<(), String> {
        std::fs::create_dir_all(self.meta()).map_err(|e| e.to_string())?;
        write_atomic(
            &self.meta().join("lane.json"),
            serde_json::to_string_pretty(lane).unwrap().as_bytes(),
        )
    }

    pub fn object(&self, h: &Hash) -> Option<Vec<u8>> {
        let hex = h.hex();
        std::fs::read(self.meta().join("objects").join(&hex[..2]).join(&hex[2..]))
            .ok()
            .filter(|b| Hash::of(b) == *h)
    }

    pub fn put_object(&self, h: &Hash, bytes: &[u8]) -> Result<(), String> {
        let hex = h.hex();
        let p = self.meta().join("objects").join(&hex[..2]).join(&hex[2..]);
        if p.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        write_atomic(&p, bytes)
    }

    /// The working files as a tree, honouring `.weftignore` and `.gitignore`.
    pub fn scan(&self) -> Result<(Tree, BTreeMap<String, PathBuf>), String> {
        let ignore = Ignore::load(&self.root);
        let mut tree = Tree::new();
        let mut paths = BTreeMap::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            for e in entries.flatten() {
                let path = e.path();
                let rel = path
                    .strip_prefix(&self.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                if rel == ".weft" || rel == ".git" || ignore.ignores(&rel, meta.is_dir()) {
                    continue;
                }
                if meta.is_dir() {
                    stack.push(path);
                    continue;
                }
                let (bytes, mode) = if meta.file_type().is_symlink() {
                    (
                        std::fs::read_link(&path)
                            .map_err(|e| e.to_string())?
                            .to_string_lossy()
                            .as_bytes()
                            .to_vec(),
                        0o120000,
                    )
                } else {
                    (
                        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
                        if executable(&meta) {
                            0o100755
                        } else {
                            0o100644
                        },
                    )
                };
                tree.insert(
                    rel.clone(),
                    Entry {
                        hash: Hash::of(&bytes),
                        mode,
                    },
                );
                paths.insert(rel, path);
            }
        }
        Ok((tree, paths))
    }

    /// Writes a file as the tree says.
    pub fn write_file(&self, path: &str, bytes: &[u8], mode: u32) -> Result<(), String> {
        let p = self.root.join(path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_file(&p);
        if mode == 0o120000 {
            #[cfg(unix)]
            {
                return std::os::unix::fs::symlink(String::from_utf8_lossy(bytes).to_string(), &p)
                    .map_err(|e| e.to_string());
            }
        }
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        #[cfg(unix)]
        if mode == 0o100755 {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        Ok(())
    }

    pub fn remove_file(&self, path: &str) {
        let p = self.root.join(path);
        let _ = std::fs::remove_file(&p);
        // Drop folders left empty.
        let mut dir = p.parent();
        while let Some(d) = dir {
            if d == self.root || std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }

    pub fn outbox(&self) -> Result<Vec<(PathBuf, Value)>, String> {
        let dir = self.meta().join("outbox");
        let mut items: Vec<(PathBuf, Value)> = Vec::new();
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let text = std::fs::read_to_string(e.path()).map_err(|e| e.to_string())?;
            items.push((
                e.path(),
                serde_json::from_str(&text).map_err(|e| e.to_string())?,
            ));
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(items)
    }

    pub fn queue(&self, item: &Value) -> Result<PathBuf, String> {
        let dir = self.meta().join("outbox");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        let p = dir.join(format!("{:06}-{}.json", n + 1, crate::record::now_ms()));
        write_atomic(&p, serde_json::to_string_pretty(item).unwrap().as_bytes())?;
        Ok(p)
    }
}

/// The deltas from `base` to `now`: what is new or different, and what is gone.
pub fn deltas(base: &BTreeMap<String, Entry>, now: &Tree) -> Vec<Delta> {
    let mut out = Vec::new();
    for (p, e) in now {
        if base.get(p) != Some(e) {
            out.push(Delta {
                path: p.clone(),
                after: Some(e.hash),
                mode: e.mode,
            });
        }
    }
    for p in base.keys() {
        if !now.contains_key(p) {
            out.push(Delta {
                path: p.clone(),
                after: None,
                mode: 0o100644,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

pub fn write_atomic(p: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, p).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(unix)]
fn executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &std::fs::Metadata) -> bool {
    false
}

/// `.gitignore`-style patterns (the common subset): `name`, `*.ext`, `dir/`, `/rooted`, `a/**/b`, `!keep`.
pub struct Ignore {
    rules: Vec<(String, bool, bool, bool)>,
}

impl Ignore {
    pub fn load(root: &Path) -> Ignore {
        let mut rules = Vec::new();
        for f in [".gitignore", ".weftignore"] {
            for line in std::fs::read_to_string(root.join(f))
                .unwrap_or_default()
                .lines()
            {
                let l = line.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                let (negate, l) = match l.strip_prefix('!') {
                    Some(r) => (true, r),
                    None => (false, l),
                };
                let dir_only = l.ends_with('/');
                let l = l.trim_end_matches('/');
                let rooted = l.starts_with('/') || l.contains('/');
                rules.push((
                    l.trim_start_matches('/').to_string(),
                    negate,
                    dir_only,
                    rooted,
                ));
            }
        }
        Ignore { rules }
    }

    pub fn ignores(&self, rel: &str, is_dir: bool) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let mut out = false;
        for (pat, negate, dir_only, rooted) in &self.rules {
            if *dir_only && !is_dir {
                continue;
            }
            let hit = if *rooted {
                crate::derived::glob(pat, rel) || crate::derived::glob(&format!("{pat}/**"), rel)
            } else {
                crate::derived::glob(pat, name)
            };
            if hit {
                out = !negate;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_like_git() {
        let dir = std::env::temp_dir().join(format!("weft-ignore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".gitignore"),
            "target/\n*.log\n/build\n!keep.log\n",
        )
        .unwrap();
        let ig = Ignore::load(&dir);
        assert!(ig.ignores("target", true));
        assert!(!ig.ignores("target", false));
        assert!(ig.ignores("a/b/x.log", false));
        assert!(!ig.ignores("keep.log", false));
        assert!(ig.ignores("build", true));
        assert!(!ig.ignores("src/build", true));
        std::fs::remove_dir_all(&dir).ok();
    }
}
