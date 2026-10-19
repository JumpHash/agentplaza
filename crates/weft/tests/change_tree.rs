//! Change trees from real packs: a Rust change (edits, a new function, a rename, touches, provenance through
//! drafts, divergence from another lane) and a research document (sections and claims, no code at all).

mod common;

use common::plaza;
use serde_json::json;
use weft::derived::Derived;

const QUEUE_V1: &str = r#"pub struct Queues {
    pub open: Vec<u64>,
}

impl Queues {
    pub fn assign_reviews(&mut self, reviewers: &[u64]) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        for (i, task) in self.open.iter().enumerate() {
            out.push((*task, reviewers[i % reviewers.len()]));
        }
        out
    }

    fn old_helper(&self) -> usize {
        let total = self.open.len();
        total * 2 + 1
    }
}
"#;

// Turn 1: assign_reviews asks pick_reviewer, which is new.
const QUEUE_D1: &str = r#"pub struct Queues {
    pub open: Vec<u64>,
}

impl Queues {
    pub fn assign_reviews(&mut self, reviewers: &[u64]) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        for task in self.open.iter() {
            out.push((*task, pick_reviewer(reviewers, &out)));
        }
        out
    }

    fn old_helper(&self) -> usize {
        let total = self.open.len();
        total * 2 + 1
    }
}

/// The reviewer with the fewest reviews so far.
fn pick_reviewer(reviewers: &[u64], given: &[(u64, u64)]) -> u64 {
    *reviewers.iter().min_by_key(|r| given.iter().filter(|(_, g)| g == *r).count()).unwrap()
}
"#;

// Turn 2: old_helper is renamed (same body), assign_reviews skips an empty list.
const QUEUE_D2: &str = r#"pub struct Queues {
    pub open: Vec<u64>,
}

impl Queues {
    pub fn assign_reviews(&mut self, reviewers: &[u64]) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        if reviewers.is_empty() {
            return out;
        }
        for task in self.open.iter() {
            out.push((*task, pick_reviewer(reviewers, &out)));
        }
        out
    }

    fn weight(&self) -> usize {
        let total = self.open.len();
        total * 2 + 1
    }
}

/// The reviewer with the fewest reviews so far.
fn pick_reviewer(reviewers: &[u64], given: &[(u64, u64)]) -> u64 {
    *reviewers.iter().min_by_key(|r| given.iter().filter(|(_, g)| g == *r).count()).unwrap()
}
"#;

