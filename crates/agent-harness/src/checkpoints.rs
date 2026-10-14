//! Workspace checkpoints: a snapshot at the start of every piece of work (and on demand),
//! a diff against any of them, and restore — the agent's undo, and the owner's review.
//!
//! Pure Rust, no git. Snapshots live under `<workspace>/.plaza/checkpoints/`: file contents
//! in `objects/<sha256 hex>` (written once, shared by every snapshot that holds them) and one
//! `snaps/<n>-<label>.json` per checkpoint listing each file's hash, mode and size.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::files::Workspace;

/// Files larger than this are recorded as skipped rather than snapshotted.
const MAX_FILE: u64 = 8 * 1024 * 1024;
/// Snapshots kept; the oldest are dropped past this.
const MAX_SNAPS: usize = 50;
/// The diff output cap, in bytes.
const DIFF_CAP: usize = 64 * 1024;
/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;
/// The largest line-by-line LCS table the diff builds; past it a changed block is shown whole.
const LCS_CELLS: usize = 4_000_000;
/// Directories never walked.
const SKIP_DIRS: [&str; 6] = [".plaza", ".git", "node_modules", "target", ".npm-global", ".cache"];

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    hash: String,
    mode: u32,
    size: u64,
}

#[derive(Clone, Debug)]
struct Snap {
    n: u64,
    id: String,
    label: String,
    at_ms: u64,
    files: BTreeMap<String, Entry>,
    skipped: Vec<String>,
}

/// The workspace as it is now: its files, where they are, and what was skipped.
#[derive(Default)]
struct Tree {
    files: BTreeMap<String, Entry>,
    paths: BTreeMap<String, PathBuf>,
    skipped: Vec<String>,
}

/// One file's change, for the summary lines and the diff body.
struct Change {
    summary: String,
    text: String,
}

/// Snapshots the workspace under `label`, returning the new checkpoint's id.
pub fn snapshot(workspace: &Workspace, label: &str) -> Result<String, String> {
    let base = base_dir(workspace);
    let objects = base.join("objects");
    let snaps_dir = base.join("snaps");
    fs::create_dir_all(&objects).map_err(|e| e.to_string())?;
    fs::create_dir_all(&snaps_dir).map_err(|e| e.to_string())?;
    let tree = walk(&workspace.dir(), Some(objects.as_path()))?;
    let n = load_snaps(&base).last().map_or(1, |s| s.n + 1);
    let slug: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(40)
        .collect();
    let id = format!("{n}-{slug}");
    let snap = Snap {
        n,
        id: id.clone(),
        label: label.to_string(),
        at_ms: now_ms(),
        files: tree.files,
        skipped: tree.skipped,
    };
    fs::write(snaps_dir.join(format!("{id}.json")), to_json(&snap).to_string()).map_err(|e| e.to_string())?;
    prune(&base)?;
    Ok(id)
}

/// The workspace's changes since a checkpoint, as summary lines and a unified diff.
pub fn diff(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let base = base_dir(workspace);
    let snaps = load_snaps(&base);
    let since = match input.get("since").and_then(Value::as_str) {
        Some(id) => find(&snaps, id)?,
        None => snaps
            .iter()
            .rev()
            .find(|s| s.label.starts_with("work-"))
            .ok_or("no work checkpoint yet; pass since")?,
    };
    let scope = scope_of(input);
    let changes = changed_files(workspace, &base, since, scope.as_deref())?;
    let mut summary = Vec::new();
    let mut body = String::new();
    for change in &changes {
        let rendered = file_change(&change.key, change.old.as_deref(), change.new.as_deref());
        summary.push(rendered.summary);
        body.push_str(&rendered.text);
    }
    if summary.is_empty() {
        return Ok(format!("no changes since {} ({} ago)", since.id, age(since.at_ms)));
    }
    let mut out = format!("changes since {} ({} ago):\n", since.id, age(since.at_ms));
    out.push_str(&summary.join("\n"));
    out.push_str("\n\n");
    out.push_str(&cap(body));
    Ok(out)
}

