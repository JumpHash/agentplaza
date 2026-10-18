//! Derived data: everything Weft can rebuild from the log and the blobs.
//!
//! - symbols per blob (through the file's pack), cached in memory and on disk under `derived/symbols/`;
//! - a name index per tree: where each name is defined and who refers to it (what a change touches);
//! - symbol lineage: one id per definition across renames and moves;
//! - the change tree of a revision: files, symbols, provenance, touches, divergence and intents.
//!
//! Deleting `derived/` is always safe.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::diff;
use crate::hash::Hash;
use crate::lang::{self, Parsed, Runtime, Symbol};
use crate::model::*;
use crate::record::Actor;
use crate::state::{ChangeState, RevKind, Revision, State};
use crate::store::{Store, tree_hash};

/// Derived data's caches.
pub struct Derived {
    pub runtime: Runtime,
    dir: PathBuf,
    parsed: Mutex<HashMap<(String, Hash), Arc<Parsed>>>,
    names: Mutex<HashMap<Hash, Arc<NameIndex>>>,
    /// `path::qualified` → lineage id, for definitions that were renamed or moved.
    aliases: Mutex<HashMap<String, String>>,
}

/// Where names are defined and who refers to them, in one tree.
#[derive(Default)]
pub struct NameIndex {
    pub defs: HashMap<String, Vec<(String, String)>>,
    pub refs: HashMap<String, Vec<(String, String)>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChangeTree {
    pub revision: RevId,
    pub base: RevId,
    pub change: String,
    pub files: Vec<FileChange>,
    pub intents: Vec<IntentView>,
    pub unsorted: Vec<String>,
    pub divergence: Vec<Divergence>,
    pub languages: BTreeMap<String, usize>,
    pub add: usize,
    pub del: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileChange {
    pub path: String,
    /// `+` added, `~` modified, `-` deleted.
    pub op: char,
    pub pack: Option<String>,
    pub binary: bool,
    pub add: usize,
    pub del: usize,
    pub before: Option<Hash>,
    pub after: Option<Hash>,
    pub symbols: Vec<SymbolChange>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SymbolChange {
    /// `path::qualified`: what selectors and findings name.
    pub id: String,
    pub qualified: String,
    pub kind: String,
    /// `+`, `~`, `-`, or `>` (renamed or moved; `from` says from where).
    pub op: char,
    pub lineage: String,
    pub from: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub add: usize,
    pub del: usize,
    pub provenance: Vec<Provenance>,
    pub touches: Vec<Touch>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Provenance {
    pub start_line: usize,
    pub end_line: usize,
    pub by: Actor,
    pub model: Option<String>,
    pub turn: Option<u64>,
    pub tool_calls: Vec<String>,
    /// The draft (or `seal`, `merge`, a git commit) that wrote these lines.
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Touch {
    /// `calls`, `uses`, `called by`, `used by`.
    pub how: String,
    pub to: String,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct IntentView {
    pub title: String,
    pub kind: String,
    pub summary: String,
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Divergence {
    pub change: String,
    pub task: Option<String>,
    pub files: Vec<String>,
    pub symbols: Vec<String>,
}

const MAX_TOUCHES: usize = 6;

impl Derived {
    pub fn new(dir: PathBuf) -> Derived {
        Derived {
            runtime: Runtime::new(),
            dir,
            parsed: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            aliases: Mutex::new(HashMap::new()),
        }
    }

    /// The symbols and references in a blob at `path`, through its pack (none without one).
    pub fn parse(&self, store: &Store, path: &str, hash: &Hash) -> Arc<Parsed> {
        let bytes = match store.blobs.get(hash) {
            Ok(b) => b,
            Err(_) => return Arc::new(Parsed::default()),
        };
        self.parse_bytes(store, path, &bytes, Some(*hash))
    }

    pub fn parse_bytes(
        &self,
        store: &Store,
        path: &str,
        bytes: &[u8],
        hash: Option<Hash>,
    ) -> Arc<Parsed> {
        if !diff::is_text(bytes) {
            return Arc::new(Parsed::default());
        }
        let first = std::str::from_utf8(bytes)
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        let pack = {
            let st = store.state();
            match lang::pack_for(&st.packs, path, &first) {
                Some(p) => p.clone(),
                None => return Arc::new(Parsed::default()),
            }
        };
        let hash = hash.unwrap_or_else(|| Hash::of(bytes));
        let key = (format!("{}@{}", pack.name, pack.version), hash);
        if let Some(p) = self
            .parsed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
        {
            return p.clone();
        }
        let disk = self
            .dir
            .join("symbols")
            .join(&key.0)
            .join(format!("{}.json", hash.hex()));
        let parsed = match std::fs::read(&disk)
            .ok()
            .and_then(|b| serde_json::from_slice::<CachedParse>(&b).ok())
        {
            Some(c) => Arc::new(c.into_parsed(&pack.name)),
            None => {
                let blob = |h: &Hash| store.blobs.get(h);
                match self.runtime.parse(&pack, &blob, bytes) {
                    Ok(p) => {
                        if let Some(dir) = disk.parent() {
                            let _ = std::fs::create_dir_all(dir);
                            let _ = std::fs::write(
                                &disk,
                                serde_json::to_vec(&CachedParse::from(&p)).unwrap_or_default(),
                            );
                        }
                        Arc::new(p)
                    }
                    Err(e) => {
                        eprintln!("weft: {path}: {e}");
                        Arc::new(Parsed::default())
                    }
                }
            }
        };
        let mut cache = self.parsed.lock().unwrap_or_else(|p| p.into_inner());
        if cache.len() > 20_000 {
            cache.clear();
        }
        cache.insert(key, parsed.clone());
        parsed
    }

    /// The name index of a revision's tree.
    pub fn names(&self, store: &Store, rev: &str) -> Result<Arc<NameIndex>, String> {
        let tree = store.tree(rev)?;
        let th = tree_hash(&tree);
        if let Some(n) = self
            .names
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&th)
        {
            return Ok(n.clone());
        }
        let mut index = NameIndex::default();
        for (path, entry) in tree.iter() {
            let parsed = self.parse(store, path, &entry.hash);
            for s in &parsed.symbols {
                index
                    .defs
                    .entry(s.name.clone())
                    .or_default()
                    .push((path.clone(), s.qualified.clone()));
            }
            for r in &parsed.references {
                let at = lang::symbol_at(&parsed.symbols, r.line)
                    .map(|i| parsed.symbols[i].qualified.clone())
                    .unwrap_or_else(|| "(top level)".into());
                index
                    .refs
                    .entry(r.name.clone())
                    .or_default()
                    .push((path.clone(), at));
            }
        }
        let index = Arc::new(index);
        self.names
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(th, index.clone());
        Ok(index)
    }

    /// A definition's lineage id.
    pub fn lineage(&self, repo: &str, id: &str) -> String {
        if let Some(l) = self
            .aliases
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
        {
            return l.clone();
        }
        format!(
            "s:{}",
            &Hash::of_parts(&[repo.as_bytes(), id.as_bytes()]).hex()[..16]
        )
    }

    /// Records that `new_id` continues `old_id` (a rename or a move that landed).
    pub fn alias(&self, repo: &str, new_id: &str, old_id: &str) {
        let lineage = self.lineage(repo, old_id);
        self.aliases
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(new_id.to_string(), lineage);
    }

    /// The change tree of a revision.
    pub fn change_tree(&self, store: &Store, rev_id: &str) -> Result<ChangeTree, String> {
        let (rev, change, intents, repo, others) = {
            let st = store.state();
            let rev = st
                .revisions
                .get(rev_id)
                .cloned()
                .ok_or_else(|| format!("no revision {rev_id}"))?;
            let change = st
                .changes
                .get(&rev.change)
                .cloned()
                .ok_or("revision without a change")?;
            let intents = change
                .intents
                .get(&rev.n)
                .cloned()
                .or_else(|| change.intents.values().last().cloned())
                .unwrap_or_default();
            let others = open_others(&st, &change.repo, change.id);
            (rev, change.clone(), intents, change.repo.clone(), others)
        };
        let base = store.tree(&rev.base)?;
        let after = store.tree(rev_id)?;
        let deltas = Store::deltas_between(&base, &after);
        let names = self.names(store, rev_id).ok();
        let changed_ids: BTreeSet<String> = BTreeSet::new();
        let mut files = Vec::new();
        let mut languages: BTreeMap<String, usize> = BTreeMap::new();
        let mut removed: Vec<(String, Symbol)> = Vec::new();
        let mut added: Vec<(usize, usize)> = Vec::new();
        for d in &deltas {
            let before = base.get(&d.path).map(|e| e.hash);
            let old_bytes = match before {
                Some(h) => store.blobs.get(&h)?,
                None => vec![],
            };
            let new_bytes = match d.after {
                Some(h) => store.blobs.get(&h)?,
                None => vec![],
            };
            let op = match (before.is_some(), d.after.is_some()) {
                (false, true) => '+',
                (true, false) => '-',
                _ => '~',
            };
            let text = diff::is_text(&old_bytes) && diff::is_text(&new_bytes);
            let old_parsed = before
                .map(|h| self.parse(store, &d.path, &h))
                .unwrap_or_default();
            let new_parsed = d
                .after
                .map(|h| self.parse(store, &d.path, &h))
                .unwrap_or_default();
            let pack = new_parsed.pack.clone().or_else(|| old_parsed.pack.clone());
            *languages
                .entry(
                    pack.clone()
                        .unwrap_or_else(|| if text { "text".into() } else { "binary".into() }),
                )
                .or_default() += 1;
            let mut fc = FileChange {
                path: d.path.clone(),
                op,
                pack,
                binary: !text,
                add: 0,
                del: 0,
                before,
                after: d.after,
                symbols: vec![],
            };
            if text {
                let old = String::from_utf8_lossy(&old_bytes).to_string();
                let new = String::from_utf8_lossy(&new_bytes).to_string();
                let (a, r) = diff::counts(&old, &new);
                fc.add = a;
                fc.del = r;
                let (old_lines, new_lines) = diff::changed_lines(&old, &new);
                let prov = self.provenance(store, &change, &rev, &d.path, &new);
                fc.symbols = symbol_changes(
                    &repo,
                    self,
                    &d.path,
                    &old_parsed.symbols,
                    &new_parsed.symbols,
                    &old_lines,
                    &new_lines,
                    &prov,
                );
                for s in &old_parsed.symbols {
                    if fc
                        .symbols
                        .iter()
                        .any(|c| c.op == '-' && c.qualified == s.qualified)
                    {
                        removed.push((d.path.clone(), s.clone()));
                    }
                }
                for (i, c) in fc.symbols.iter().enumerate() {
                    if c.op == '+' {
                        added.push((files.len(), i));
                    }
                }
                // What each changed definition touches, by name.
                if let Some(names) = &names {
                    for sc in fc.symbols.iter_mut().filter(|s| s.op != '-') {
                        sc.touches = touches(names, &new_parsed, &d.path, sc, &changed_ids);
                    }
                }
            }
            files.push(fc);
        }
        // Renames and moves: an added definition with the text (or the shape) of a removed one.
        let mut renames = Vec::new();
        for (fi, si) in added {
            let f = &files[fi];
            let qualified = f.symbols[si].qualified.clone();
            let Some(parsed) = f.after.map(|h| self.parse(store, &f.path, &h)) else {
                continue;
            };
            let Some(sym) = parsed.symbols.iter().find(|s| s.qualified == qualified) else {
                continue;
            };
            // The same text, or the same shape when there is more to it than its name (a list item is all name).
            let shaped = sym.end_byte - sym.start_byte >= sym.name.len() + 24;
            let found = removed
                .iter()
                .find(|(_, o)| o.content_hash == sym.content_hash)
                .or_else(|| {
                    removed.iter().find(|(_, o)| {
                        shaped && o.shape_hash == sym.shape_hash && o.kind == sym.kind
                    })
                });
            if let Some((old_path, old)) = found {
                renames.push((fi, si, format!("{old_path}::{}", old.qualified)));
            }
        }
        let gone: BTreeSet<String> = renames.iter().map(|(_, _, old)| old.clone()).collect();
        for (fi, si, old_id) in renames {
            let s = &mut files[fi].symbols[si];
            s.op = '>';
            s.lineage = self.lineage(&repo, &old_id);
            s.from = Some(old_id);
        }
        for f in files.iter_mut() {
            f.symbols.retain(|c| !(c.op == '-' && gone.contains(&c.id)));
        }
        // Intents: each changed definition (or file without definitions) goes to the first intent that covers it.
        let mut views: Vec<IntentView> = intents
            .iter()
            .map(|i| IntentView {
                title: i.title.clone(),
                kind: i.kind.clone(),
                summary: i.summary.clone(),
                items: vec![],
            })
            .collect();
        let mut unsorted = Vec::new();
        for f in &files {
            let items: Vec<String> = if f.symbols.is_empty() {
                vec![f.path.clone()]
            } else {
                f.symbols.iter().map(|s| s.id.clone()).collect()
            };
            for item in items {
                match intents.iter().position(|i| covers(i, &f.path, &item)) {
                    Some(k) => views[k].items.push(item),
                    None => unsorted.push(item),
                }
            }
        }
        // Divergence: other open lanes in the repo that change the same files or definitions.
        let mut divergence = Vec::new();
        let mine: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
        for (name, task, other_rev) in others {
            let Ok(other_base) = store.tree(&other_rev.base) else {
                continue;
            };
            let Ok(other_after) = store.tree(&other_rev.id) else {
                continue;
            };
            let theirs = Store::deltas_between(&other_base, &other_after);
            let shared: Vec<String> = theirs
                .iter()
                .filter(|d| mine.contains(d.path.as_str()))
                .map(|d| d.path.clone())
                .collect();
            if shared.is_empty() {
                continue;
            }
            let mut symbols = Vec::new();
            for path in &shared {
                let ours: BTreeSet<String> = files
                    .iter()
                    .filter(|f| &f.path == path)
                    .flat_map(|f| f.symbols.iter().map(|s| s.qualified.clone()))
                    .collect();
                let (ob, nb) = (
                    other_base.get(path).map(|e| e.hash),
                    other_after.get(path).map(|e| e.hash),
                );
                let old = ob
                    .and_then(|h| store.blobs.get(&h).ok())
                    .unwrap_or_default();
                let new = nb
                    .and_then(|h| store.blobs.get(&h).ok())
                    .unwrap_or_default();
                let op = ob.map(|h| self.parse(store, path, &h)).unwrap_or_default();
                let np = nb.map(|h| self.parse(store, path, &h)).unwrap_or_default();
                let (ol, nl) = diff::changed_lines(
                    &String::from_utf8_lossy(&old),
                    &String::from_utf8_lossy(&new),
                );
                for s in symbol_changes(&repo, self, path, &op.symbols, &np.symbols, &ol, &nl, &[])
                {
                    if ours.contains(&s.qualified) {
                        symbols.push(format!("{path}::{}", s.qualified));
                    }
                }
            }
            divergence.push(Divergence {
                change: name,
                task,
                files: shared,
                symbols,
            });
        }
        let add = files.iter().map(|f| f.add).sum();
        let del = files.iter().map(|f| f.del).sum();
        Ok(ChangeTree {
            revision: rev_id.to_string(),
            base: rev.base.clone(),
            change: change.name.clone(),
            files,
            intents: views,
            unsorted,
            divergence,
            languages,
            add,
            del,
        })
    }

    /// Who wrote each line of `final_text`: walk the change's drafts in order and keep, for every line, the
    /// draft it first appeared in. Lines from the base are not reported.
    fn provenance(
        &self,
        store: &Store,
        change: &crate::state::Change,
        rev: &Revision,
        path: &str,
        final_text: &str,
    ) -> Vec<Option<Provenance>> {
        let read = |r: &str| -> String {
            store
                .read(r, path)
                .ok()
                .flatten()
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default()
        };
        let mut drafts: Vec<Revision> = {
            let st = store.state();
            change
                .drafts
                .iter()
                .filter_map(|d| st.revisions.get(d).cloned())
                .filter(|d| d.seq < rev.seq)
                .collect()
        };
        drafts.sort_by_key(|d| d.n);
        let base_text = read(&change.base);
        let mut owners: Vec<Option<Provenance>> = vec![None; base_text.lines().count()];
        let mut prev = base_text;
        let label = |r: &Revision, source: String| Provenance {
            start_line: 0,
            end_line: 0,
            by: r.actor.clone(),
            model: r.model.clone(),
            turn: r.turn,
            tool_calls: r.tool_calls.clone(),
            source,
        };
        for d in &drafts {
            let text = read(&d.id);
            let kept = diff::kept_from(&prev, &text);
            owners = kept
                .into_iter()
                .map(|k| match k {
                    Some(i) => owners.get(i).cloned().flatten(),
                    None => Some(label(d, d.id.clone())),
                })
                .collect();
            prev = text;
        }
        let last = match rev.kind {
            RevKind::Imported => {
                let info = change.import.clone();
                Some(Provenance {
                    start_line: 0,
                    end_line: 0,
                    by: Actor::Owner,
                    model: info.as_ref().and_then(|i| {
                        (!i.ai_coauthors.is_empty()).then(|| i.ai_coauthors.join(", "))
                    }),
                    turn: None,
                    tool_calls: vec![],
                    source: format!(
                        "git {}",
                        info.map(|i| i.git[..12.min(i.git.len())].to_string())
                            .unwrap_or_default()
                    ),
                })
            }
            RevKind::Landed => Some(Provenance {
                start_line: 0,
                end_line: 0,
                by: Actor::Hub,
                model: None,
                turn: None,
                tool_calls: vec![],
                source: "merge".into(),
            }),
            _ => Some(label(rev, rev.id.clone())),
        };
        let kept = diff::kept_from(&prev, final_text);
        kept.into_iter()
            .map(|k| match k {
                Some(i) => owners.get(i).cloned().flatten(),
                None => last.clone(),
            })
            .collect()
    }
}

/// The other open changes in a repo, with their latest revision.
fn open_others(st: &State, repo: &str, me: u64) -> Vec<(String, Option<String>, Revision)> {
    st.changes
        .values()
        .filter(|c| {
            c.repo == repo
                && c.id != me
                && matches!(
                    c.state,
                    ChangeState::Open
                        | ChangeState::Sealed
                        | ChangeState::Queued
                        | ChangeState::Conflict
                )
        })
        .filter_map(|c| {
            let latest = c.rounds.last().or_else(|| c.drafts.last())?;
            Some((
                c.name.clone(),
                c.task.clone(),
                st.revisions.get(latest)?.clone(),
            ))
        })
        .collect()
}

/// Whether intent `i` covers `item` (a path, or `path::qualified`).
pub fn covers(i: &IntentDef, path: &str, item: &str) -> bool {
    i.covers.iter().any(|sel| {
        if let Some((p, sym)) = sel.split_once("::") {
            glob(p, path)
                && item.split_once("::").is_some_and(|(_, q)| {
                    q == sym || q.ends_with(&format!(" {sym}")) || q.contains(&format!(" {sym} ›"))
                })
        } else {
            glob(sel, path)
        }
    })
}

/// `*` within a segment, `**` across segments.
pub fn glob(pattern: &str, path: &str) -> bool {
    fn rec(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) if p.get(1) == Some(&b'*') => {
                let rest = if p.get(2) == Some(&b'/') {
                    &p[3..]
                } else {
                    &p[2..]
                };
                (0..=s.len()).any(|i| rec(rest, &s[i..]))
            }
            (Some(b'*'), _) => (0..=s.len())
                .take_while(|i| *i == 0 || s[i - 1] != b'/')
                .any(|i| rec(&p[1..], &s[i..])),
            (Some(a), Some(b)) if a == b => rec(&p[1..], &s[1..]),
            _ => false,
        }
    }
    pattern == path || rec(pattern.as_bytes(), path.as_bytes())
}

#[allow(clippy::too_many_arguments)]
fn symbol_changes(
    repo: &str,
    derived: &Derived,
    path: &str,
    old: &[Symbol],
    new: &[Symbol],
    old_lines: &[usize],
    new_lines: &[usize],
    prov: &[Option<Provenance>],
) -> Vec<SymbolChange> {
    let mut out: BTreeMap<String, SymbolChange> = BTreeMap::new();
    let new_by_q: HashMap<&str, &Symbol> = new.iter().map(|s| (s.qualified.as_str(), s)).collect();
    let old_by_q: HashMap<&str, &Symbol> = old.iter().map(|s| (s.qualified.as_str(), s)).collect();
    let mut top_add = 0;
    let mut top_del = 0;
    let new_set: BTreeSet<usize> = new_lines.iter().copied().collect();
    let old_set: BTreeSet<usize> = old_lines.iter().copied().collect();
    for &line in new_lines {
        match owner(new, line, &new_set) {
            Some(i) => {
                let s = &new[i];
                let op = match old_by_q.get(s.qualified.as_str()) {
                    Some(o) if o.content_hash == s.content_hash => continue,
                    Some(_) => '~',
                    None => '+',
                };
                let id = format!("{path}::{}", s.qualified);
                let e = out
                    .entry(s.qualified.clone())
                    .or_insert_with(|| SymbolChange {
                        id: id.clone(),
                        qualified: s.qualified.clone(),
                        kind: s.kind.clone(),
                        op,
                        lineage: derived.lineage(repo, &id),
                        from: None,
                        start_line: s.start_line,
                        end_line: s.end_line,
                        add: 0,
                        del: 0,
                        provenance: ranges(prov, s.start_line, s.end_line),
                        touches: vec![],
                    });
                e.add += 1;
            }
            None => top_add += 1,
        }
    }
    for &line in old_lines {
        match owner(old, line, &old_set) {
            Some(i) => {
                let s = &old[i];
                if let Some(e) = out.get_mut(&s.qualified) {
                    e.del += 1;
                    continue;
                }
                match new_by_q.get(s.qualified.as_str()) {
                    Some(n) if n.content_hash == s.content_hash => {}
                    Some(n) => {
                        let id = format!("{path}::{}", n.qualified);
                        out.insert(
                            n.qualified.clone(),
                            SymbolChange {
                                id: id.clone(),
                                qualified: n.qualified.clone(),
                                kind: n.kind.clone(),
                                op: '~',
                                lineage: derived.lineage(repo, &id),
                                from: None,
                                start_line: n.start_line,
                                end_line: n.end_line,
                                add: 0,
                                del: 1,
                                provenance: ranges(prov, n.start_line, n.end_line),
                                touches: vec![],
                            },
                        );
                    }
                    None => {
                        let id = format!("{path}::{}", s.qualified);
                        out.insert(
                            s.qualified.clone(),
                            SymbolChange {
                                id: id.clone(),
                                qualified: s.qualified.clone(),
                                kind: s.kind.clone(),
                                op: '-',
                                lineage: derived.lineage(repo, &id),
                                from: None,
                                start_line: s.start_line,
                                end_line: s.end_line,
                                add: 0,
                                del: 1,
                                provenance: vec![],
                                touches: vec![],
                            },
                        );
                    }
                }
            }
            None => top_del += 1,
        }
    }
    let mut v: Vec<SymbolChange> = out.into_values().collect();
    // A parent whose only changes sit inside a changed child is not listed separately.
    let quals: Vec<String> = v.iter().map(|s| s.qualified.clone()).collect();
    v.retain(|s| {
        !(s.add + s.del == 0
            && quals
                .iter()
                .any(|q| q != &s.qualified && q.starts_with(&format!("{} ›", s.qualified))))
    });
    if (top_add + top_del > 0) && !new.is_empty() {
        let id = format!("{path}::(top level)");
        v.push(SymbolChange {
            id: id.clone(),
            qualified: "(top level)".into(),
            kind: "file".into(),
            op: '~',
            lineage: derived.lineage(repo, &id),
            from: None,
            start_line: 0,
            end_line: 0,
            add: top_add,
            del: top_del,
            provenance: vec![],
            touches: vec![],
        });
    }
    v.sort_by_key(|s| (s.start_line, s.qualified.clone()));
    v
}

/// The definition a changed line belongs to: the innermost one around it, or the next one when the line sits
/// just before it (its comment, the blank line above it) with only changed lines between. A definition's own
/// first line (a heading, a signature) stays with it.
fn owner(symbols: &[Symbol], line: usize, changed: &BTreeSet<usize>) -> Option<usize> {
    let inner = lang::symbol_at(symbols, line);
    if inner.is_some_and(|i| symbols[i].start_line == line) {
        return inner;
    }
    let next = symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.start_line > line && s.parent == inner)
        .min_by_key(|(_, s)| s.start_line);
    match next {
        Some((i, s))
            if s.start_line - line <= 3
                && (line + 1..s.start_line).all(|l| changed.contains(&l)) =>
        {
            Some(i)
        }
        _ => inner,
    }
}

/// The provenance of lines `start..=end`, merged into runs by the same source.
fn ranges(prov: &[Option<Provenance>], start: usize, end: usize) -> Vec<Provenance> {
    let mut out: Vec<Provenance> = Vec::new();
    for line in start..=end.min(prov.len()) {
        let Some(Some(p)) = prov.get(line.wrapping_sub(1)) else {
            continue;
        };
        match out.last_mut() {
            Some(last) if last.source == p.source && last.end_line + 1 == line => {
                last.end_line = line
            }
            _ => {
                let mut r = p.clone();
                r.start_line = line;
                r.end_line = line;
                out.push(r);
            }
        }
    }
    out
}

fn touches(
    names: &NameIndex,
    parsed: &Parsed,
    path: &str,
    sc: &SymbolChange,
    changed: &BTreeSet<String>,
) -> Vec<Touch> {
    let mut out: Vec<Touch> = Vec::new();
    let mut seen = BTreeSet::new();
    // What it calls or uses.
    for r in parsed
        .references
        .iter()
        .filter(|r| r.line >= sc.start_line && r.line <= sc.end_line)
    {
        if let Some(defs) = names.defs.get(&r.name) {
            for (p, q) in defs
                .iter()
                .filter(|(p, q)| !(p == path && *q == sc.qualified))
                .take(2)
            {
                let to = format!("{p}::{q}");
                if seen.insert(to.clone()) {
                    out.push(Touch {
                        how: if r.kind == "call" {
                            "calls".into()
                        } else {
                            "uses".into()
                        },
                        changed: changed.contains(&to),
                        to,
                    });
                }
            }
        }
        if out.len() >= MAX_TOUCHES {
            break;
        }
    }
    // Who calls or uses it.
    let name = sc.qualified.rsplit(' ').next().unwrap_or(&sc.qualified);
    if let Some(users) = names.refs.get(name) {
        for (p, q) in users
            .iter()
            .filter(|(p, q)| !(p == path && *q == sc.qualified))
            .take(MAX_TOUCHES.saturating_sub(out.len()).max(2))
        {
            let to = format!("{p}::{q}");
            if seen.insert(to.clone()) {
                out.push(Touch {
                    how: "used by".into(),
                    changed: changed.contains(&to),
                    to,
                });
            }
        }
    }
    out
}

/// The symbol cache's on-disk form.
#[derive(Serialize, serde::Deserialize)]
struct CachedParse {
    symbols: Vec<CachedSymbol>,
    references: Vec<(String, String, usize)>,
    error: bool,
}

#[derive(Serialize, serde::Deserialize)]
struct CachedSymbol {
    kind: String,
    name: String,
    qualified: String,
    bytes: (usize, usize),
    lines: (usize, usize),
    content: Hash,
    shape: Hash,
    parent: Option<usize>,
}

impl CachedParse {
    fn from(p: &Parsed) -> CachedParse {
        CachedParse {
            symbols: p
                .symbols
                .iter()
                .map(|s| CachedSymbol {
                    kind: s.kind.clone(),
                    name: s.name.clone(),
                    qualified: s.qualified.clone(),
                    bytes: (s.start_byte, s.end_byte),
                    lines: (s.start_line, s.end_line),
                    content: s.content_hash,
                    shape: s.shape_hash,
                    parent: s.parent,
                })
                .collect(),
            references: p
                .references
                .iter()
                .map(|r| (r.kind.clone(), r.name.clone(), r.line))
                .collect(),
            error: p.error,
        }
    }

    fn into_parsed(self, pack: &str) -> Parsed {
        Parsed {
            pack: Some(pack.to_string()),
            symbols: self
                .symbols
                .into_iter()
                .map(|s| Symbol {
                    kind: s.kind,
                    name: s.name,
                    qualified: s.qualified,
                    start_byte: s.bytes.0,
                    end_byte: s.bytes.1,
                    start_line: s.lines.0,
                    end_line: s.lines.1,
                    content_hash: s.content,
                    shape_hash: s.shape,
                    parent: s.parent,
                })
                .collect(),
            references: self
                .references
                .into_iter()
                .map(|(kind, name, line)| lang::Reference { kind, name, line })
                .collect(),
            error: self.error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_segments_and_trees() {
        assert!(glob("crates/apid/**", "crates/apid/src/nodes.rs"));
        assert!(glob("apps/web/src/*.ts", "apps/web/src/store.ts"));
        assert!(!glob("apps/web/src/*.ts", "apps/web/src/views/x.ts"));
        assert!(glob("**/*.md", "docs/design/weft.md"));
        assert!(glob("docs/architecture.md", "docs/architecture.md"));
    }

    #[test]
    fn an_intent_covers_paths_and_symbols() {
        let i = IntentDef {
            title: "t".into(),
            summary: String::new(),
            kind: "fix".into(),
            covers: vec![
                "crates/apid/src/nodes.rs::memory_mib".into(),
                "docs/**".into(),
            ],
        };
        assert!(covers(
            &i,
            "crates/apid/src/nodes.rs",
            "crates/apid/src/nodes.rs::fn memory_mib"
        ));
        assert!(!covers(
            &i,
            "crates/apid/src/nodes.rs",
            "crates/apid/src/nodes.rs::fn local_summary"
        ));
        assert!(covers(&i, "docs/architecture.md", "docs/architecture.md"));
    }
}
