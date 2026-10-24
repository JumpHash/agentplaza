//! History as commits (PLAN 21.6): each landing reads as a commit. A landing's content id is a hash of its revision,
//! its tree and the landing before it, so the ids form the same chain git's commits do; the view (oldest first) gives its parents,
//! its author as the connection's platform knows it, its message (the one export writes), its time, the hub's signed
//! records (its sign-off chain) and the git commit each connection received. The open changes on the reference are
//! its lanes, each with the landing it starts from.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::export::landing_message;
use crate::hash::Hash;
use crate::model::RevId;
use crate::state::ChangeState;
use crate::state::{Change, State};
use crate::store::{Store, tree_hash};

/// A landing's content id: its revision, its tree and the landing before it (`l-` and 16 hex digits).
pub fn content_id(rev: &str, tree: &Hash, parent: Option<&str>) -> String {
    let h = Hash::of_parts(&[
        rev.as_bytes(),
        tree.0.as_slice(),
        parent.unwrap_or("root").as_bytes(),
    ]);
    format!("l-{}", &h.hex()[..16])
}

/// The review run that checked a change: the run its last gate record names (the hub signs the gate when a review
/// settles). None until a review has settled.
pub fn review_run_of(st: &State, change: &Change) -> Option<String> {
    change
        .records
        .iter()
        .rev()
        .filter_map(|s| st.records.get(s))
        .find(|r| r.attest.predicate_type == "gate")
        .and_then(|r| r.attest.predicate["run"].as_str().map(str::to_string))
}

/// The history of `reference` in `repo`: its landings oldest first, and its lanes.
pub fn view(store: &Store, repo: &str, reference: &str) -> Result<Value, String> {
    let chain: Vec<RevId> = {
        let st = store.state();
        st.repo(repo)?
            .refs
            .get(reference)
            .cloned()
            .unwrap_or_default()
    };
    // Trees are read before the state is held: the store's own lock is not re-entered.
    let mut trees = Vec::with_capacity(chain.len());
    for rev in &chain {
        trees.push(tree_hash(&*store.tree(rev)?));
    }
    let st = store.state();
    let r = st.repo(repo)?;
    let author = r.connections.values().next().map(|c| {
        json!({ "name": c.conn.author_name, "email": c.conn.author_email, "connection": c.conn.id })
    });
    let mut ids: HashMap<RevId, String> = HashMap::new();
    let mut landings = Vec::new();
    let mut parent: Option<String> = None;
    for (rev, tree) in chain.iter().zip(&trees) {
        let id = content_id(rev, tree, parent.as_deref());
        ids.insert(rev.clone(), id.clone());
        let parents: Vec<String> = parent.iter().cloned().collect();
        parent = Some(id.clone());
        let Some(revision) = st.revisions.get(rev) else {
            continue;
        };
        let Some(change) = st.changes.get(&revision.change) else {
            continue;
        };
        let intents = change
            .intents
            .get(&revision.n)
            .cloned()
            .or_else(|| change.intents.values().last().cloned())
            .unwrap_or_default();
        let exported: serde_json::Map<String, Value> = change
            .exported
            .iter()
            .filter_map(|(conn, commits)| commits.last().map(|c| (conn.clone(), json!(c.git))))
            .collect();
        landings.push(json!({
            "id": id,
            "parents": parents,
            "rev": rev,
            "change": change.name,
            "title": change.title,
            "kind": change.kind,
            "imported": change.import.is_some(),
            "intents": intents.iter().map(|i| json!({ "title": i.title, "summary": i.summary, "kind": i.kind })).collect::<Vec<_>>(),
            "author": author,
            "time_ms": revision.at_ms,
            "review_run": review_run_of(&st, change),
            "message": match &change.import {
                Some(i) => i.message.clone(),
                None => landing_message(&change.title, &intents, review_run_of(&st, change).as_deref()),
            },
            "exported": exported,
            "records": change.records.iter().filter_map(|s| st.records.get(s)).map(|r| json!({ "seq": r.seq, "at_ms": r.at_ms, "actor": r.actor, "key_id": r.key_id, "attest": r.attest })).collect::<Vec<_>>(),
        }));
    }
    let lanes: Vec<Value> = st
        .changes
        .values()
        .filter(|c| c.repo == repo && c.reference == reference)
        .filter(|c| {
            matches!(
                c.state,
                ChangeState::Open
                    | ChangeState::Sealed
                    | ChangeState::Queued
                    | ChangeState::Conflict
            )
        })
        .map(|c| {
            json!({
                "change": c.name,
                "title": c.title,
                "state": c.state,
                "round": c.round(),
                "base": ids.get(&c.base),
                "kind": c.kind,
            })
        })
        .collect();
    Ok(json!({
        "repo": repo,
        "ref": reference,
        "head": parent,
        "landings": landings,
        "lanes": lanes,
    }))
}