/// One file that differs between a checkpoint and the workspace now, with its bytes on each side
/// (`None` where the file is absent).
struct Changed {
    key: String,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
}

/// The files under `scope` whose hash or mode differs between `since` and the workspace now, in path order.
fn changed_files(workspace: &Workspace, base: &Path, since: &Snap, scope: Option<&str>) -> Result<Vec<Changed>, String> {
    let now = walk(workspace.dir(), None)?;
    let keys: BTreeSet<&String> = since
        .files
        .keys()
        .chain(now.files.keys())
        .filter(|k| within(k.as_str(), scope))
        .collect();
    let mut changed = Vec::new();
    for key in keys {
        let old = since.files.get(key.as_str());
        let new = now.files.get(key.as_str());
        if old.map(|e| (e.hash.as_str(), e.mode)) == new.map(|e| (e.hash.as_str(), e.mode)) {
            continue;
        }
        let old = old.map(|e| read_object(base, &e.hash)).transpose()?;
        let new = match now.paths.get(key.as_str()) {
            Some(path) => Some(fs::read(path).map_err(|e| e.to_string())?),
            None => None,
        };
        changed.push(Changed { key: key.clone(), old, new });
    }
    Ok(changed)
}

/// Every change since checkpoint `n` as one git-style unified diff, with no cap: the owner's diff.
/// Files for which `keep` is false are left out.
pub fn unified_since(workspace: &Workspace, n: u64, keep: impl Fn(&str) -> bool) -> Result<String, String> {
    let base = base_dir(workspace);
    let snaps = load_snaps(&base);
    let since = snaps.iter().find(|s| s.n == n).ok_or_else(|| format!("no checkpoint {n}"))?;
    let changes = changed_files(workspace, &base, since, None)?;
    Ok(changes
        .iter()
        .filter(|change| keep(&change.key))
        .map(|change| git_change(&change.key, change.old.as_deref(), change.new.as_deref()))
        .collect())
}

