//! Landing: each repo's queues, one per ref; only a queue's head moves its ref.
//!
//! A step takes the head of a queue and merges it onto the ref ([`crate::merge`]). A change whose base is the
//! head lands as it is. A merged one becomes a candidate: when the repo asks for re-checks, the candidate waits for
//! the checks' verdict (apid runs them in a VM and reports back); otherwise it lands at once. A clash leaves the
//! queue: the change moves onto the head with the clashing files marked, for the lane's agent to resolve.
//!
//! While the head's candidate is being checked, the next item is merged on top of it (speculatively), so it can
//! land as soon as the head does. When the head stalls, a later item that shares no file with it may land first
//! (the two commute). After a landing, lanes stacked on the landed change move onto it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{Value, json};

use crate::derived::Derived;
use crate::hash::Hash;
use crate::merge;
use crate::model::*;
use crate::record::{Actor, now_ms};
use crate::state::ChangeState;
use crate::store::{Store, apply_deltas, tree_hash};

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub change: String,
    pub n: u32,
    /// The ref head it was merged onto, and that tree's hash (a speculative candidate sits on another candidate's
    /// tree, which the head's landing makes the ref's).
    pub onto: RevId,
    pub onto_tree: Hash,
    pub deltas: Vec<Delta>,
    pub digest: Hash,
    pub merged: Vec<String>,
    /// Whether it lands on a tree other than the one it was reviewed on (so a re-check applies).
    pub rebased: bool,
    pub files: BTreeSet<String>,
    /// `None` while the checks run.
    pub checked: Option<bool>,
    pub made_ms: u64,
    pub speculative: bool,
    /// Whether a step has already asked for its re-check.
    pub announced: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Step {
    Empty,
    Landed {
        change: String,
        revision: RevId,
        merged: Vec<String>,
    },
    Recheck {
        change: String,
        digest: Hash,
    },
    Waiting {
        change: String,
        digest: Hash,
    },
    Conflict {
        change: String,
        files: Vec<String>,
        symbols: Vec<String>,
    },
    Failed {
        change: String,
        reason: String,
    },
}

#[derive(Default)]
pub struct Lander {
    /// Per `repo|ref`: the candidates, head first.
    candidates: Mutex<HashMap<String, Vec<Candidate>>>,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Wait for the checks' verdict on a merged candidate before it lands.
    pub recheck: bool,
    /// How long a head may wait on its checks before a commuting item may pass it.
    pub stall_ms: u64,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            recheck: false,
            stall_ms: 10 * 60 * 1000,
        }
    }
}

fn key(repo: &str, reference: &str) -> String {
    format!("{repo}|{reference}")
}

