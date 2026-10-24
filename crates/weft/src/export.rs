//! The one-way export to git: after a landing, one commit per intent, on top of what the connection received last.
//!
//! - The author (and committer) is the owner, as the git platform's authorization knows them (W19). The message is
//!   the intent's title and summary. No agent names, no credit lines, no notes (W13).
//! - Commits are signed with the hub's SSH signing key, so a platform that has it registered shows them verified.
//! - Fast-forward only: a remote that moved on its own stops the export and marks the connection `moved`; nothing is
//!   forced. Paths whose rule says "not exported" are left out.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;

use crate::derived::{Derived, covers};
use crate::git::{self, Commit, Kind, Oid, Person, TreeEntry};
use crate::gitwire::{self, Remote, Update};
use crate::hash::Hash;
use crate::keys::KeyPair;
use crate::model::*;
use crate::record::LogRecord;
use crate::state::{ChangeState, State};
use crate::store::{Store, apply_deltas};

/// The git ref that carries the hub's review records beside the exported branch. A note is keyed by the commit a
/// change was exported as (its last), and holds a JSON array of that change's reviews: each one the hub's signed
/// `review.outcome`, the certificates that prove its key and the owner's key. Import and adoption read it back
/// (`import::carry_notes`), so the review travels with the history it reviewed.
///
/// This takes PLAN 8.4's proposal (review records as Weft statements, the hub's tables as the working copy). PLAN
/// 28.1 point 5 is still open: the hub's tables do not need these statements to work, so the choice can be reverted.
pub const REVIEW_NOTES: &str = "refs/notes/weft-reviews";

#[derive(Debug, Serialize)]
pub struct ExportedChange {
    pub change: String,
    pub commits: Vec<CommitView>,
}

#[derive(Debug, Serialize)]
pub struct CommitView {
    pub intent: String,
    pub git: String,
    pub files: usize,
}

#[derive(Debug, Serialize)]
pub struct ExportReport {
    pub repo: String,
    pub connection: String,
    pub remote: String,
    pub branch: String,
    /// `pushed`, `in-step` (nothing new), `moved` (the remote moved on its own), `dry-run`.
    pub state: String,
    pub detail: String,
    pub head: Option<String>,
    pub changes: Vec<ExportedChange>,
    pub objects: usize,
}

/// What a lane's export did: the branch it pushed to, the commit the branch is at, and the state (`pushed`, `in-step`
/// when the lane's tree is what the branch already holds, `moved` when the remote's main is not where Weft last left
/// it, `dry-run`).
#[derive(Debug, Serialize)]
pub struct LaneExport {
    pub repo: String,
    pub change: String,
    pub branch: String,
    pub state: String,
    pub detail: String,
    /// The branch's commit on the remote (git), after the export.
    pub head: Option<String>,
    /// The remote's main (git), as the export found it.
    pub main: Option<String>,
    pub remote: String,
}

/// Remembers git blob ids by Weft hash, so a file is hashed for git once; and the tree each lane branch last got.
#[derive(Default)]
pub struct Exporter {
    blob_ids: Mutex<HashMap<Hash, Oid>>,
    lane_trees: Mutex<HashMap<String, Hash>>,
}

/// What an export needs that the store does not hold: the remote's credential and the signing key.
pub struct Keys<'a> {
    pub token: Option<String>,
    pub signing: Option<&'a KeyPair>,
}

impl Exporter {
    fn blob_id(&self, store: &Store, h: &Hash) -> Result<(Oid, Option<Vec<u8>>), String> {
        if let Some(id) = self
            .blob_ids
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(h)
        {
            return Ok((*id, None));
        }
        let bytes = store.blobs.get(h)?;
        let id = git::oid(Kind::Blob, &bytes);
        self.blob_ids
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(*h, id);
        Ok((id, Some(bytes)))
    }