/// One file as git writes it: a `diff --git` header, the new or deleted marker, then the
/// `---`/`+++` names and the hunks. A binary file has no hunks.
fn git_change(key: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> String {
    let mut out = format!("diff --git a/{key} b/{key}\n");
    if old.is_none() {
        out.push_str("new file mode 100644\n");
    }
    if new.is_none() {
        out.push_str("deleted file mode 100644\n");
    }
    let (a_name, b_name) = (side('a', key, old.is_some()), side('b', key, new.is_some()));
    let a = old.unwrap_or_default();
    let b = new.unwrap_or_default();
    let binary = a.contains(&0)
        || b.contains(&0)
        || std::str::from_utf8(a).is_err()
        || std::str::from_utf8(b).is_err();
    if binary {
        out.push_str(&format!("Binary files {a_name} and {b_name} differ\n"));
        return out;
    }
    out.push_str(&format!("--- {a_name}\n+++ {b_name}\n"));
    let a_lines: Vec<&str> = std::str::from_utf8(a).unwrap_or_default().lines().collect();
    let b_lines: Vec<&str> = std::str::from_utf8(b).unwrap_or_default().lines().collect();
    let ops = line_diff(&a_lines, &b_lines);
    if !ops.is_empty() {
        out.push_str(&hunks(&ops));
    }
    out
}

/// A checkpoint as the owner's list shows it.
pub struct Summary {
    pub n: u64,
    pub id: String,
    pub label: String,
    pub at_ms: u64,
    /// How many files the checkpoint holds.
    pub files: usize,
}

/// Every checkpoint, oldest first.
pub fn summaries(workspace: &Workspace) -> Vec<Summary> {
    load_snaps(&base_dir(workspace))
        .into_iter()
        .map(|snap| Summary {
            n: snap.n,
            id: snap.id,
            label: snap.label,
            at_ms: snap.at_ms,
            files: snap.files.len(),
        })
        .collect()
}

/// Rolls the workspace or a path back to a checkpoint. A `before-restore` checkpoint is taken first.
pub fn restore(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let id = input.get("id").and_then(Value::as_str).ok_or("checkpoint_restore needs an id")?;
    let base = base_dir(workspace);
    let snap = find(&load_snaps(&base), id)?.clone();
    let scope = scope_of(input);
    let scope = scope.as_deref();
    let root = workspace.dir().to_path_buf();
    let now = walk(&root, None)?;
    // Read everything needed before the new checkpoint can prune the one being restored.
    let mut plan = Vec::new();
    for (key, want) in snap.files.iter().filter(|(key, _)| within(key.as_str(), scope)) {
        if now.files.get(key.as_str()) == Some(want) {
            continue;
        }
        plan.push((key.clone(), read_object(&base, &want.hash)?, want.mode));
    }
    let added: Vec<String> = now
        .files
        .keys()
        .filter(|key| within(key.as_str(), scope) && !snap.files.contains_key(key.as_str()))
        .cloned()
        .collect();
    let removing = scope.is_none() && !added.is_empty();
    let what = scope.unwrap_or("the whole workspace");
    if plan.is_empty() && !removing {
        return Ok(format!("nothing to restore: {what} already matches {}", snap.id));
    }
    let before = snapshot(workspace, "before-restore")?;
    let mut lines = Vec::new();
    for (key, bytes, mode) in &plan {
        let path = abs_path(&root, key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        fs::set_permissions(&path, fs::Permissions::from_mode(*mode)).map_err(|e| e.to_string())?;
        lines.push(format!("restored {key}"));
    }
    if scope.is_none() {
        for key in &added {
            fs::remove_file(abs_path(&root, key)?).map_err(|e| e.to_string())?;
            lines.push(format!("removed {key}"));
        }
    } else if !added.is_empty() {
        lines.push(format!("kept {} files added since; only a whole-workspace restore removes them", added.len()));
    }
    Ok(format!(
        "rolled {what} back to {}; the state before is checkpoint {before}\n{}",
        snap.id,
        lines.join("\n")
    ))
}

/// The checkpoints, newest first.
pub fn list(workspace: &Workspace, _input: &Value) -> Result<String, String> {
    let snaps = load_snaps(&base_dir(workspace));
    if snaps.is_empty() {
        return Ok("no checkpoints yet".to_string());
    }
    let lines: Vec<String> = snaps
        .iter()
        .rev()
        .map(|s| {
            let skipped = if s.skipped.is_empty() { String::new() } else { format!(", {} skipped", s.skipped.len()) };
            format!("{}  {}  {} ago  {} files{skipped}", s.id, s.label, age(s.at_ms), s.files.len())
        })
        .collect();
    Ok(lines.join("\n"))
}

fn base_dir(workspace: &Workspace) -> PathBuf {
    workspace.dir().join(".plaza").join("checkpoints")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn age(at_ms: u64) -> String {
    let secs = now_ms().saturating_sub(at_ms) / 1000;
    match secs {
        0..=119 => format!("{secs}s"),
        120..=7199 => format!("{}m", secs / 60),
        7200..=172_799 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

pub(crate) fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn scope_of(input: &Value) -> Option<String> {
    input
        .get("path")
        .and_then(Value::as_str)
        .map(|p| p.trim().trim_start_matches("./").trim_matches('/').to_string())
        .filter(|p| !p.is_empty())
}

fn within(key: &str, scope: Option<&str>) -> bool {
    match scope {
        None => true,
        Some(s) => key == s || key.starts_with(&format!("{s}/")),
    }
}

fn find<'a>(snaps: &'a [Snap], id: &str) -> Result<&'a Snap, String> {
    snaps
        .iter()
        .rev()
        .find(|s| s.id == id || s.n.to_string() == id)
        .ok_or_else(|| format!("no checkpoint {id}"))
}

/// Walks the workspace. With `objects`, each file's contents are stored there (once).
fn walk(root: &Path, objects: Option<&Path>) -> Result<Tree, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let mut tree = Tree::default();
    walk_dir(root, root, objects, &mut tree)?;
    Ok(tree)
}

fn walk_dir(root: &Path, dir: &Path, objects: Option<&Path>, tree: &mut Tree) -> Result<(), String> {
    let Ok(items) = fs::read_dir(dir) else {
        return Ok(());
    };
    for item in items.flatten() {
        let path = item.path();
        let Ok(kind) = item.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk_dir(root, &path, objects, tree)?;
            }
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Ok(rel_path) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let Ok(meta) = item.metadata() else {
            continue;
        };
        if meta.len() > MAX_FILE {
            tree.skipped.push(rel);
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let hash = sha_hex(&bytes);
        if let Some(objects) = objects {
            let object = objects.join(&hash);
            if !object.exists() {
                fs::write(&object, &bytes).map_err(|e| e.to_string())?;
            }
        }
        let entry = Entry {
            hash,
            mode: meta.permissions().mode() & 0o7777,
            size: bytes.len() as u64,
        };
        tree.paths.insert(rel.clone(), path);
        tree.files.insert(rel, entry);
    }
    Ok(())
}

fn read_object(base: &Path, hash: &str) -> Result<Vec<u8>, String> {
    fs::read(base.join("objects").join(hash)).map_err(|e| format!("checkpoint object {hash} is missing: {e}"))
}

fn abs_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(format!("bad path in checkpoint: {rel}"));
    }
    Ok(root.join(rel))
}

