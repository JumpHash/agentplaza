//! The one-time import of a git repository's history: one imported change per commit on the branch's first-parent
//! line, its author and date kept, its git id recorded. Tags become imported releases. Every imported tree is
//! checked against git's: Weft's copy of each commit gives the same git tree id, or the import stops.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::json;

use crate::git::{self, Kind, Oid, Pack, Repo};
use crate::gitwire::{self, Remote};
use crate::hash::Hash;
use crate::model::*;
use crate::store::Store;

/// Where the history comes from.
pub enum Source {
    Local(Repo),
    Fetched {
        pack: Pack,
        refs: std::collections::BTreeMap<String, Oid>,
    },
}

impl Source {
    /// A local path, or a remote fetched once (the branch's full history).
    pub fn open(location: &str, token: Option<String>, branch: &str) -> Result<Source, String> {
        match Remote::parse(location, token) {
            Remote::Local(p) => Ok(Source::Local(Repo::open(&p)?)),
            remote => {
                let refs = gitwire::list_refs(&remote, "git-upload-pack")?;
                let want: Vec<Oid> = refs
                    .iter()
                    .filter(|(k, _)| {
                        *k == &format!("refs/heads/{branch}") || k.starts_with("refs/tags/")
                    })
                    .map(|(_, v)| *v)
                    .collect();
                if want.is_empty() {
                    return Err(format!("{} has no branch {branch}", remote.describe()));
                }
                let pack = gitwire::fetch(&remote, &want, &[])?;
                Ok(Source::Fetched { pack, refs })
            }
        }
    }

    pub fn read(&self, id: &Oid) -> Result<(Kind, Vec<u8>), String> {
        match self {
            Source::Local(r) => r.read(id),
            Source::Fetched { pack, .. } => pack.get(id),
        }
    }

    fn refs(&self) -> Result<std::collections::BTreeMap<String, Oid>, String> {
        match self {
            Source::Local(r) => r.refs(),
            Source::Fetched { refs, .. } => Ok(refs.clone()),
        }
    }

    fn resolve(&self, branch: &str) -> Result<Oid, String> {
        match self {
            Source::Local(r) => r.resolve(branch),
            Source::Fetched { refs, .. } => refs
                .get(&format!("refs/heads/{branch}"))
                .copied()
                .ok_or_else(|| format!("no branch {branch}")),
        }
    }

    fn tree_entries(&self, id: &Oid) -> Result<Vec<git::TreeEntry>, String> {
        let (k, d) = self.read(id)?;
        if k != Kind::Tree {
            return Err(format!("{} is not a tree", git::hex(id)));
        }
        git::parse_tree(&d)
    }

    /// path → new (mode, id) or `None`, between two trees; only differing subtrees are read.
    fn diff(
        &self,
        old: Option<&Oid>,
        new: &Oid,
    ) -> Result<std::collections::BTreeMap<String, Option<(u32, Oid)>>, String> {
        let mut out = std::collections::BTreeMap::new();
        self.diff_into(old, Some(new), "", &mut out)?;
        Ok(out)
    }