    /// The git tree of a Weft tree, and every object in it (`id → (kind, bytes)`; blobs only when not already known).
    fn git_tree(
        &self,
        store: &Store,
        tree: &Tree,
        keep: &dyn Fn(&str) -> bool,
        known: &HashSet<Oid>,
        new: &mut Vec<(Oid, Kind, Vec<u8>)>,
        all: &mut HashSet<Oid>,
    ) -> Result<Oid, String> {
        // Directory → its entries (files and subdirectory names).
        let mut dirs: BTreeMap<String, Vec<(String, u32, Option<Hash>)>> = BTreeMap::new();
        dirs.entry(String::new()).or_default();
        for (path, e) in tree.iter().filter(|(p, _)| keep(p)) {
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
            dirs.entry(dir.to_string())
                .or_default()
                .push((name.to_string(), e.mode, Some(e.hash)));
            // Make sure each ancestor lists its child directory.
            let mut d = dir.to_string();
            while !d.is_empty() {
                let (parent, child) = d
                    .rsplit_once('/')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .unwrap_or((String::new(), d.clone()));
                let entries = dirs.entry(parent.clone()).or_default();
                if !entries.iter().any(|(n, m, _)| *n == child && *m == 0o40000) {
                    entries.push((child, 0o40000, None));
                }
                d = parent;
            }
        }
        // Deepest first, so each subtree's id is known before its parent's.
        let mut order: Vec<String> = dirs.keys().cloned().collect();
        order.sort_by_key(|d| {
            std::cmp::Reverse(if d.is_empty() {
                0
            } else {
                d.matches('/').count() + 1
            })
        });
        let mut ids: HashMap<String, Oid> = HashMap::new();
        for dir in order {
            let mut entries = Vec::new();
            for (name, mode, h) in &dirs[&dir] {
                let id = match h {
                    Some(h) => {
                        let (id, bytes) = self.blob_id(store, h)?;
                        if !known.contains(&id) && all.insert(id) {
                            let bytes = match bytes {
                                Some(b) => b,
                                None => store.blobs.get(h)?,
                            };
                            new.push((id, Kind::Blob, bytes));
                        }
                        id
                    }
                    None => {
                        ids[&if dir.is_empty() {
                            name.clone()
                        } else {
                            format!("{dir}/{name}")
                        }]
                    }
                };
                entries.push(TreeEntry {
                    mode: *mode,
                    name: name.clone(),
                    id,
                });
            }
            let data = git::write_tree(&entries);
            let id = git::oid(Kind::Tree, &data);
            if !known.contains(&id) && all.insert(id) {
                new.push((id, Kind::Tree, data));
            }
            ids.insert(dir, id);
        }
        Ok(ids[""])
    }

    /// Every object id in a tree (for what a remote already has).
    fn known_ids(
        &self,
        store: &Store,
        tree: &Tree,
        keep: &dyn Fn(&str) -> bool,
    ) -> Result<HashSet<Oid>, String> {
        let mut new = Vec::new();
        let mut all = HashSet::new();
        self.git_tree(store, tree, keep, &HashSet::new(), &mut new, &mut all)?;
        Ok(all)
    }