fn to_json(snap: &Snap) -> Value {
    let files: serde_json::Map<String, Value> = snap
        .files
        .iter()
        .map(|(path, e)| (path.clone(), json!({"hash": e.hash, "mode": e.mode, "size": e.size})))
        .collect();
    json!({"id": snap.id, "label": snap.label, "at_ms": snap.at_ms, "files": files, "skipped": snap.skipped})
}

fn from_json(n: u64, v: &Value) -> Option<Snap> {
    let mut files = BTreeMap::new();
    for (path, e) in v.get("files")?.as_object()? {
        let entry = Entry {
            hash: e.get("hash")?.as_str()?.to_string(),
            mode: u32::try_from(e.get("mode")?.as_u64()?).ok()?,
            size: e.get("size")?.as_u64()?,
        };
        files.insert(path.clone(), entry);
    }
    let skipped = v
        .get("skipped")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| items.iter().filter_map(|s| s.as_str().map(str::to_string)).collect());
    Some(Snap {
        n,
        id: v.get("id")?.as_str()?.to_string(),
        label: v.get("label")?.as_str()?.to_string(),
        at_ms: v.get("at_ms")?.as_u64()?,
        files,
        skipped,
    })
}

/// Every readable checkpoint, oldest first.
fn load_snaps(base: &Path) -> Vec<Snap> {
    let Ok(items) = fs::read_dir(base.join("snaps")) else {
        return Vec::new();
    };
    let mut snaps: Vec<Snap> = items
        .flatten()
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") {
                return None;
            }
            let n = name.split('-').next()?.parse::<u64>().ok()?;
            let value: Value = serde_json::from_slice(&fs::read(item.path()).ok()?).ok()?;
            from_json(n, &value)
        })
        .collect();
    snaps.sort_by_key(|s| s.n);
    snaps
}

/// Drops the oldest checkpoints past the cap, then the objects no kept checkpoint holds.
fn prune(base: &Path) -> Result<(), String> {
    let mut snaps = load_snaps(base);
    let extra = snaps.len().saturating_sub(MAX_SNAPS);
    for old in snaps.drain(..extra) {
        fs::remove_file(base.join("snaps").join(format!("{}.json", old.id))).map_err(|e| e.to_string())?;
    }
    let live: BTreeSet<String> = snaps.iter().flat_map(|s| s.files.values().map(|e| e.hash.clone())).collect();
    let Ok(objects) = fs::read_dir(base.join("objects")) else {
        return Ok(());
    };
    for object in objects.flatten() {
        if !live.contains(&object.file_name().to_string_lossy().into_owned()) {
            let _ = fs::remove_file(object.path());
        }
    }
    Ok(())
}

fn side(prefix: char, key: &str, present: bool) -> String {
    if present {
        format!("{prefix}/{key}")
    } else {
        "/dev/null".to_string()
    }
}

