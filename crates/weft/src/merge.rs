//! Merging a change onto a moved ref: a line merge first; where lines clash, a structural merge per definition
//! (through the file's pack); where a file does not parse, the clash stands.
//!
//! `base` is the tree the change started from, `ours` the change's revision, `theirs` the ref's head now.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use similar::{DiffOp, TextDiff};

use crate::derived::Derived;
use crate::diff;
use crate::hash::Hash;
use crate::lang::Symbol;
use crate::model::{Delta, Entry, Tree};
use crate::store::Store;

#[derive(Clone, Debug, Serialize)]
pub struct Clash {
    pub path: String,
    /// `content`, `delete/modify`, `binary`, `mode`.
    pub kind: String,
    /// The definitions both sides changed, when the file has a pack.
    pub symbols: Vec<String>,
    /// The file with conflict markers, for the resolve task (text files only).
    #[serde(skip)]
    pub marked: Option<String>,
}

#[derive(Debug, Default)]
pub struct Merged {
    /// The merged tree's deltas against `theirs`.
    pub deltas: Vec<Delta>,
    pub clashes: Vec<Clash>,
    /// Files merged from both sides, and how (`lines` or `symbols`).
    pub merged: Vec<(String, String)>,
}

/// Merges the change (`base` → `ours`) onto `theirs`.
pub fn merge_trees(
    store: &Store,
    derived: &Derived,
    base: &Tree,
    ours: &Tree,
    theirs: &Tree,
) -> Result<Merged, String> {
    let mut out = Merged::default();
    let paths: BTreeSet<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let read = |e: Option<&Entry>| -> Result<Option<Vec<u8>>, String> {
        e.map(|e| store.blobs.get(&e.hash)).transpose()
    };
    for path in paths {
        let (b, o, t) = (base.get(path), ours.get(path), theirs.get(path));
        if o == b {
            continue; // the change did not touch it: theirs stands
        }
        if t == b || o == t {
            // Only the change touched it (or both made the same edit).
            if t != o {
                out.deltas.push(Delta {
                    path: path.clone(),
                    after: o.map(|e| e.hash),
                    mode: o.map(|e| e.mode).unwrap_or(0o100644),
                });
            }
            continue;
        }
        // Both sides changed it.
        let mode = match (b.map(|e| e.mode), o.map(|e| e.mode), t.map(|e| e.mode)) {
            (bm, Some(om), Some(tm)) if om != tm => {
                if Some(om) == bm {
                    tm
                } else if Some(tm) == bm {
                    om
                } else {
                    out.clashes.push(Clash {
                        path: path.clone(),
                        kind: "mode".into(),
                        symbols: vec![],
                        marked: None,
                    });
                    continue;
                }
            }
            (_, Some(om), _) => om,
            (_, None, Some(tm)) => tm,
            _ => 0o100644,
        };
        let (bb, ob, tb) = (read(b)?.unwrap_or_default(), read(o)?, read(t)?);
        let (Some(ob), Some(tb)) = (ob, tb) else {
            out.clashes.push(Clash {
                path: path.clone(),
                kind: "delete/modify".into(),
                symbols: vec![],
                marked: None,
            });
            continue;
        };
        if !(diff::is_text(&bb) && diff::is_text(&ob) && diff::is_text(&tb)) {
            out.clashes.push(Clash {
                path: path.clone(),
                kind: "binary".into(),
                symbols: vec![],
                marked: None,
            });
            continue;
        }
        let (bs, os, ts) = (
            String::from_utf8_lossy(&bb).to_string(),
            String::from_utf8_lossy(&ob).to_string(),
            String::from_utf8_lossy(&tb).to_string(),
        );
        match merge_text(&bs, &os, &ts) {
            Ok(text) => {
                let h = store.blobs.put(text.as_bytes())?;
                out.deltas.push(Delta {
                    path: path.clone(),
                    after: Some(h),
                    mode,
                });
                out.merged.push((path.clone(), "lines".into()));
            }
            Err(marked) => match merge_symbols(store, derived, path, &bs, &os, &ts) {
                Ok(text) => {
                    let h = store.blobs.put(text.as_bytes())?;
                    out.deltas.push(Delta {
                        path: path.clone(),
                        after: Some(h),
                        mode,
                    });
                    out.merged.push((path.clone(), "symbols".into()));
                }
                Err(symbols) => out.clashes.push(Clash {
                    path: path.clone(),
                    kind: "content".into(),
                    symbols,
                    marked: Some(marked),
                }),
            },
        }
    }
    out.deltas.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// One side's edit of the base: base lines `start..end` become `lines`.
#[derive(Clone, Debug, PartialEq)]
struct Edit {
    start: usize,
    end: usize,
    lines: Vec<String>,
}

fn edits(base: &[&str], other: &[&str]) -> Vec<Edit> {
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .diff_slices(base, other);
    let mut out: Vec<Edit> = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => out.push(Edit {
                start: old_index,
                end: old_index + old_len,
                lines: vec![],
            }),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => out.push(Edit {
                start: old_index,
                end: old_index,
                lines: other[new_index..new_index + new_len]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            }),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => out.push(Edit {
                start: old_index,
                end: old_index + old_len,
                lines: other[new_index..new_index + new_len]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            }),
        }
    }
    // Neighbouring edits with nothing between them are one edit.
    let mut merged: Vec<Edit> = Vec::new();
    for e in out {
        match merged.last_mut() {
            Some(last) if last.end == e.start && !(last.start == last.end && e.start == e.end) => {
                last.end = e.end;
                last.lines.extend(e.lines);
            }
            _ => merged.push(e),
        }
    }
    merged
}