#[test]
fn a_rust_change_is_a_tree_of_definitions_with_who_wrote_what() {
    let p = plaza("tree-rust", &["agentplaza"]);
    p.add_packs(&["rust", "markdown"]);
    let s = &p.store;
    let base = p.lane(
        "agentplaza",
        107,
        "t50",
        "Queues",
        &[vec![
            p.put("crates/apid/src/queue.rs", QUEUE_V1),
            p.put("README.md", "# AgentPlaza\n"),
        ]],
    );
    p.land("agentplaza", &base);

    // Priya's lane: two turns.
    let change = p.lane(
        "agentplaza",
        107,
        "t51",
        "Spread reviews",
        &[
            vec![p.put("crates/apid/src/queue.rs", QUEUE_D1)],
            vec![
                p.put("crates/apid/src/queue.rs", QUEUE_D2),
                p.put(
                    "docs/reviews.md",
                    "# Reviews\n\nThe least busy reviewer goes first.\n",
                ),
            ],
        ],
    );
    p.agent_says(
        107,
        "intent.set",
        json!({ "change": change, "n": 1, "intents": [
            { "title": "Reviews go to the least busy reviewer", "kind": "feature", "covers": ["crates/apid/src/queue.rs::assign_reviews", "crates/apid/src/queue.rs::pick_reviewer"] },
            { "title": "Document it", "kind": "docs", "covers": ["docs/**"] }
        ] }),
    )
    .unwrap();
    // Omar, at the same time, also changes assign_reviews.
    let omar = p.lane(
        "agentplaza",
        112,
        "t53",
        "Log assignments",
        &[vec![p.put(
            "crates/apid/src/queue.rs",
            &QUEUE_V1.replace(
                "        out\n    }\n\n    fn old",
                "        eprintln!(\"assigned {}\", out.len());\n        out\n    }\n\n    fn old",
            ),
        )]],
    );

    let derived = Derived::new(p.dir.join("derived"));
    let tree = derived.change_tree(s, &format!("{change}.r1")).unwrap();
    println!("{}", serde_json::to_string_pretty(&tree).unwrap());

    assert_eq!(tree.files.len(), 2);
    let rs = tree
        .files
        .iter()
        .find(|f| f.path.ends_with("queue.rs"))
        .unwrap();
    assert_eq!(rs.pack.as_deref(), Some("rust"));
    let sym = |q: &str| {
        rs.symbols
            .iter()
            .find(|x| x.qualified.ends_with(q))
            .unwrap_or_else(|| {
                panic!(
                    "no {q} in {:?}",
                    rs.symbols.iter().map(|x| &x.qualified).collect::<Vec<_>>()
                )
            })
    };

    let assign = sym("fn assign_reviews");
    assert_eq!(assign.op, '~');
    assert_eq!(assign.qualified, "impl Queues › fn assign_reviews");
    assert!(
        assign
            .touches
            .iter()
            .any(|t| t.how == "calls" && t.to.ends_with("fn pick_reviewer")),
        "assign_reviews calls pick_reviewer: {:?}",
        assign.touches
    );

    let pick = sym("fn pick_reviewer");
    assert_eq!(pick.op, '+');
    assert!(
        pick.touches
            .iter()
            .any(|t| t.how == "used by" && t.to.ends_with("fn assign_reviews"))
    );

    // The rename keeps the lineage of the old definition.
    let weight = sym("fn weight");
    assert_eq!(weight.op, '>');
    assert_eq!(
        weight.from.as_deref(),
        Some("crates/apid/src/queue.rs::impl Queues › fn old_helper")
    );
    assert_eq!(
        weight.lineage,
        derived.lineage(
            "agentplaza",
            "crates/apid/src/queue.rs::impl Queues › fn old_helper"
        )
    );
    assert!(
        !rs.symbols.iter().any(|x| x.op == '-'),
        "a renamed definition is not also listed as removed"
    );

    // Provenance: pick_reviewer came from turn 1, the empty-list guard from turn 2.
    assert!(pick.provenance.iter().all(
        |r| r.source == format!("{change}.d1") && r.model.as_deref() == Some("claude-opus-5-5")
    ));
    assert!(
        assign
            .provenance
            .iter()
            .any(|r| r.source == format!("{change}.d1"))
    );
    assert!(
        assign
            .provenance
            .iter()
            .any(|r| r.source == format!("{change}.d2") && r.turn == Some(2))
    );

    // Intents group the definitions; the rename belongs to none.
    assert_eq!(tree.intents[0].items.len(), 2);
    assert_eq!(
        tree.intents[1].items,
        vec!["docs/reviews.md::section Reviews".to_string()]
    );
    assert_eq!(
        tree.unsorted,
        vec!["crates/apid/src/queue.rs::impl Queues › fn weight".to_string()]
    );

    // Divergence: Omar's open lane changes the same definition.
    assert_eq!(tree.divergence.len(), 1);
    assert_eq!(tree.divergence[0].change, omar);
    assert_eq!(
        tree.divergence[0].symbols,
        vec!["crates/apid/src/queue.rs::impl Queues › fn assign_reviews".to_string()]
    );

    // Only the packs the files needed were loaded, and a second derivation comes from the cache.
    assert_eq!(
        derived.runtime.loaded(),
        vec!["markdown".to_string(), "rust".to_string()]
    );
    let loads = derived.runtime.loads();
    let again = Derived::new(p.dir.join("derived"));
    let tree2 = again.change_tree(s, &format!("{change}.r1")).unwrap();
    assert_eq!(
        serde_json::to_value(&tree2).unwrap(),
        serde_json::to_value(&tree).unwrap()
    );
    assert_eq!(
        again.runtime.loads(),
        0,
        "the on-disk symbol cache answers without loading a pack"
    );
    assert_eq!(loads, 2);
    std::fs::remove_dir_all(&p.dir).ok();
}