fn file_change(key: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> Change {
    let tag = match (old.is_some(), new.is_some()) {
        (false, true) => 'A',
        (true, false) => 'D',
        _ => 'M',
    };
    let a = old.unwrap_or_default();
    let b = new.unwrap_or_default();
    let binary = a.contains(&0)
        || b.contains(&0)
        || std::str::from_utf8(a).is_err()
        || std::str::from_utf8(b).is_err();
    let header = format!("--- {}\n+++ {}\n", side('a', key, old.is_some()), side('b', key, new.is_some()));
    if binary {
        let summary = if tag == 'M' { format!("M {key} (binary)") } else { format!("{tag} {key}") };
        return Change { summary, text: format!("{header}binary differs\n") };
    }
    let a_text = std::str::from_utf8(a).unwrap_or_default();
    let b_text = std::str::from_utf8(b).unwrap_or_default();
    let a_lines: Vec<&str> = a_text.lines().collect();
    let b_lines: Vec<&str> = b_text.lines().collect();
    let ops = line_diff(&a_lines, &b_lines);
    let added = ops.iter().filter(|op| op.0 == '+').count();
    let removed = ops.iter().filter(|op| op.0 == '-').count();
    let summary = if tag == 'M' {
        format!("M {key} (+{added} −{removed})")
    } else {
        format!("{tag} {key}")
    };
    let text = if ops.is_empty() { String::new() } else { format!("{header}{}", hunks(&ops)) };
    Change { summary, text }
}

/// A line diff as (' ' | '-' | '+', line) pairs: common prefix and suffix are kept as they are,
/// and the middle is aligned by longest common subsequence.
fn line_diff<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, &'a str)> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let am = &a[pre..a.len() - suf];
    let bm = &b[pre..b.len() - suf];
    let mut out: Vec<(char, &'a str)> = a[..pre].iter().map(|l| (' ', *l)).collect();
    if am.len() * bm.len() > LCS_CELLS {
        out.extend(am.iter().map(|l| ('-', *l)));
        out.extend(bm.iter().map(|l| ('+', *l)));
    } else {
        let (n, m) = (am.len(), bm.len());
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if am[i] == bm[j] {
                    t[(i + 1) * w + j + 1] + 1
                } else {
                    t[(i + 1) * w + j].max(t[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if am[i] == bm[j] {
                out.push((' ', am[i]));
                i += 1;
                j += 1;
            } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
                out.push(('-', am[i]));
                i += 1;
            } else {
                out.push(('+', bm[j]));
                j += 1;
            }
        }
        out.extend(am[i..].iter().map(|l| ('-', *l)));
        out.extend(bm[j..].iter().map(|l| ('+', *l)));
    }
    out.extend(a[a.len() - suf..].iter().map(|l| (' ', *l)));
    out
}

/// Unified hunks with `CONTEXT` lines around each run of changes; nearby runs share a hunk.
fn hunks(ops: &[(char, &str)]) -> String {
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.0 != ' ')
        .map(|(i, _)| i)
        .collect();
    let mut out = String::new();
    let mut k = 0;
    while k < changed.len() {
        let first = changed[k];
        let mut last = first;
        k += 1;
        while k < changed.len() && changed[k] <= last + 2 * CONTEXT + 1 {
            last = changed[k];
            k += 1;
        }
        let start = first.saturating_sub(CONTEXT);
        let end = (last + CONTEXT + 1).min(ops.len());
        let old_start = ops[..start].iter().filter(|op| op.0 != '+').count() + 1;
        let new_start = ops[..start].iter().filter(|op| op.0 != '-').count() + 1;
        let old_len = ops[start..end].iter().filter(|op| op.0 != '+').count();
        let new_len = ops[start..end].iter().filter(|op| op.0 != '-').count();
        out.push_str(&format!("@@ -{old_start},{old_len} +{new_start},{new_len} @@\n"));
        for (c, line) in &ops[start..end] {
            out.push(*c);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn cap(mut text: String) -> String {
    if text.len() > DIFF_CAP {
        let mut cut = DIFF_CAP;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n... diff truncated at 64 KiB\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!("checkpoints-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        let workspace = Workspace::at(&dir);
        (dir, workspace)
    }

    fn latest(workspace: &Workspace) -> Snap {
        load_snaps(&base_dir(workspace)).pop().expect("a snapshot")
    }

    #[test]
    fn a_snapshot_diffs_and_restores_the_workspace() {
        let (dir, ws) = scratch("roundtrip");
        fs::write(dir.join("keep.txt"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\n").unwrap();
        fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        fs::write(dir.join("node_modules/pkg/index.js"), "x").unwrap();
        let id = snapshot(&ws, "work-1").expect("snapshot");

        fs::write(dir.join("keep.txt"), "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n").unwrap();
        fs::remove_file(dir.join("gone.txt")).unwrap();
        fs::write(dir.join("new.txt"), "fresh\n").unwrap();

        let report = diff(&ws, &json!({})).expect("diff");
        assert!(report.contains("A new.txt"), "{report}");
        assert!(report.contains("M keep.txt (+1 −1)"), "{report}");
        assert!(report.contains("D gone.txt"), "{report}");
        assert!(report.contains("@@ -1,6 +1,6 @@"), "{report}");
        assert!(report.contains("-three") && report.contains("+THREE"), "{report}");

        let restored = restore(&ws, &json!({"id": id})).expect("restore");
        assert!(restored.contains("restored gone.txt") && restored.contains("removed new.txt"), "{restored}");
        assert_eq!(fs::read_to_string(dir.join("keep.txt")).unwrap(), "one\ntwo\nthree\nfour\nfive\nsix\nseven\n");
        assert_eq!(fs::read_to_string(dir.join("gone.txt")).unwrap(), "bye\n");
        assert!(!dir.join("new.txt").exists());
        assert!(diff(&ws, &json!({})).unwrap().starts_with("no changes"));
        assert!(list(&ws, &json!({})).unwrap().contains("before-restore"));
    }

    #[test]
    fn a_path_restore_touches_only_that_path() {
        let (dir, ws) = scratch("scoped");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "a1\n").unwrap();
        fs::write(dir.join("b.txt"), "b1\n").unwrap();
        let id = snapshot(&ws, "work-1").unwrap();
        fs::write(dir.join("src/a.rs"), "a2\n").unwrap();
        fs::write(dir.join("b.txt"), "b2\n").unwrap();

        restore(&ws, &json!({"id": id, "path": "src"})).expect("restore");
        assert_eq!(fs::read_to_string(dir.join("src/a.rs")).unwrap(), "a1\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "b2\n");
    }

    #[test]
    fn the_newest_fifty_snapshots_are_kept_with_their_objects() {
        let (dir, ws) = scratch("prune");
        for i in 0..55 {
            fs::write(dir.join("f.txt"), format!("version {i}\n")).unwrap();
            snapshot(&ws, "work-x").unwrap();
        }
        assert_eq!(list(&ws, &json!({})).unwrap().lines().count(), MAX_SNAPS);
        assert_eq!(fs::read_dir(base_dir(&ws).join("objects")).unwrap().count(), MAX_SNAPS);
        assert_eq!(latest(&ws).files["f.txt"].size, "version 54\n".len() as u64);
    }

    #[test]
    fn dependency_build_and_vcs_directories_are_not_snapshotted() {
        let (dir, ws) = scratch("skips");
        for d in [".git", "node_modules", "target", ".npm-global", ".cache", ".plaza"] {
            fs::create_dir_all(dir.join(d)).unwrap();
            fs::write(dir.join(d).join("x"), "x").unwrap();
        }
        fs::write(dir.join("ok.txt"), "ok").unwrap();
        fs::File::create(dir.join("huge.bin")).unwrap().set_len(MAX_FILE + 1).unwrap();
        snapshot(&ws, "work-1").unwrap();
        let snap = latest(&ws);
        let keys: Vec<String> = snap.files.keys().cloned().collect();
        assert_eq!(keys, vec!["ok.txt".to_string()]);
        assert_eq!(snap.skipped, vec!["huge.bin".to_string()]);
    }
}