/// A three-way line merge. `Err` carries the text with conflict markers.
pub fn merge_text(base: &str, ours: &str, theirs: &str) -> Result<String, String> {
    let split = |s: &str| -> Vec<String> { s.split_inclusive('\n').map(str::to_string).collect() };
    let (b, o, t) = (split(base), split(ours), split(theirs));
    let br: Vec<&str> = b.iter().map(String::as_str).collect();
    let or: Vec<&str> = o.iter().map(String::as_str).collect();
    let tr: Vec<&str> = t.iter().map(String::as_str).collect();
    // Every edit, tagged with its side (0 ours, 1 theirs), in base order.
    let mut all: Vec<(usize, Edit)> = edits(&br, &or)
        .into_iter()
        .map(|e| (0, e))
        .chain(edits(&br, &tr).into_iter().map(|e| (1, e)))
        .collect();
    all.sort_by_key(|(side, e)| (e.start, e.end, *side));
    // Groups of edits that touch the same base lines (or insert at the same place, or next to the other's edit).
    let mut groups: Vec<Vec<(usize, Edit)>> = Vec::new();
    for (side, e) in all {
        let joins = groups.last().is_some_and(|g| {
            let gs = g.iter().map(|x| x.1.start).min().unwrap();
            let ge = g.iter().map(|x| x.1.end).max().unwrap();
            let insert = e.start == e.end;
            e.start < ge || (e.start == ge && (insert || gs == ge)) || (insert && e.start == gs)
        });
        if joins {
            groups.last_mut().unwrap().push((side, e));
        } else {
            groups.push(vec![(side, e)]);
        }
    }
    let mut out = String::new();
    let mut clean = true;
    let mut at = 0;
    for g in groups {
        let start = g.iter().map(|x| x.1.start).min().unwrap();
        let end = g.iter().map(|x| x.1.end).max().unwrap();
        for line in &br[at..start] {
            out.push_str(line);
        }
        // One side's text for base[start..end]: its edits there, base lines between them.
        let side_text = |side: usize| -> Option<String> {
            let mine: Vec<&Edit> = g.iter().filter(|x| x.0 == side).map(|x| &x.1).collect();
            if mine.is_empty() {
                return None;
            }
            let mut s = String::new();
            let mut pos = start;
            for e in mine {
                for l in &br[pos..e.start] {
                    s.push_str(l);
                }
                for l in &e.lines {
                    s.push_str(l);
                }
                pos = e.end;
            }
            for l in &br[pos..end] {
                s.push_str(l);
            }
            Some(s)
        };
        match (side_text(0), side_text(1)) {
            (Some(o), None) => out.push_str(&o),
            (None, Some(t)) => out.push_str(&t),
            (Some(o), Some(t)) if o == t => out.push_str(&o),
            (Some(o), Some(t)) => {
                clean = false;
                out.push_str("<<<<<<< this change\n");
                out.push_str(&o);
                if !o.is_empty() && !o.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("=======\n");
                out.push_str(&t);
                if !t.is_empty() && !t.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(">>>>>>> main\n");
            }
            (None, None) => {}
        }
        at = end;
    }
    for line in &br[at..] {
        out.push_str(line);
    }
    if clean { Ok(out) } else { Err(out) }
}

/// A file cut into units: each top-level definition with the text before it, keyed by its name.
fn units(text: &str, symbols: &[Symbol]) -> Option<Vec<(String, String)>> {
    let mut tops: Vec<&Symbol> = symbols.iter().filter(|s| s.parent.is_none()).collect();
    tops.sort_by_key(|s| s.start_byte);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    let mut seen = BTreeSet::new();
    for s in tops {
        if s.start_byte < at {
            return None;
        }
        // Extend to the end of the definition's last line.
        let end = text[s.end_byte..]
            .find('\n')
            .map(|i| s.end_byte + i + 1)
            .unwrap_or(text.len());
        if !seen.insert(s.qualified.clone()) {
            return None; // two definitions of one name: no safe key
        }
        out.push((s.qualified.clone(), text[at..end].to_string()));
        at = end;
    }
    out.push(("(end)".into(), text[at..].to_string()));
    Some(out)
}