impl Lander {
    pub fn candidates(&self, repo: &str, reference: &str) -> Vec<Candidate> {
        self.candidates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key(repo, reference))
            .cloned()
            .unwrap_or_default()
    }

    /// The tree a candidate would land (for the checks to run on).
    pub fn candidate_tree(&self, store: &Store, digest: &Hash) -> Result<Tree, String> {
        let all = self
            .candidates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        let c = all
            .iter()
            .find(|c| c.digest == *digest)
            .ok_or("no such candidate")?;
        let mut tree = (*store.tree(&c.onto)?).clone();
        // A speculative candidate sits on the head candidate's tree.
        if c.speculative
            && let Some(head) = all
                .iter()
                .find(|h| !h.speculative && h.onto == c.onto && h.change != c.change)
        {
            apply_deltas(&mut tree, &head.deltas);
        }
        apply_deltas(&mut tree, &c.deltas);
        Ok(tree)
    }

    /// The checks' verdict on a candidate.
    pub fn report(&self, digest: &Hash, ok: bool) -> bool {
        let mut all = self.candidates.lock().unwrap_or_else(|p| p.into_inner());
        for list in all.values_mut() {
            for c in list.iter_mut() {
                if c.digest == *digest {
                    c.checked = Some(ok);
                    return true;
                }
            }
        }
        false
    }

    /// One step of a queue.
    pub fn step(
        &self,
        store: &Store,
        derived: &Derived,
        repo: &str,
        reference: &str,
        opts: Options,
    ) -> Result<Step, String> {
        let (queue, head) = {
            let st = store.state();
            let r = st.repo(repo)?;
            let q: Vec<(u64, String, u32, String)> = r
                .queues
                .get(reference)
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|id| st.changes.get(id))
                .map(|c| (c.id, c.name.clone(), c.round(), c.kind.clone()))
                .collect();
            (q, r.head(reference))
        };
        if queue.is_empty() {
            self.candidates
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&key(repo, reference));
            return Ok(Step::Empty);
        }
        let head_tree = tree_hash(&*store.tree(&head)?);
        // Reverts go first.
        let order: Vec<&(u64, String, u32, String)> = queue
            .iter()
            .filter(|q| q.3 == "revert")
            .chain(queue.iter().filter(|q| q.3 != "revert"))
            .collect();
        let first = order[0];
        // Drop candidates that no longer fit: not queued any more, or merged onto a tree that is not the head's.
        {
            let mut all = self.candidates.lock().unwrap_or_else(|p| p.into_inner());
            let list = all.entry(key(repo, reference)).or_default();
            list.retain(|c| queue.iter().any(|q| q.1 == c.change && q.2 == c.n));
            let head_change = first.1.clone();
            list.retain(|c| {
                if c.change == head_change {
                    c.onto_tree == head_tree || (c.speculative && c.onto_tree == head_tree)
                } else {
                    true
                }
            });
            // A speculative candidate becomes the head's once its base tree is the head's tree.
            for c in list.iter_mut() {
                if c.speculative && c.onto_tree == head_tree {
                    c.speculative = false;
                    c.onto = head.clone();
                }
            }
        }
        let existing = self
            .candidates(repo, reference)
            .into_iter()
            .find(|c| c.change == first.1 && !c.speculative && c.onto_tree == head_tree);
        let cand = match existing {
            Some(c) => c,
            None => match self.make(store, derived, repo, &first.1, first.2, &head, None)? {
                Ok(c) => {
                    self.candidates
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .entry(key(repo, reference))
                        .or_default()
                        .insert(0, c.clone());
                    c
                }
                Err(clash) => {
                    return self.clash(store, repo, reference, &first.1, first.2, &head, clash);
                }
            },
        };
        let needs_check = opts.recheck && cand.rebased;
        match (needs_check, cand.checked) {
            (false, _) | (true, Some(true)) => self.land(store, derived, repo, reference, &cand),
            (true, Some(false)) => {
                store.hub_append("queue.remove", json!({ "repo": repo, "ref": reference, "change": cand.change, "n": cand.n, "reason": "the re-check on the merged revision failed" }))?;
                self.forget(repo, reference, &cand.change);
                Ok(Step::Failed {
                    change: cand.change,
                    reason: "the re-check failed".into(),
                })
            }
            (true, None) => {
                // Speculatively merge the next item on top of the head's candidate.
                if let Some(next) = order.get(1)
                    && !self
                        .candidates(repo, reference)
                        .iter()
                        .any(|c| c.change == next.1)
                    && let Ok(Ok(c)) =
                        self.make(store, derived, repo, &next.1, next.2, &head, Some(&cand))
                {
                    self.candidates
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .entry(key(repo, reference))
                        .or_default()
                        .push(c);
                }
                // A stalled head lets a commuting item pass.
                if now_ms().saturating_sub(cand.made_ms) >= opts.stall_ms {
                    for item in order.iter().skip(1) {
                        let Ok(Ok(c)) =
                            self.make(store, derived, repo, &item.1, item.2, &head, None)
                        else {
                            continue;
                        };
                        if !c.rebased && c.files.is_disjoint(&cand.files) {
                            return self.land(store, derived, repo, reference, &c);
                        }
                    }
                }
                let first_ask = {
                    let mut all = self.candidates.lock().unwrap_or_else(|p| p.into_inner());
                    let c = all
                        .entry(key(repo, reference))
                        .or_default()
                        .iter_mut()
                        .find(|c| c.digest == cand.digest);
                    c.map(|c| !std::mem::replace(&mut c.announced, true))
                        .unwrap_or(false)
                };
                Ok(if first_ask {
                    Step::Recheck {
                        change: cand.change,
                        digest: cand.digest,
                    }
                } else {
                    Step::Waiting {
                        change: cand.change,
                        digest: cand.digest,
                    }
                })
            }
        }
    }

    fn forget(&self, repo: &str, reference: &str, change: &str) {
        if let Some(list) = self
            .candidates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&key(repo, reference))
        {
            list.retain(|c| c.change != change);
        }
    }

    /// A candidate for `change` onto `head` (or onto `below`, the head's candidate, speculatively).
    #[allow(clippy::too_many_arguments)]
    fn make(
        &self,
        store: &Store,
        derived: &Derived,
        _repo: &str,
        change: &str,
        n: u32,
        head: &str,
        below: Option<&Candidate>,
    ) -> Result<Result<Candidate, merge::Merged>, String> {
        let rev_id = format!("{change}.r{n}");
        let (base_id, rev_deltas) = {
            let st = store.state();
            let r = st
                .revisions
                .get(&rev_id)
                .ok_or_else(|| format!("no revision {rev_id}"))?;
            (r.base.clone(), r.deltas.clone())
        };
        let mut theirs = (*store.tree(head)?).clone();
        if let Some(b) = below {
            apply_deltas(&mut theirs, &b.deltas);
        }
        let onto_tree = tree_hash(&theirs);
        let (deltas, merged, rebased) = if base_id == head && below.is_none() {
            (rev_deltas, vec![], false)
        } else {
            let base = store.tree(&base_id)?;
            let ours = store.tree(&rev_id)?;
            let m = merge::merge_trees(store, derived, &base, &ours, &theirs)?;
            if !m.clashes.is_empty() {
                return Ok(Err(m));
            }
            // A fast-forward in all but name: the base was the head's tree.
            let rebased = tree_hash(&base) != onto_tree;
            let merged = if rebased {
                m.merged
                    .iter()
                    .map(|(p, how)| format!("{p} ({how})"))
                    .collect()
            } else {
                vec![]
            };
            (m.deltas, merged, rebased)
        };
        let files = deltas.iter().map(|d| d.path.clone()).collect();
        let digest = crate::state::digest(head, &deltas);
        Ok(Ok(Candidate {
            change: change.into(),
            n,
            onto: head.into(),
            onto_tree,
            deltas,
            digest,
            merged,
            rebased,
            files,
            checked: None,
            made_ms: now_ms(),
            speculative: below.is_some(),
            announced: false,
        }))
    }

    fn land(
        &self,
        store: &Store,
        derived: &Derived,
        repo: &str,
        reference: &str,
        c: &Candidate,
    ) -> Result<Step, String> {
        let head = store.state().repo(repo)?.head(reference);
        store.hub_append("land", json!({ "repo": repo, "ref": reference, "change": c.change, "n": c.n, "expected": head, "deltas": c.deltas, "merged": c.merged }))?;
        let landed = format!("{}.l", c.change);
        let digest = store
            .state()
            .revisions
            .get(&landed)
            .map(|r| r.digest)
            .ok_or("the landing left no revision")?;
        store.hub_append("attest", json!({ "revision": landed, "digest": digest, "predicate_type": "landed", "predicate": { "ref": reference, "onto": head, "merged": c.merged, "checked": c.checked } }))?;
        self.forget(repo, reference, &c.change);
        // The next item merged on top of this one now sits on the ref's head.
        let new_tree = tree_hash(&*store.tree(&landed)?);
        if let Some(list) = self
            .candidates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&key(repo, reference))
        {
            for n in list
                .iter_mut()
                .filter(|n| n.speculative && n.onto_tree == new_tree)
            {
                n.speculative = false;
                n.onto = landed.clone();
                n.digest = crate::state::digest(&landed, &n.deltas);
            }
        }
        let _ = derived.learn(store, repo, reference);
        self.move_stacked(store, derived, &c.change)?;
        Ok(Step::Landed {
            change: c.change.clone(),
            revision: landed,
            merged: c.merged.clone(),
        })
    }

    /// A clash: the change leaves the queue and moves onto the head, with the clashing files marked in a draft for
    /// the resolve task.
    #[allow(clippy::too_many_arguments)]
    fn clash(
        &self,
        store: &Store,
        repo: &str,
        reference: &str,
        change: &str,
        n: u32,
        head: &str,
        m: merge::Merged,
    ) -> Result<Step, String> {
        let files: Vec<String> = m.clashes.iter().map(|c| c.path.clone()).collect();
        let symbols: Vec<String> = m
            .clashes
            .iter()
            .flat_map(|c| c.symbols.iter().map(move |s| format!("{}::{s}", c.path)))
            .collect();
        let detail = json!({ "clashes": m.clashes, "merged": m.merged.iter().map(|(p, h)| format!("{p} ({h})")).collect::<Vec<_>>() });
        store.hub_append(
            "conflict",
            json!({ "change": change, "n": n, "onto": head, "files": files, "detail": detail }),
        )?;
        self.forget(repo, reference, change);
        // The resolve draft: the clean merge plus the clashing files with markers, on top of the head.
        let mut deltas = m.deltas.clone();
        for c in &m.clashes {
            if let Some(text) = &c.marked {
                let h = store.blobs.put(text.as_bytes())?;
                deltas.push(Delta {
                    path: c.path.clone(),
                    after: Some(h),
                    mode: 0o100644,
                });
            }
        }
        deltas.sort_by(|a, b| a.path.cmp(&b.path));
        store.hub_append("lane.rebase", json!({ "change": change, "onto": head, "reason": format!("clash on {}", files.join(", ")) }))?;
        let next = store.state().change(change)?.drafts.len() as u32 + 1;
        store.hub_append(
            "revision.draft",
            json!({ "change": change, "n": next, "deltas": deltas, "tool_calls": ["weft:merge"] }),
        )?;
        Ok(Step::Conflict {
            change: change.into(),
            files,
            symbols,
        })
    }

    /// Lanes stacked on a change that just landed move onto its landed revision.
    fn move_stacked(
        &self,
        store: &Store,
        derived: &Derived,
        landed_change: &str,
    ) -> Result<(), String> {
        let stacked: Vec<(String, RevId, Option<RevId>)> = {
            let st = store.state();
            let id = st.change(landed_change)?.id;
            st.changes
                .values()
                .filter(|c| {
                    c.stacked_on == Some(id)
                        && matches!(c.state, ChangeState::Open | ChangeState::Sealed)
                })
                .map(|c| {
                    (
                        c.name.clone(),
                        c.base.clone(),
                        c.drafts
                            .iter()
                            .chain(&c.rounds)
                            .filter_map(|r| st.revisions.get(r))
                            .max_by_key(|r| r.seq)
                            .map(|r| r.id.clone()),
                    )
                })
                .collect()
        };
        let landed = format!("{landed_change}.l");
        for (name, base, latest) in stacked {
            let theirs = store.tree(&landed)?;
            let ours = match &latest {
                Some(l) => store.tree(l)?,
                None => store.tree(&base)?,
            };
            let m = merge::merge_trees(store, derived, &*store.tree(&base)?, &ours, &theirs)?;
            store.hub_append("lane.rebase", json!({ "change": name, "onto": landed, "reason": format!("{landed_change} landed") }))?;
            if latest.is_some() {
                let mut deltas = m.deltas.clone();
                for c in &m.clashes {
                    if let Some(text) = &c.marked {
                        deltas.push(Delta {
                            path: c.path.clone(),
                            after: Some(store.blobs.put(text.as_bytes())?),
                            mode: 0o100644,
                        });
                    }
                }
                let next = store.state().change(&name)?.drafts.len() as u32 + 1;
                store.hub_append("revision.draft", json!({ "change": name, "n": next, "deltas": deltas, "tool_calls": ["weft:restack"] }))?;
            }
        }
        Ok(())
    }
}

