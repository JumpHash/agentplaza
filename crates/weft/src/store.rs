//! The store: the log, the blobs and the state, kept consistent.
//!
//! `append` verifies a signed statement (its key is certified and not revoked, the signature matches), checks
//! it against the state, writes it to the log, and applies it. Reads go to the state under a read lock. Trees
//! are computed from revisions' deltas on demand and cached.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, mpsc};

use serde_json::{Value, json};

use crate::blobs::Blobs;
use crate::hash::Hash;
use crate::keys::{KeyPair, PublicKey};
use crate::log::Log;
use crate::model::*;
use crate::record::{Actor, LogRecord, Signed, Statement};
use crate::state::State;

/// Something that happened, for listeners (the server's event stream).
#[derive(Clone, Debug)]
pub struct Event {
    pub seq: u64,
    pub kind: String,
    pub summary: String,
    pub body: Value,
}

pub struct Store {
    pub dir: PathBuf,
    log: Mutex<Log>,
    pub blobs: Blobs,
    state: RwLock<State>,
    trees: Mutex<HashMap<RevId, Arc<Tree>>>,
    hub: Option<KeyPair>,
    listeners: Mutex<Vec<mpsc::Sender<Event>>>,
}

impl Store {
    /// Opens the store in `dir` (`log/`, `blobs/`), replaying the log. `hub` is the hub's signing key, held
    /// in memory only; without it the store can be read and verified but signs nothing itself.
    pub fn open(dir: &Path, hub: Option<KeyPair>) -> Result<Store, String> {
        let opened = Log::open(&dir.join("log"))?;
        let blobs = Blobs::open(&dir.join("blobs"))?;
        let mut state = State::default();
        for rec in &opened.records {
            let stmt = rec.signed.statement()?;
            state.apply(rec, &stmt, false).map_err(|e| {
                format!("record {} ({}) no longer applies: {e}", rec.seq, stmt.kind)
            })?;
        }
        Ok(Store {
            dir: dir.to_path_buf(),
            log: Mutex::new(opened.log),
            blobs,
            state: RwLock::new(state),
            trees: Mutex::new(HashMap::new()),
            hub,
            listeners: Mutex::new(vec![]),
        })
    }

    /// Starts a new plaza's history: the owner's genesis, then the hub key's certificate with the capabilities
    /// the hub needs.
    pub fn init(
        dir: &Path,
        plaza: &str,
        owner: &KeyPair,
        hub: KeyPair,
        hub_caps: &[&str],
    ) -> Result<Store, String> {
        if dir.join("log").exists()
            && std::fs::read_dir(dir.join("log"))
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
        {
            return Err(format!("{} already holds a history", dir.display()));
        }
        let hub_pub = hub.public();
        let store = Store::open(dir, Some(hub))?;
        store.append(
            Statement::new(
                "genesis",
                Actor::Owner,
                json!({ "owner": owner.public(), "plaza": plaza }),
            )
            .sign(owner),
        )?;
        store.append(
            Statement::new(
                "cert.issue",
                Actor::Owner,
                json!({ "key": hub_pub, "holder": { "actor": { "kind": "hub" }, "name": "hub" }, "caps": hub_caps }),
            )
            .sign(owner),
        )?;
        // Every store reads Markdown from the start: documents, briefs and notes are prose.
        let md = crate::packsrc::prepare_bundled("markdown", None)?;
        store.blobs.put(&md.wasm)?;
        store.blobs.put(&md.query)?;
        store.append(
            Statement::new(
                "pack.add",
                Actor::Owner,
                serde_json::to_value(&md.pack).map_err(|e| e.to_string())?,
            )
            .sign(owner),
        )?;
        Ok(store)
    }