const BRIEF_V1: &str = "# Edge caching for the plaza

## Question

Should nodes cache objects close to agents?

## Findings

- Most reads repeat within a task.
- Node disks are small.

## Recommendation

Cache on every node.
";

const BRIEF_D1: &str = "# Edge caching for the plaza

## Question

Should nodes cache objects close to agents?

## Findings

- Most reads repeat within a task: 83% in the sample of 40 tasks.
- Node disks are small.
- Caches warm in under a minute.

## Recommendation

Cache on every node.
";

const BRIEF_D2: &str = "# Edge caching for the plaza

## Question

Should nodes cache objects close to agents?

## Findings

- Most reads repeat within a task: 83% in the sample of 40 tasks.
- Node disks are small.
- Caches warm in under a minute.

## Recommendation

Cache on every node, capped at 2 GB, evicting the least recently used objects.

## Sources

- The plaza's own read logs, 40 tasks.
";

#[test]
fn a_research_brief_is_a_tree_of_sections_and_claims() {
    let p = plaza("tree-brief", &["research"]);
    p.add_packs(&["markdown"]);
    let base = p.lane(
        "research",
        115,
        "t60",
        "Start the brief",
        &[vec![p.put("briefs/edge-caching.md", BRIEF_V1)]],
    );
    p.land("research", &base);
    let change = p.lane(
        "research",
        115,
        "t61",
        "Measure and recommend",
        &[
            vec![p.put("briefs/edge-caching.md", BRIEF_D1)],
            vec![
                p.put("briefs/edge-caching.md", BRIEF_D2),
                p.put("data/reads.csv", "task,reads,repeats\nt1,120,101\n"),
            ],
        ],
    );

    let derived = Derived::new(p.dir.join("derived"));
    let tree = derived
        .change_tree(&p.store, &format!("{change}.r1"))
        .unwrap();
    println!("{}", serde_json::to_string_pretty(&tree).unwrap());

    let md = tree.files.iter().find(|f| f.path.ends_with(".md")).unwrap();
    assert_eq!(md.pack.as_deref(), Some("markdown"));
    let q: Vec<(&str, char)> = md
        .symbols
        .iter()
        .map(|s| (s.qualified.as_str(), s.op))
        .collect();
    let has = |name: &str, op: char| q.iter().any(|(n, o)| n.ends_with(name) && *o == op);
    assert!(has("section Recommendation", '~'), "{q:?}");
    assert!(has("section Sources", '+'), "{q:?}");
    assert!(has("item Caches warm in under a minute.", '+'), "{q:?}");
    assert!(
        has(
            "item Most reads repeat within a task: 83% in the sample of 40 tasks.",
            '+'
        ) || has(
            "item Most reads repeat within a task: 83% in the sample of 40 tasks.",
            '>'
        ),
        "{q:?}"
    );
    assert!(
        !q.iter().any(|(n, _)| n.ends_with("section Question")),
        "an untouched section is not listed: {q:?}"
    );
    // The new section came from the second turn.
    let sources = md
        .symbols
        .iter()
        .find(|s| s.qualified.ends_with("section Sources"))
        .unwrap();
    assert!(
        sources
            .provenance
            .iter()
            .all(|r| r.source == format!("{change}.d2"))
    );
    // A data file without a pack is still in the tree, as text.
    let csv = tree
        .files
        .iter()
        .find(|f| f.path.ends_with(".csv"))
        .unwrap();
    assert_eq!((csv.pack.as_deref(), csv.binary, csv.add), (None, false, 2));
    assert_eq!(tree.languages.get("text"), Some(&1));
    assert_eq!(
        derived.runtime.loaded(),
        vec!["markdown".to_string()],
        "no code pack was loaded for a document"
    );
    std::fs::remove_dir_all(&p.dir).ok();
}