/// Opens, seals and queues a change that reverts `change` on its ref. The undo is merged onto the head, so later
/// work on the same files stays.
pub fn revert(
    store: &Store,
    derived: &Derived,
    change: &str,
    author: Actor,
    reason: &str,
) -> Result<String, String> {
    let (repo, reference, landed, title) = {
        let st = store.state();
        let c = st.change(change)?;
        let landed = c
            .landed
            .clone()
            .ok_or_else(|| format!("{change} has not landed"))?;
        (c.repo.clone(), c.reference.clone(), landed, c.title.clone())
    };
    let before = store
        .state()
        .revisions
        .get(&landed)
        .map(|r| r.base.clone())
        .ok_or("no landed revision")?;
    let head = store.state().repo(&repo)?.head(&reference);
    let m = merge::merge_trees(
        store,
        derived,
        &*store.tree(&landed)?,
        &*store.tree(&before)?,
        &*store.tree(&head)?,
    )?;
    if !m.clashes.is_empty() {
        return Err(format!(
            "later changes to {} stand in the way; revert by hand",
            m.clashes
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let name = store.next_change();
    store.hub_append("lane.open", json!({ "repo": repo, "change": name, "ref": reference, "title": format!("Revert \"{title}\""), "author": author, "base": head, "kind": "revert", "reverts": change }))?;
    store.hub_append(
        "revision.draft",
        json!({ "change": name, "n": 1, "deltas": m.deltas, "tool_calls": ["weft:revert"] }),
    )?;
    store.hub_append(
        "revision.seal",
        json!({ "change": name, "n": 1, "deltas": m.deltas }),
    )?;
    store.hub_append("intent.set", json!({ "change": name, "n": 1, "intents": [{ "title": format!("Revert \"{title}\""), "summary": reason, "kind": "revert", "covers": m.deltas.iter().map(|d| d.path.clone()).collect::<Vec<_>>() }] }))?;
    store.hub_append(
        "queue.add",
        json!({ "repo": repo, "ref": reference, "change": name, "n": 1 }),
    )?;
    Ok(name)
}

/// The landed revisions of a ref, oldest first (for bisect).
pub fn chain(store: &Store, repo: &str, reference: &str) -> Result<Vec<Value>, String> {
    let st = store.state();
    let r = st.repo(repo)?;
    Ok(r.refs
        .get(reference)
        .into_iter()
        .flatten()
        .filter_map(|id| st.revisions.get(id))
        .map(|rev| {
            let c = st.changes.get(&rev.change);
            json!({ "revision": rev.id, "change": c.map(|c| c.name.clone()), "title": c.map(|c| c.title.clone()), "task": c.and_then(|c| c.task.clone()), "seq": rev.seq })
        })
        .collect())
}