    fn diff_into(
        &self,
        old: Option<&Oid>,
        new: Option<&Oid>,
        prefix: &str,
        out: &mut std::collections::BTreeMap<String, Option<(u32, Oid)>>,
    ) -> Result<(), String> {
        if old == new {
            return Ok(());
        }
        let list = |t: Option<&Oid>| -> Result<std::collections::BTreeMap<String, git::TreeEntry>, String> {
            match t {
                Some(t) => Ok(self.tree_entries(t)?.into_iter().map(|e| (e.name.clone(), e)).collect()),
                None => Ok(Default::default()),
            }
        };
        let (a, b) = (list(old)?, list(new)?);
        let names: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        for name in names {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let (ea, eb) = (a.get(name), b.get(name));
            if ea == eb {
                continue;
            }
            let (ta, tb) = (
                ea.filter(|e| e.is_tree()).map(|e| &e.id),
                eb.filter(|e| e.is_tree()).map(|e| &e.id),
            );
            if ta.is_some() || tb.is_some() {
                self.diff_into(ta, tb, &path, out)?;
            }
            let fa = ea.filter(|e| !e.is_tree() && e.mode != 0o160000);
            let fb = eb.filter(|e| !e.is_tree() && e.mode != 0o160000);
            match (fa, fb) {
                (_, Some(f)) if fa != Some(f) => {
                    out.insert(path, Some((f.mode, f.id)));
                }
                (Some(_), None) => {
                    out.insert(path, None);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub repo: String,
    pub branch: String,
    pub commits: usize,
    pub skipped: usize,
    pub files: usize,
    pub releases: Vec<String>,
    pub head: Option<String>,
    pub checked: usize,
}

/// `Co-authored-by` trailers that name an AI assistant.
pub fn ai_coauthors(message: &str) -> Vec<String> {
    const AI: &[&str] = &[
        "claude",
        "anthropic",
        "gpt",
        "openai",
        "copilot",
        "gemini",
        "cursor",
        "codex",
        "devin",
        "aider",
        "bot",
    ];
    message
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let lower = l.to_ascii_lowercase();
            lower
                .strip_prefix("co-authored-by:")
                .map(|_| l["co-authored-by:".len()..].trim().to_string())
        })
        .filter(|who| AI.iter().any(|a| who.to_ascii_lowercase().contains(a)))
        .collect()
}

/// Imports `branch` from `source` into `repo` (which must exist). Commits already imported are skipped, so a
/// second run brings only what is new (adopting commits a remote received on its own).
pub fn import(
    store: &Store,
    repo: &str,
    source: &Source,
    branch: &str,
    reference: &str,
) -> Result<ImportReport, String> {
    let head = source.resolve(branch)?;
    // The first-parent line, oldest first.
    let mut line = Vec::new();
    let mut at = Some(head);
    while let Some(id) = at {
        let (k, d) = source.read(&id)?;
        if k != Kind::Commit {
            return Err(format!("{} is not a commit", git::hex(&id)));
        }
        let c = git::parse_commit(&d)?;
        at = c.parents.first().copied();
        line.push((id, c));
    }
    line.reverse();
    let imported: HashMap<String, RevId> = {
        let st = store.state();
        st.changes
            .values()
            .filter(|c| c.repo == repo)
            .filter_map(|c| Some((c.import.as_ref()?.git.clone(), c.landed.clone()?)))
            .collect()
    };
    let mut report = ImportReport {
        repo: repo.into(),
        branch: branch.into(),
        ..Default::default()
    };
    let mut by_commit: HashMap<Oid, RevId> = HashMap::new();
    let mut prev_tree: Option<Oid> = None;
    for (n, (id, c)) in line.iter().enumerate() {
        let hexid = git::hex(id);
        if let Some(rev) = imported.get(&hexid) {
            report.skipped += 1;
            by_commit.insert(*id, rev.clone());
            prev_tree = Some(c.tree);
            continue;
        }
        let expected = store.state().repo(repo)?.head(reference);
        if report.commits == 0 && report.skipped > 0 {
            // Resuming: the ref must still be at the last imported commit.
            let last = line[n - 1].0;
            if by_commit.get(&last) != Some(&expected) {
                return Err(format!(
                    "{repo}:{reference} moved past the imported history; its new commits cannot be imported under it"
                ));
            }
        }
        let mut deltas = Vec::new();
        for (path, entry) in source.diff(prev_tree.as_ref(), &c.tree)? {
            match entry {
                Some((mode, blob)) => {
                    let (k, bytes) = source.read(&blob)?;
                    if k != Kind::Blob {
                        return Err(format!("{path} is not a blob"));
                    }
                    let h = store.blobs.put(&bytes)?;
                    let mode = match mode {
                        0o100755 => 0o100755,
                        0o120000 => 0o120000,
                        _ => 0o100644,
                    };
                    deltas.push(Delta {
                        path,
                        after: Some(h),
                        mode,
                    });
                }
                None => deltas.push(Delta {
                    path,
                    after: None,
                    mode: 0o100644,
                }),
            }
        }
        report.files += deltas.len();
        let change = store.next_change();
        store.hub_append(
            "import",
            json!({
                "repo": repo, "ref": reference, "change": change, "git": hexid,
                "author_name": c.author.name, "author_email": c.author.email, "at_ms": c.author.ms(),
                "message": c.message, "expected": expected, "deltas": deltas, "ai_coauthors": ai_coauthors(&c.message),
            }),
        )?;
        let rev = format!("{change}.l");
        by_commit.insert(*id, rev.clone());
        report.commits += 1;
        // Weft's copy must give git's tree: every 100 commits and at the end.
        if report.commits.is_multiple_of(100) || n + 1 == line.len() {
            check_tree(store, &rev, &c.tree)?;
            report.checked += 1;
        }
        prev_tree = Some(c.tree);
    }
    // Tags on imported commits become imported releases.
    for (name, id) in source.refs()? {
        let Some(tag) = name.strip_prefix("refs/tags/") else {
            continue;
        };
        let (target, notes) = peel(source, &id)?;
        let Some(rev) = by_commit.get(&target) else {
            continue;
        };
        if store.state().repo(repo)?.releases.contains_key(tag) {
            continue;
        }
        store.hub_append("release.propose", json!({ "repo": repo, "version": tag, "from": rev, "notes": notes, "imported": git::hex(&id) }))?;
        report.releases.push(tag.to_string());
    }
    report.head = Some(git::hex(&head));
    Ok(report)
}

/// A tag's commit and its message (a lightweight tag has none).
fn peel(source: &Source, id: &Oid) -> Result<(Oid, String), String> {
    let (k, d) = source.read(id)?;
    if k != Kind::Tag {
        return Ok((*id, String::new()));
    }
    let text = String::from_utf8_lossy(&d).to_string();
    let object = text
        .lines()
        .find_map(|l| l.strip_prefix("object "))
        .ok_or("a tag without an object")?;
    let message = text.split_once("\n\n").map(|(_, m)| m).unwrap_or("");
    let message = message
        .split("-----BEGIN")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let (target, _) = peel(source, &git::parse_oid(object)?)?;
    Ok((target, message))
}

/// Weft's tree at `rev`, as git would hash it, against git's tree id.
pub fn check_tree(store: &Store, rev: &str, want: &Oid) -> Result<(), String> {
    let got = git_tree_id(store, &*store.tree(rev)?)?;
    if got != *want {
        return Err(format!(
            "{rev}: Weft's tree hashes to {} but git's is {}",
            git::hex(&got),
            git::hex(want)
        ));
    }
    Ok(())
}

/// A Weft tree's git tree id (no objects written).
pub fn git_tree_id(store: &Store, tree: &Tree) -> Result<Oid, String> {
    fn build(
        store: &Store,
        entries: &[(&str, &Entry)],
        ids: &mut HashMap<Hash, Oid>,
    ) -> Result<Oid, String> {
        let mut out: Vec<git::TreeEntry> = Vec::new();
        let mut i = 0;
        while i < entries.len() {
            let (path, e) = entries[i];
            match path.split_once('/') {
                None => {
                    let id = match ids.get(&e.hash) {
                        Some(id) => *id,
                        None => {
                            let id = git::oid(Kind::Blob, &store.blobs.get(&e.hash)?);
                            ids.insert(e.hash, id);
                            id
                        }
                    };
                    out.push(git::TreeEntry {
                        mode: e.mode,
                        name: path.to_string(),
                        id,
                    });
                    i += 1;
                }
                Some((dir, _)) => {
                    let mut j = i;
                    let mut sub = Vec::new();
                    while j < entries.len()
                        && entries[j].0.split_once('/').is_some_and(|(d, _)| d == dir)
                    {
                        sub.push((entries[j].0.split_once('/').unwrap().1, entries[j].1));
                        j += 1;
                    }
                    let id = build(store, &sub, ids)?;
                    out.push(git::TreeEntry {
                        mode: 0o40000,
                        name: dir.to_string(),
                        id,
                    });
                    i = j;
                }
            }
        }
        Ok(git::oid(Kind::Tree, &git::write_tree(&out)))
    }
    // Group by first segment: sort so that a directory's paths are together.
    let mut entries: Vec<(&str, &Entry)> = tree.iter().map(|(p, e)| (p.as_str(), e)).collect();
    entries.sort_by(|a, b| {
        let ka = a.0.split_once('/').map(|(d, _)| d).unwrap_or(a.0);
        let kb = b.0.split_once('/').map(|(d, _)| d).unwrap_or(b.0);
        ka.cmp(kb).then(a.0.cmp(b.0))
    });
    build(store, &entries, &mut HashMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_trailers_are_found() {
        let m = "Fix nodes\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\nCo-authored-by: Pat <pat@example.com>\n";
        assert_eq!(
            ai_coauthors(m),
            vec!["Claude Opus 5.5 <noreply@anthropic.com>".to_string()]
        );
    }
}

#[derive(Debug, Serialize)]
pub struct AdoptReport {
    pub change: String,
    pub remote_head: String,
    pub commits: Vec<String>,
    pub files: usize,
}

/// Adopts the commits a remote received on its own: their difference from what Weft last exported becomes a lane
/// (reviewed and landed like any change); the export joins the remote's head once it lands.
pub fn adopt(
    store: &Store,
    repo: &str,
    connection: &str,
    token: Option<String>,
) -> Result<AdoptReport, String> {
    let conn = store
        .state()
        .repo(repo)?
        .connections
        .get(connection)
        .cloned()
        .ok_or_else(|| format!("no connection {connection}"))?;
    let ours_hex = conn
        .head
        .clone()
        .ok_or("nothing was exported yet; import the remote instead")?;
    let ours = git::parse_oid(&ours_hex)?;
    let source = Source::open(&conn.conn.remote, token, &conn.conn.branch)?;
    let theirs = source.resolve(&conn.conn.branch)?;
    if theirs == ours {
        return Err("the remote has nothing of its own to adopt".into());
    }
    // The remote's own commits, newest first, back to what Weft pushed.
    let mut commits = Vec::new();
    let mut at = Some(theirs);
    while let Some(id) = at {
        if id == ours {
            break;
        }
        let c = git::parse_commit(&source.read(&id)?.1)?;
        commits.push(c.message.lines().next().unwrap_or("").to_string());
        at = c.parents.first().copied();
        if at.is_none() {
            return Err(
                "the remote's branch does not contain what Weft pushed; it was rewritten".into(),
            );
        }
    }
    let ours_tree = git::parse_commit(&source.read(&ours)?.1)?.tree;
    let theirs_tree = git::parse_commit(&source.read(&theirs)?.1)?.tree;
    let mut deltas = Vec::new();
    for (path, entry) in source.diff(Some(&ours_tree), &theirs_tree)? {
        match entry {
            Some((mode, blob)) => {
                let h = store.blobs.put(&source.read(&blob)?.1)?;
                deltas.push(Delta {
                    path,
                    after: Some(h),
                    mode: if mode == 0o100755 || mode == 0o120000 {
                        mode
                    } else {
                        0o100644
                    },
                });
            }
            None => deltas.push(Delta {
                path,
                after: None,
                mode: 0o100644,
            }),
        }
    }
    // The lane starts from the landed revision Weft last exported.
    let base = {
        let st = store.state();
        st.changes
            .values()
            .filter(|c| {
                c.repo == repo
                    && c.exported
                        .get(connection)
                        .and_then(|v| v.last())
                        .is_some_and(|e| e.git == ours_hex)
            })
            .filter_map(|c| c.landed.clone())
            .next()
            .ok_or("cannot find the change Weft last exported")?
    };
    let change = store.next_change();
    commits.reverse();
    let title = format!(
        "Adopt {} commit{} from {}",
        commits.len(),
        if commits.len() == 1 { "" } else { "s" },
        conn.conn.remote
    );
    store.hub_append("lane.open", json!({ "repo": repo, "change": change, "ref": "main", "title": title, "author": crate::record::Actor::Owner, "base": base, "kind": "adopt" }))?;
    store.hub_append(
        "revision.draft",
        json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["weft:adopt"] }),
    )?;
    store.hub_append(
        "revision.seal",
        json!({ "change": change, "n": 1, "deltas": deltas }),
    )?;
    let intents: Vec<serde_json::Value> = commits.iter().map(|m| json!({ "title": m, "kind": "adopted", "covers": deltas.iter().map(|d| d.path.clone()).collect::<Vec<_>>() })).take(1).collect();
    store.hub_append(
        "intent.set",
        json!({ "change": change, "n": 1, "intents": intents }),
    )?;
    store.hub_append("connection.state", json!({ "repo": repo, "id": connection, "state": "adopting", "detail": format!("{change} adopts the remote's {} commits", commits.len()), "remote_head": git::hex(&theirs), "adopt_change": change }))?;
    Ok(AdoptReport {
        change,
        remote_head: git::hex(&theirs),
        commits,
        files: deltas.len(),
    })
}