/// A structural merge: definitions both sides changed are merged line by line within themselves; definitions
/// only one side changed, added or removed are taken from that side. `Err` names the definitions that still clash.
pub fn merge_symbols(
    store: &Store,
    derived: &Derived,
    path: &str,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Result<String, Vec<String>> {
    let parse =
        |t: &str| derived.parse_bytes(store, path, t.as_bytes(), Some(Hash::of(t.as_bytes())));
    let (pb, po, pt) = (parse(base), parse(ours), parse(theirs));
    if pb.pack.is_none() || pb.error || po.error || pt.error {
        return Err(vec![]);
    }
    let (Some(ub), Some(uo), Some(ut)) = (
        units(base, &pb.symbols),
        units(ours, &po.symbols),
        units(theirs, &pt.symbols),
    ) else {
        return Err(vec![]);
    };
    let bm: BTreeMap<&str, &str> = ub.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let om: BTreeMap<&str, &str> = uo.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let tm: BTreeMap<&str, &str> = ut.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut clashes = Vec::new();
    let mut pick = |name: &str| -> Option<String> {
        let (b, o, t) = (
            bm.get(name).copied(),
            om.get(name).copied(),
            tm.get(name).copied(),
        );
        match (b, o, t) {
            (_, Some(o), Some(t)) if o == t => Some(o.to_string()),
            (Some(b), Some(o), Some(t)) if o == b => Some(t.to_string()),
            (Some(b), Some(o), Some(t)) if t == b => Some(o.to_string()),
            (Some(b), Some(o), Some(t)) => match merge_text(b, o, t) {
                Ok(m) => Some(m),
                Err(_) => {
                    clashes.push(name.to_string());
                    None
                }
            },
            (None, Some(o), Some(t)) => match merge_text("", o, t) {
                Ok(m) => Some(m),
                Err(_) => {
                    clashes.push(name.to_string());
                    None
                }
            },
            (Some(b), None, Some(t)) if t == b => None, // we removed it
            (Some(b), Some(o), None) if o == b => None, // they removed it
            (Some(_), None, Some(_)) | (Some(_), Some(_), None) => {
                clashes.push(name.to_string());
                None
            }
            (None, Some(o), None) => Some(o.to_string()),
            (None, None, Some(t)) => Some(t.to_string()),
            _ => None,
        }
    };
    // Their order, with our new units after the unit they follow in ours.
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, _) in &ut {
        if name == "(end)" {
            continue;
        }
        if let Some(text) = pick(name) {
            out.push((name.clone(), text));
        }
    }
    let mut prev: Option<&str> = None;
    for (name, _) in &uo {
        if name != "(end)" && !tm.contains_key(name.as_str()) && !bm.contains_key(name.as_str()) {
            let text = pick(name).unwrap_or_default();
            let mut at = prev
                .and_then(|p| out.iter().position(|(n, _)| n == p))
                .map(|i| i + 1)
                .unwrap_or(0);
            // After what main added at the same place: main's additions first, then the change's.
            while at < out.len()
                && !bm.contains_key(out[at].0.as_str())
                && !om.contains_key(out[at].0.as_str())
            {
                at += 1;
            }
            out.insert(at, (name.clone(), text));
        }
        prev = Some(name.as_str());
    }
    let end = pick("(end)").unwrap_or_default();
    if !clashes.is_empty() {
        return Err(clashes);
    }
    let mut text: String = out.into_iter().map(|(_, t)| t).collect();
    text.push_str(&end);
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_edits_merge_and_overlapping_ones_clash() {
        let base = "a\nb\nc\nd\ne\n";
        assert_eq!(
            merge_text(base, "A\nb\nc\nd\ne\n", "a\nb\nc\nd\nE\n").unwrap(),
            "A\nb\nc\nd\nE\n"
        );
        assert_eq!(
            merge_text(base, "a\nb\nC\nd\ne\n", "a\nb\nC\nd\ne\n").unwrap(),
            "a\nb\nC\nd\ne\n"
        );
        let marked = merge_text(base, "a\nb\nX\nd\ne\n", "a\nb\nY\nd\ne\n").unwrap_err();
        assert!(
            marked.contains("<<<<<<< this change\nX\n=======\nY\n>>>>>>> main\n"),
            "{marked}"
        );
        // Both append at the end: a line clash (the structural merge takes both).
        assert!(merge_text(base, "a\nb\nc\nd\ne\nf\n", "a\nb\nc\nd\ne\ng\n").is_err());
        // Insert and delete far apart.
        assert_eq!(
            merge_text(base, "a\nb\nc\nd\n", "z\na\nb\nc\nd\ne\n").unwrap(),
            "z\na\nb\nc\nd\n"
        );
    }
}