    /// Exports what has landed on the connection's branch since its last export.
    pub fn export(
        &self,
        store: &Store,
        derived: &Derived,
        repo: &str,
        connection: &str,
        keys: Keys,
        dry_run: bool,
    ) -> Result<ExportReport, String> {
        let (conn, _reference, chain, rules, done) = {
            let st = store.state();
            let r = st.repo(repo)?;
            let c = r
                .connections
                .get(connection)
                .ok_or_else(|| format!("{repo} has no connection {connection}"))?
                .clone();
            let reference = "main".to_string();
            let chain = r.refs.get(&reference).cloned().unwrap_or_default();
            let done: HashSet<u64> = c.exported.iter().map(|(id, _)| *id).collect();
            (c, reference, chain, r.rules.clone(), done)
        };
        let keep = |p: &str| {
            !rules
                .iter()
                .any(|r| !r.export && crate::derived::glob(&r.glob, p))
        };
        let remote =
            Remote::for_platform(&conn.conn.remote, &conn.conn.platform, keys.token.clone());
        let branch_ref = format!("refs/heads/{}", conn.conn.branch);
        let remote_refs = gitwire::list_refs(&remote, "git-receive-pack")?;
        let remote_head = remote_refs.get(&branch_ref).copied();
        let remote_notes = remote_refs.get(REVIEW_NOTES).copied();
        let mut report = ExportReport {
            repo: repo.into(),
            connection: connection.into(),
            remote: remote.describe(),
            branch: conn.conn.branch.clone(),
            state: String::new(),
            detail: String::new(),
            head: None,
            changes: vec![],
            objects: 0,
        };
        // Where the export starts: after what this connection received, or after the imported commit the remote is at.
        let ours = conn.head.as_deref().map(git::parse_oid).transpose()?;
        let after_done = chain
            .iter()
            .rposition(|l| landed_change(&store.state(), l).is_some_and(|c| done.contains(&c)))
            .map(|i| i + 1)
            .unwrap_or(0);
        let adopting = conn
            .adopting
            .clone()
            .and_then(|(h, c)| git::parse_oid(&h).ok().map(|h| (h, c)));
        let mut join: Option<(Oid, String)> = None;
        let (start, seeded) = match (ours, remote_head) {
            (Some(o), Some(r)) if o == r => (after_done, None),
            (Some(_), Some(r)) if adopting.as_ref().is_some_and(|(h, _)| *h == r) => {
                // The remote's own commits are being adopted: the export waits for the adopting change to land,
                // then joins the remote's head with a merge commit, so the push is still a fast-forward.
                let (h, change) = adopting.clone().unwrap();
                if store
                    .state()
                    .change(&change)
                    .map(|c| c.state != ChangeState::Landed)
                    .unwrap_or(true)
                {
                    report.state = "adopting".into();
                    report.detail =
                        format!("waiting for {change}, which adopts the remote's commits, to land");
                    return Ok(report);
                }
                join = Some((h, change));
                (after_done, None)
            }
            (Some(o), r) => {
                return self.moved(
                    store,
                    &mut report,
                    &format!(
                        "the remote's {} is at {} but Weft last pushed {}",
                        conn.conn.branch,
                        r.map(|r| git::hex(&r)).unwrap_or("nothing".into()),
                        git::hex(&o)
                    ),
                );
            }
            (None, None) => (0, None),
            (None, Some(r)) => {
                // The remote already has history: it must be an imported commit on our chain.
                let st = store.state();
                let at = chain.iter().position(|l| {
                    st.revisions
                        .get(l)
                        .and_then(|rev| st.changes.get(&rev.change))
                        .and_then(|c| c.import.as_ref())
                        .is_some_and(|i| i.git == git::hex(&r))
                });
                drop(st);
                match at {
                    Some(i) => (i + 1, Some((i, r))),
                    None => return self.moved(store, &mut report, &format!("the remote's {} ({}) has history Weft did not import; import or adopt it first", conn.conn.branch, git::hex(&r))),
                }
            }
        };
        let mut parent = remote_head;
        let mut cur: Tree = match start {
            0 => Tree::new(),
            i => (*store.tree(&chain[i - 1])?).clone(),
        };
        let mut known = if start == 0 {
            HashSet::new()
        } else {
            self.known_ids(store, &cur, &keep)?
        };
        let mut objects: Vec<(Oid, Kind, Vec<u8>)> = Vec::new();
        let mut per_change: Vec<(String, Vec<ExportedCommit>)> = Vec::new();
        for l in &chain[start..] {
            let (change, rev) = {
                let st = store.state();
                let rev = st
                    .revisions
                    .get(l)
                    .cloned()
                    .ok_or_else(|| format!("no revision {l}"))?;
                let c = st
                    .changes
                    .get(&rev.change)
                    .cloned()
                    .ok_or("revision without a change")?;
                (c, rev)
            };
            if change.import.is_some() {
                return self.moved(store, &mut report, &format!("{} is an imported commit the remote does not have; push the imported history first", change.name));
            }
            let intents = change
                .intents
                .get(&rev.n)
                .cloned()
                .or_else(|| change.intents.values().last().cloned())
                .unwrap_or_default();
            let groups = group(derived, store, &rev, &intents)?;
            let mut commits = Vec::new();
            let mut views = Vec::new();
            for (title, summary, deltas) in groups {
                let before = cur.clone();
                apply_deltas(&mut cur, &deltas);
                if deltas.iter().all(|d| !keep(&d.path)) || before == cur {
                    continue; // nothing git would see
                }
                let mut all = HashSet::new();
                let tree = self.git_tree(store, &cur, &keep, &known, &mut objects, &mut all)?;
                let who = Person {
                    name: conn.conn.author_name.clone(),
                    email: conn.conn.author_email.clone(),
                    time: (rev.at_ms / 1000) as i64,
                    tz: "+0000".into(),
                };
                let message = if summary.trim().is_empty() {
                    format!("{}\n", title.trim())
                } else {
                    format!("{}\n\n{}\n", title.trim(), summary.trim())
                };
                let mut parents: Vec<Oid> = parent.into_iter().collect();
                // The adopting change's first commit joins the remote's head.
                if let Some(r) = join
                    .as_ref()
                    .filter(|(_, adopt)| *adopt == change.name)
                    .map(|(r, _)| *r)
                {
                    if !parents.contains(&r) {
                        parents.push(r);
                    }
                    join = None;
                }
                let commit = Commit {
                    tree,
                    parents,
                    author: who.clone(),
                    committer: who,
                    extra: vec![],
                    message,
                };
                let unsigned = git::write_commit(&commit, None);
                let bytes = match keys.signing {
                    Some(k) => {
                        git::write_commit(&commit, Some(&crate::sshsig::sign(k, "git", &unsigned)))
                    }
                    None => unsigned,
                };
                let id = git::oid(Kind::Commit, &bytes);
                objects.push((id, Kind::Commit, bytes));
                known.extend(all);
                known.insert(id);
                parent = Some(id);
                commits.push(ExportedCommit {
                    intent: title.clone(),
                    git: git::hex(&id),
                });
                views.push(CommitView {
                    intent: title,
                    git: git::hex(&id),
                    files: deltas.len(),
                });
            }
            if !commits.is_empty() {
                per_change.push((change.name.clone(), commits));
                report.changes.push(ExportedChange {
                    change: change.name.clone(),
                    commits: views,
                });
            }
        }
        report.objects = objects.len();
        report.head = parent.map(|p| git::hex(&p));
        if dry_run {
            report.state = "dry-run".into();
            return Ok(report);
        }
        // The review records of the exported changes, as notes on the branch's new commits.
        let notes = if per_change.is_empty() {
            None
        } else {
            let who = Person {
                name: conn.conn.author_name.clone(),
                email: conn.conn.author_email.clone(),
                time: (crate::record::now_ms() / 1000) as i64,
                tz: "+0000".into(),
            };
            let fresh: Vec<(String, String)> = per_change
                .iter()
                .filter_map(|(name, commits)| Some((name.clone(), commits.last()?.git.clone())))
                .collect();
            self.review_notes(
                store,
                repo,
                connection,
                &fresh,
                remote_notes,
                &who,
                keys.signing,
            )?
        };
        // The hub's own copy of what it exported.
        if let Some((_, note_objects)) = &notes {
            objects.extend(note_objects.iter().cloned());
        }
        let mirror = git::Repo::init_bare(&store.dir.join("export").join(format!("{repo}.git")))?;
        for (_, k, d) in &objects {
            mirror.write(*k, d)?;
        }
        if let Some((i, r)) = seeded {
            // Record where the export began: the imported commit the remote is at.
            let st = store.state();
            let name = landed_change(&st, &chain[i])
                .and_then(|c| st.changes.get(&c))
                .map(|c| c.name.clone())
                .unwrap_or_default();
            drop(st);
            store.hub_append("export", json!({ "repo": repo, "connection": connection, "change": name, "commits": [{ "intent": "imported", "git": git::hex(&r) }], "head": git::hex(&r) }))?;
        }
        if per_change.is_empty() {
            report.state = "in-step".into();
            return Ok(report);
        }
        let new = parent.unwrap();
        let mut updates = vec![Update {
            name: branch_ref.clone(),
            old: remote_head,
            new,
        }];
        if let Some((notes_head, _)) = &notes {
            updates.push(Update {
                name: REVIEW_NOTES.into(),
                old: remote_notes,
                new: *notes_head,
            });
        }
        let send: Vec<(Kind, Vec<u8>)> = objects.into_iter().map(|(_, k, d)| (k, d)).collect();
        if let Err(e) = gitwire::push(&remote, &updates, &send) {
            store.hub_append(
                "connection.state",
                json!({ "repo": repo, "id": connection, "state": "error", "detail": e }),
            )?;
            return Err(e);
        }
        let _ = mirror.update_ref(&branch_ref, mirror.refs()?.get(&branch_ref).copied(), new);
        if let Some((notes_head, _)) = &notes {
            let _ = mirror.update_ref(
                REVIEW_NOTES,
                mirror.refs()?.get(REVIEW_NOTES).copied(),
                *notes_head,
            );
        }
        for (change, commits) in per_change {
            let head = commits.last().map(|c| c.git.clone()).unwrap_or_default();
            store.hub_append("export", json!({ "repo": repo, "connection": connection, "change": change, "commits": commits, "head": head }))?;
        }
        report.state = "pushed".into();
        Ok(report)
    }