    pub fn state(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|p| p.into_inner())
    }

    /// The hub's key, for signing what ships beside the log (provenance envelopes).
    pub fn hub_key(&self) -> Option<&KeyPair> {
        self.hub.as_ref()
    }

    pub fn hub_public(&self) -> Option<PublicKey> {
        self.hub.as_ref().map(|k| k.public())
    }

    /// Verifies, checks, writes and applies one signed statement. Returns the record and its summary.
    pub fn append(&self, signed: Signed) -> Result<(LogRecord, String), String> {
        let stmt = signed.statement()?;
        let mut log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = self.state.write().unwrap_or_else(|p| p.into_inner());
        let seq = log.next_seq();
        // The signature: by the key the genesis names, or by a certified, unrevoked key.
        let key = if stmt.kind == "genesis" {
            let owner: PublicKey = serde_json::from_value(stmt.body["owner"].clone())
                .map_err(|e| format!("genesis: {e}"))?;
            owner
        } else {
            state.key_at(&signed.key_id, seq)?.key
        };
        if key.id() != signed.key_id {
            return Err("the key id does not name the signing key".into());
        }
        key.verify(&signed.stmt, &signed.sig)?;
        let missing = self.blobs.missing(&named_blobs(&stmt.kind, &stmt.body));
        if !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|h| h.short()).collect();
            return Err(format!(
                "missing blobs (upload them first): {}",
                list.join(", ")
            ));
        }
        let probe = LogRecord {
            seq,
            prev: log.head(),
            signed: signed.clone(),
        };
        let summary = state.apply(&probe, &stmt, true)?;
        let rec = log.append(signed)?;
        state
            .apply(&rec, &stmt, false)
            .map_err(|e| format!("record {} was written but did not apply: {e}", rec.seq))?;
        drop(state);
        drop(log);
        self.notify(Event {
            seq: rec.seq,
            kind: stmt.kind,
            summary: summary.clone(),
            body: stmt.body,
        });
        Ok((rec, summary))
    }

    /// Signs a statement with the hub key and appends it.
    pub fn hub_append(&self, kind: &str, body: Value) -> Result<(LogRecord, String), String> {
        let key = self.hub.as_ref().ok_or("this store holds no hub key")?;
        self.append(Statement::new(kind, Actor::Hub, body).sign(key))
    }

    /// Signs a statement as the owner, from the console, with the hub's `rule-for-owner` capability.
    pub fn console_owner_append(
        &self,
        kind: &str,
        body: Value,
    ) -> Result<(LogRecord, String), String> {
        let key = self.hub.as_ref().ok_or("this store holds no hub key")?;
        self.append(Statement::new(kind, Actor::Owner, body).sign(key))
    }

    /// The next change's name, for a lane the hub is about to open.
    pub fn next_change(&self) -> String {
        change_name(self.state().last_change + 1)
    }

    /// Opens a lane for a task: on the ref's head, or stacked on another change's latest revision. Returns the
    /// change's name.
    pub fn open_lane(
        &self,
        repo: &str,
        reference: &str,
        task: Option<&str>,
        title: &str,
        author: Actor,
        stacked_on: Option<&str>,
        kind: &str,
    ) -> Result<String, String> {
        // Allocate under the log lock's ordering: retry if another lane took the number.
        for _ in 0..4 {
            let (change, base) = {
                let st = self.state();
                let r = st.repo(repo)?;
                let base = match stacked_on {
                    Some(s) => {
                        let c = st.change(s)?;
                        c.latest()
                            .cloned()
                            .or_else(|| c.drafts.last().cloned())
                            .unwrap_or_else(|| c.base.clone())
                    }
                    None => r.head(reference),
                };
                (change_name(st.last_change + 1), base)
            };
            let body = json!({
                "repo": repo, "change": change, "ref": reference, "task": task, "title": title,
                "author": author, "stacked_on": stacked_on, "base": base, "kind": kind,
            });
            match self.hub_append("lane.open", body) {
                Ok(_) => return Ok(change),
                Err(e) if e.contains("the next change is") => continue,
                Err(e) => return Err(e),
            }
        }
        Err("could not allocate a change".into())
    }

    /// The full tree of a revision (`root` is empty).
    pub fn tree(&self, rev: &str) -> Result<Arc<Tree>, String> {
        if rev == "root" {
            return Ok(Arc::new(Tree::new()));
        }
        if let Some(t) = self
            .trees
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(rev)
        {
            return Ok(t.clone());
        }
        // Walk back to a cached tree or the root, then apply forward.
        let mut chain = Vec::new();
        let mut at = rev.to_string();
        let start: Arc<Tree> = loop {
            if at == "root" {
                break Arc::new(Tree::new());
            }
            if let Some(t) = self
                .trees
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&at)
            {
                break t.clone();
            }
            let r = self
                .state()
                .revisions
                .get(&at)
                .cloned()
                .ok_or_else(|| format!("no revision {at}"))?;
            at = r.base.clone();
            chain.push(r);
        };
        // One working tree, moved forward through the chain. Cache the landed revisions on the
        // way (the spine every lane starts from); the one asked for goes in at the end.
        let mut tree = (*start).clone();
        for r in chain.iter().rev() {
            apply_deltas(&mut tree, &r.deltas);
            if r.id.ends_with(".l") && r.id != rev {
                self.trees
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(r.id.clone(), Arc::new(tree.clone()));
            }
        }
        let result = Arc::new(tree);
        if !chain.is_empty() {
            self.trees
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(rev.to_string(), result.clone());
        }
        Ok(result)
    }

    /// A file's bytes in a revision, if it is there.
    pub fn read(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        match self.tree(rev)?.get(path) {
            Some(e) => Ok(Some(self.blobs.get(&e.hash)?)),
            None => Ok(None),
        }
    }

    /// The deltas that turn `from` into `to` (both full trees).
    pub fn deltas_between(from: &Tree, to: &Tree) -> Vec<Delta> {
        let mut out = Vec::new();
        for (path, e) in to {
            if from.get(path) != Some(e) {
                out.push(Delta {
                    path: path.clone(),
                    after: Some(e.hash),
                    mode: e.mode,
                });
            }
        }
        for (path, e) in from {
            if !to.contains_key(path) {
                out.push(Delta {
                    path: path.clone(),
                    after: None,
                    mode: e.mode,
                });
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Every record in the log, read back from disk, for verification and the op log.
    pub fn records(&self) -> Result<Vec<LogRecord>, String> {
        let log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        let mut out = Vec::new();
        for seg in log.segments() {
            out.extend(crate::log::read_segment(&seg)?);
        }
        Ok(out)
    }

    pub fn log_size(&self) -> u64 {
        self.log.lock().unwrap_or_else(|p| p.into_inner()).size()
    }

    /// Listens to what happens from now on.
    pub fn subscribe(&self) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(tx);
        rx
    }

    fn notify(&self, event: Event) {
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Re-checks everything: the chain, every signature against the key valid at its place, and that replaying
    /// gives the state the store holds.
    pub fn verify_deep(&self) -> Result<VerifyReport, String> {
        let records = self.records()?;
        let mut replay = State::default();
        let mut prev = Hash::ZERO;
        let mut report = VerifyReport::default();
        for rec in &records {
            if rec.prev != prev {
                return Err(format!(
                    "record {} does not follow the one before it",
                    rec.seq
                ));
            }
            prev = rec.hash();
            let stmt = rec.signed.statement()?;
            let key = if stmt.kind == "genesis" {
                serde_json::from_value::<PublicKey>(stmt.body["owner"].clone())
                    .map_err(|e| e.to_string())?
            } else {
                replay
                    .key_at(&rec.signed.key_id, rec.seq)
                    .map_err(|e| format!("record {}: {e}", rec.seq))?
                    .key
            };
            key.verify(&rec.signed.stmt, &rec.signed.sig)
                .map_err(|e| format!("record {}: {e}", rec.seq))?;
            replay
                .apply(rec, &stmt, false)
                .map_err(|e| format!("record {}: {e}", rec.seq))?;
            report.records += 1;
            report.signatures += 1;
        }
        let live = self.state();
        report.changes = live.changes.len();
        if replay.changes.len() != live.changes.len() || replay.ops.len() != live.ops.len() {
            return Err("replaying the log does not give the state the store holds".into());
        }
        report.blobs = self.blobs.stats().0;
        Ok(report)
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct VerifyReport {
    pub records: usize,
    pub signatures: usize,
    pub changes: usize,
    pub blobs: usize,
}

/// The blobs a statement names, which must be stored before it is accepted.
pub fn named_blobs(kind: &str, body: &Value) -> Vec<Hash> {
    let hash = |v: &Value| v.as_str().and_then(|s| s.parse::<Hash>().ok());
    let mut out = Vec::new();
    match kind {
        "revision.draft" | "revision.seal" | "land" | "import" => {
            for d in body["deltas"].as_array().into_iter().flatten() {
                out.extend(hash(&d["after"]));
            }
        }
        "pack.add" => out.extend(
            [hash(&body["wasm"]), hash(&body["tags"])]
                .into_iter()
                .flatten(),
        ),
        "artifact.put" => out.extend(hash(&body["digest"])),
        _ => {}
    }
    out
}

pub fn apply_deltas(tree: &mut Tree, deltas: &[Delta]) {
    for d in deltas {
        match d.after {
            Some(h) => {
                tree.insert(
                    d.path.clone(),
                    Entry {
                        hash: h,
                        mode: d.mode,
                    },
                );
            }
            None => {
                tree.remove(&d.path);
            }
        }
    }
}

/// A tree's hash: what a client asks for when it wants a whole checkout.
pub fn tree_hash(tree: &Tree) -> Hash {
    let mut hasher = blake3::Hasher::new();
    for (path, e) in tree {
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
        hasher.update(&e.hash.0);
        hasher.update(&e.mode.to_le_bytes());
    }
    Hash(*hasher.finalize().as_bytes())
}