    /// The notes that carry the reviews of the changes exported on `connection`: one commit on top of the remote's
    /// notes (a fast-forward), its tree of notes, and their objects. None when no exported change has a review.
    fn review_notes(
        &self,
        store: &Store,
        repo: &str,
        connection: &str,
        fresh: &[(String, String)],
        parent: Option<Oid>,
        who: &Person,
        signing: Option<&KeyPair>,
    ) -> Result<Option<(Oid, Vec<(Oid, Kind, Vec<u8>)>)>, String> {
        let records: HashMap<u64, LogRecord> =
            store.records()?.into_iter().map(|r| (r.seq, r)).collect();
        let st = store.state();
        let owner = st.owner.ok_or("the log has no genesis")?;
        let mut entries: Vec<TreeEntry> = Vec::new();
        let mut objects: Vec<(Oid, Kind, Vec<u8>)> = Vec::new();
        for c in st.changes.values().filter(|c| c.repo == repo) {
            if c.reviews.is_empty() {
                continue;
            }
            // The commit a change was exported as: this push's, or the one an earlier export recorded.
            let last = fresh
                .iter()
                .find(|(name, _)| *name == c.name)
                .map(|(_, git)| git.clone())
                .or_else(|| {
                    c.exported
                        .get(connection)
                        .and_then(|v| v.last())
                        .map(|e| e.git.clone())
                });
            let Some(last) = last else {
                continue;
            };
            let mut envelopes = Vec::new();
            for r in &c.reviews {
                let signed = &records
                    .get(&r.seq)
                    .ok_or_else(|| format!("review at record {} is not in the log", r.seq))?
                    .signed;
                let certs = cert_chain(&st, &r.key)?
                    .into_iter()
                    .map(|seq| {
                        records
                            .get(&seq)
                            .map(|rec| rec.signed.to_json())
                            .ok_or_else(|| format!("certificate at record {seq} is not in the log"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                envelopes.push(json!({ "change": c.name, "owner": owner, "certs": certs, "signed": signed.to_json() }));
            }
            let bytes = serde_json::to_vec(&envelopes).map_err(|e| e.to_string())?;
            let blob = git::oid(Kind::Blob, &bytes);
            objects.push((blob, Kind::Blob, bytes));
            entries.push(TreeEntry {
                mode: 0o100644,
                name: last,
                id: blob,
            });
        }
        drop(st);
        if entries.is_empty() {
            return Ok(None);
        }
        let tree_bytes = git::write_tree(&entries);
        let tree = git::oid(Kind::Tree, &tree_bytes);
        objects.push((tree, Kind::Tree, tree_bytes));
        let commit = Commit {
            tree,
            parents: parent.into_iter().collect(),
            author: who.clone(),
            committer: who.clone(),
            extra: vec![],
            message: "Review records\n".into(),
        };
        let unsigned = git::write_commit(&commit, None);
        let bytes = match signing {
            Some(k) => git::write_commit(&commit, Some(&crate::sshsig::sign(k, "git", &unsigned))),
            None => unsigned,
        };
        let id = git::oid(Kind::Commit, &bytes);
        objects.push((id, Kind::Commit, bytes));
        Ok(Some((id, objects)))
    }

    /// Pushes an open lane to `refs/heads/<branch>` on the connection's remote, for a platform to review (a pull request;
    /// `ci` mode, PLAN 21.5). The branch's tree is the repo's head with the lane's own deltas applied (its base's
    /// differences from its head revision), committed on the branch's last commit, or on the remote's main when the
    /// branch is new: always a fast-forward. The lane is not landed. Refused as `moved` when the remote's main is not
    /// the commit Weft last left there (its connection's head), so nothing is pushed over history Weft does not know.
    pub fn export_lane(
        &self,
        store: &Store,
        repo: &str,
        change: &str,
        connection: &str,
        branch: &str,
        keys: Keys,
        dry_run: bool,
    ) -> Result<LaneExport, String> {
        let (conn, main_head, rev, base, title, at_ms, rules) = {
            let st = store.state();
            let r = st.repo(repo)?;
            let c = r
                .connections
                .get(connection)
                .ok_or_else(|| format!("{repo} has no connection {connection}"))?
                .clone();
            let ch = st.change(change)?;
            let rev = ch
                .latest()
                .or(ch.drafts.last())
                .cloned()
                .ok_or_else(|| format!("{change} has no revision to export"))?;
            let at_ms = st.revisions.get(&rev).map(|v| v.at_ms).unwrap_or(0);
            (
                c,
                r.head("main"),
                rev,
                ch.base.clone(),
                ch.title.clone(),
                at_ms,
                r.rules.clone(),
            )
        };
        let keep = |p: &str| {
            !rules
                .iter()
                .any(|r| !r.export && crate::derived::glob(&r.glob, p))
        };
        let remote =
            Remote::for_platform(&conn.conn.remote, &conn.conn.platform, keys.token.clone());
        let refs = gitwire::list_refs(&remote, "git-receive-pack")?;
        let main_ref = format!("refs/heads/{}", conn.conn.branch);
        let branch_ref = format!("refs/heads/{branch}");
        let remote_main = refs.get(&main_ref).copied();
        let branch_head = refs.get(&branch_ref).copied();
        let mut report = LaneExport {
            repo: repo.into(),
            change: change.into(),
            branch: branch.into(),
            state: String::new(),
            detail: String::new(),
            head: branch_head.map(|h| git::hex(&h)),
            main: remote_main.map(|h| git::hex(&h)),
            remote: remote.describe(),
        };
        if let Some(known) = conn.head.as_deref()
            && remote_main.map(|h| git::hex(&h)).as_deref() != Some(known)
        {
            report.state = "moved".into();
            report.detail = format!(
                "the remote's {} is at {} but Weft last left it at {known}",
                conn.conn.branch,
                remote_main
                    .map(|h| git::hex(&h))
                    .unwrap_or("nothing".into())
            );
            return Ok(report);
        }
        // The lane's own deltas: its head revision against its base.
        let lane = (*store.tree(&rev)?).clone();
        let base_tree = (*store.tree(&base)?).clone();
        let mut deltas = Vec::new();
        for (path, e) in &lane {
            match base_tree.get(path) {
                Some(b) if b.hash == e.hash && b.mode == e.mode => {}
                _ => deltas.push(Delta {
                    path: path.clone(),
                    after: Some(e.hash),
                    mode: e.mode,
                }),
            }
        }
        for path in base_tree.keys().filter(|p| !lane.contains_key(*p)) {
            deltas.push(Delta {
                path: path.clone(),
                after: None,
                mode: 0o100644,
            });
        }
        let mut tree = (*store.tree(&main_head)?).clone();
        apply_deltas(&mut tree, &deltas);
        let tree_hash = crate::store::tree_hash(&tree);
        if self
            .lane_trees
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(branch)
            == Some(&tree_hash)
            && branch_head.is_some()
        {
            report.state = "in-step".into();
            return Ok(report);
        }
        let mut objects: Vec<(Oid, Kind, Vec<u8>)> = Vec::new();
        let mut all = HashSet::new();
        let root = self.git_tree(store, &tree, &keep, &HashSet::new(), &mut objects, &mut all)?;
        let who = Person {
            name: conn.conn.author_name.clone(),
            email: conn.conn.author_email.clone(),
            time: (at_ms / 1000) as i64,
            tz: "+0000".into(),
        };
        let commit = Commit {
            tree: root,
            parents: branch_head.or(remote_main).into_iter().collect(),
            author: who.clone(),
            committer: who,
            extra: vec![],
            message: format!("{}\n", title.trim()),
        };
        let unsigned = git::write_commit(&commit, None);
        let bytes = match keys.signing {
            Some(k) => git::write_commit(&commit, Some(&crate::sshsig::sign(k, "git", &unsigned))),
            None => unsigned,
        };
        let id = git::oid(Kind::Commit, &bytes);
        objects.push((id, Kind::Commit, bytes));
        report.head = Some(git::hex(&id));
        if dry_run {
            report.state = "dry-run".into();
            return Ok(report);
        }
        let send: Vec<(Kind, Vec<u8>)> = objects.into_iter().map(|(_, k, d)| (k, d)).collect();
        gitwire::push(
            &remote,
            &[Update {
                name: branch_ref,
                old: branch_head,
                new: id,
            }],
            &send,
        )?;
        self.lane_trees
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(branch.to_string(), tree_hash);
        report.state = "pushed".into();
        Ok(report)
    }

    fn moved(
        &self,
        store: &Store,
        report: &mut ExportReport,
        detail: &str,
    ) -> Result<ExportReport, String> {
        store.hub_append("connection.state", json!({ "repo": report.repo, "id": report.connection, "state": "moved", "detail": detail }))?;
        report.state = "moved".into();
        report.detail = detail.into();
        Ok(std::mem::replace(
            report,
            ExportReport {
                repo: String::new(),
                connection: String::new(),
                remote: String::new(),
                branch: String::new(),
                state: String::new(),
                detail: String::new(),
                head: None,
                changes: vec![],
                objects: 0,
            },
        ))
    }
}

fn landed_change(st: &State, rev: &str) -> Option<u64> {
    st.revisions.get(rev).map(|r| r.change).filter(|c| {
        st.changes
            .get(c)
            .is_some_and(|c| c.state == ChangeState::Landed)
    })
}

/// A landed revision's deltas, grouped by intent in the intents' order: each file goes to the first intent that
/// covers it (or one of its changed definitions); what no intent covers joins the last one. A change without
/// intents is one commit, titled by the change.
fn group(
    derived: &Derived,
    store: &Store,
    rev: &crate::state::Revision,
    intents: &[IntentDef],
) -> Result<Vec<(String, String, Vec<Delta>)>, String> {
    if intents.is_empty() {
        let title = store
            .state()
            .changes
            .get(&rev.change)
            .map(|c| c.title.clone())
            .unwrap_or_default();
        return Ok(vec![(title, String::new(), rev.deltas.clone())]);
    }
    let tree = derived.cached_tree(store, &rev.id).ok();
    let mut groups: Vec<(String, String, Vec<Delta>)> = intents
        .iter()
        .map(|i| (i.title.clone(), i.summary.clone(), vec![]))
        .collect();
    for d in &rev.deltas {
        let items: Vec<String> = tree
            .as_ref()
            .and_then(|t| t.files.iter().find(|f| f.path == d.path))
            .map(|f| f.symbols.iter().map(|s| s.id.clone()).collect())
            .unwrap_or_default();
        let at = intents
            .iter()
            .position(|i| {
                covers(i, &d.path, &d.path) || items.iter().any(|item| covers(i, &d.path, item))
            })
            .unwrap_or(intents.len() - 1);
        groups[at].2.push(d.clone());
    }
    Ok(groups.into_iter().filter(|g| !g.2.is_empty()).collect())
}

/// The git commit a landed revision was exported as, on a connection.
pub fn commit_of(st: &State, repo: &str, connection: &str, rev: &str) -> Option<String> {
    let r = st.repos.get(repo)?;
    let chain = r.refs.get("main")?;
    let at = chain.iter().position(|x| x == rev)?;
    // The last exported (or imported) change at or before `rev`.
    for l in chain[..=at].iter().rev() {
        let c = st.changes.get(&st.revisions.get(l)?.change)?;
        if let Some(commits) = c.exported.get(connection)
            && let Some(last) = commits.last()
        {
            return Some(last.git.clone());
        }
        if let Some(i) = &c.import {
            return Some(i.git.clone());
        }
    }
    None
}

/// The sequence numbers of the certificates that prove `key`, root first: the owner's certificate of the first key
/// below the owner, down to `key`'s own. The owner's key itself has none.
fn cert_chain(st: &State, key: &str) -> Result<Vec<u64>, String> {
    let owner = st.owner.ok_or("the log has no genesis")?;
    let mut seqs = Vec::new();
    let mut at = key.to_string();
    loop {
        let info = st
            .keys
            .get(&at)
            .ok_or_else(|| format!("unknown key {at}"))?;
        if info.key == owner {
            break;
        }
        seqs.push(info.issued_seq);
        at = info.issuer.clone();
    }
    seqs.reverse();
    Ok(seqs)
}
