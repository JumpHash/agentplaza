//! Landing: queues, merges (by lines, then by definitions), clashes sent back to their lanes, re-checks with a
//! speculative next item, commuting changes, stacked lanes, and reverts.

mod common;

use common::{Plaza, plaza};
use serde_json::json;
use weft::derived::Derived;
use weft::landing::{Lander, Options, Step};
use weft::state::ChangeState;

const LIB: &str = "pub fn add(a: u64, b: u64) -> u64 {
    a + b
}

pub fn sub(a: u64, b: u64) -> u64 {
    a - b
}
";

fn text(p: &Plaza, rev: &str, path: &str) -> String {
    String::from_utf8(p.store.read(rev, path).unwrap().unwrap()).unwrap()
}

fn head(p: &Plaza, repo: &str) -> String {
    p.store.state().repos[repo].head("main")
}

fn queue(p: &Plaza, repo: &str, change: &str) {
    let n = p.store.state().change(change).unwrap().round();
    p.hub_says(
        "queue.add",
        json!({ "repo": repo, "ref": "main", "change": change, "n": n }),
    )
    .unwrap();
}

fn setup(name: &str) -> (Plaza, Derived, Lander) {
    let p = plaza(name, &["calc"]);
    p.add_packs(&["rust"]);
    let base = p.lane(
        "calc",
        107,
        "t1",
        "The calculator",
        &[vec![
            p.put("src/lib.rs", LIB),
            p.put("README.md", "# calc\n"),
        ]],
    );
    let derived = Derived::new(p.dir.join("derived"));
    let lander = Lander::default();
    queue(&p, "calc", &base);
    assert!(matches!(
        lander
            .step(&p.store, &derived, "calc", "main", Options::default())
            .unwrap(),
        Step::Landed { .. }
    ));
    (p, derived, lander)
}

#[test]
fn two_new_functions_at_the_same_place_merge_by_definition() {
    let (p, derived, lander) = setup("land-symbols");
    // Both agents start from the same head and append a function at the end of the file.
    let mul = p.lane(
        "calc",
        107,
        "t2",
        "Multiply",
        &[vec![p.put(
            "src/lib.rs",
            &format!("{LIB}\npub fn mul(a: u64, b: u64) -> u64 {{\n    a * b\n}}\n"),
        )]],
    );
    let div = p.lane(
        "calc",
        112,
        "t3",
        "Divide",
        &[vec![p.put(
            "src/lib.rs",
            &format!("{LIB}\npub fn div(a: u64, b: u64) -> u64 {{\n    a / b\n}}\n"),
        )]],
    );
    queue(&p, "calc", &mul);
    queue(&p, "calc", &div);
    let first = lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap();
    assert!(
        matches!(&first, Step::Landed { merged, .. } if merged.is_empty()),
        "{first:?}"
    );
    let second = lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap();
    match &second {
        Step::Landed { merged, .. } => {
            assert_eq!(merged, &vec!["src/lib.rs (symbols)".to_string()])
        }
        other => panic!("{other:?}"),
    }
    let lib = text(&p, &head(&p, "calc"), "src/lib.rs");
    assert!(
        lib.contains("fn mul") && lib.contains("fn div") && lib.contains("fn add"),
        "{lib}"
    );
    assert!(lib.find("fn mul").unwrap() < lib.find("fn div").unwrap());
    // The landing is signed by the hub and says what was merged.
    let st = p.store.state();
    let c = st.change(&div).unwrap();
    let landed = c
        .records
        .iter()
        .map(|s| &st.records[s])
        .find(|r| r.attest.predicate_type == "landed")
        .unwrap();
    assert_eq!(landed.attest.predicate["merged"][0], "src/lib.rs (symbols)");
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", Options::default())
            .unwrap()
            .outcome(),
        "empty"
    );
    drop(st);
    std::fs::remove_dir_all(&p.dir).ok();
}

/// A batch of four lanes, the third breaking it: the batch's check fails on every prefix that holds `c.txt`, and the
/// check reads the candidate's tree, as apid's does. Bisection names `c` and it leaves with the failure; the others
/// land in order, and the batch that remains passes.
#[test]
fn four_lanes_in_a_batch_land_in_order_and_bisection_ejects_the_one_that_breaks_it() {
    let (p, derived, lander) = setup("land-batch");
    let lanes: Vec<String> = [
        ("a", "a.txt"),
        ("b", "b.txt"),
        ("c", "c.txt"),
        ("d", "d.txt"),
    ]
    .iter()
    .map(|(name, file)| {
        p.lane(
            "calc",
            107,
            &format!("t-{name}"),
            &format!("Lane {name}"),
            &[vec![p.put(file, &format!("{name}\n"))]],
        )
    })
    .collect();
    for lane in &lanes {
        queue(&p, "calc", lane);
    }
    let opts = Options {
        recheck: true,
        stall_ms: u64::MAX,
        batch: 4,
        ..Default::default()
    };
    let (mut landed, mut failed) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        match lander
            .step(&p.store, &derived, "calc", "main", opts)
            .unwrap()
        {
            Step::Landed { change, .. } => landed.push(change),
            Step::Failed { change, reason } => {
                assert!(reason.contains("bisect"), "{reason}");
                failed.push(change);
            }
            Step::Recheck { digest, .. } | Step::Waiting { digest, .. } => {
                let tree = lander.candidate_tree(&p.store, &digest).unwrap();
                lander.report(&digest, !tree.contains_key("c.txt"));
            }
            Step::Empty => break,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        landed,
        vec![lanes[0].clone(), lanes[1].clone(), lanes[3].clone()]
    );
    assert_eq!(failed, vec![lanes[2].clone()]);
    let h = head(&p, "calc");
    for file in ["a.txt", "b.txt", "d.txt"] {
        assert!(
            p.store.read(&h, file).unwrap().is_some(),
            "{file} is on main"
        );
    }
    assert!(p.store.read(&h, "c.txt").unwrap().is_none(), "c.txt is not");
    std::fs::remove_dir_all(&p.dir).ok();
}

#[test]
fn a_held_landing_waits_until_the_owner_approves_it() {
    let (p, derived, lander) = setup("land-hold");
    let before = head(&p, "calc");
    let docs = p.lane(
        "calc",
        107,
        "t2",
        "Docs",
        &[vec![p.put("README.md", "# calc\n\nheld\n")]],
    );
    queue(&p, "calc", &docs);
    let hold = Options {
        hold: true,
        ..Default::default()
    };
    for _ in 0..2 {
        match lander
            .step(&p.store, &derived, "calc", "main", hold)
            .unwrap()
        {
            Step::Held { change, .. } => assert_eq!(change, docs),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(head(&p, "calc"), before, "nothing landed while held");
    let approved = Options {
        approved: true,
        ..hold
    };
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", approved)
            .unwrap()
            .outcome(),
        "landed"
    );
    assert!(text(&p, &head(&p, "calc"), "README.md").contains("held"));
    std::fs::remove_dir_all(&p.dir).ok();
}

trait Outcome {
    fn outcome(&self) -> &'static str;
}
impl Outcome for Step {
    fn outcome(&self) -> &'static str {
        match self {
            Step::Empty => "empty",
            Step::Landed { .. } => "landed",
            Step::Recheck { .. } => "recheck",
            Step::Waiting { .. } => "waiting",
            Step::Held { .. } => "held",
            Step::Conflict { .. } => "conflict",
            Step::Failed { .. } => "failed",
        }
    }
}

#[test]
fn a_real_clash_goes_back_to_its_lane_marked_and_lands_once_resolved() {
    let (p, derived, lander) = setup("land-clash");
    let a = p.lane(
        "calc",
        107,
        "t2",
        "Saturating add",
        &[vec![p.put(
            "src/lib.rs",
            &LIB.replace("a + b", "a.saturating_add(b)"),
        )]],
    );
    let b = p.lane(
        "calc",
        112,
        "t3",
        "Checked add",
        &[vec![p.put(
            "src/lib.rs",
            &LIB.replace("a + b", "a.checked_add(b).unwrap()"),
        )]],
    );
    queue(&p, "calc", &a);
    queue(&p, "calc", &b);
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", Options::default())
            .unwrap()
            .outcome(),
        "landed"
    );
    match lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap()
    {
        Step::Conflict {
            change,
            files,
            symbols,
        } => {
            assert_eq!(change, b);
            assert_eq!(files, vec!["src/lib.rs".to_string()]);
            assert_eq!(symbols, vec!["src/lib.rs::fn add".to_string()]);
        }
        other => panic!("{other:?}"),
    }
    // The lane moved onto the head, with a hub draft that marks the clash.
    let (base, draft, state) = {
        let st = p.store.state();
        let c = st.change(&b).unwrap();
        (c.base.clone(), c.drafts.last().unwrap().clone(), c.state)
    };
    assert_eq!(base, head(&p, "calc"));
    assert_eq!(state, ChangeState::Open);
    let marked = text(&p, &draft, "src/lib.rs");
    assert!(
        marked.contains("<<<<<<< this change")
            && marked.contains("checked_add")
            && marked.contains("saturating_add"),
        "{marked}"
    );
    // Omar resolves it: round 2, from the new base.
    let resolved = LIB.replace("a + b", "a.checked_add(b).unwrap_or(u64::MAX)");
    let d = vec![p.put("src/lib.rs", &resolved)];
    p.agent_says(
        112,
        "revision.draft",
        json!({ "change": b, "n": 3, "deltas": d, "turn": 3 }),
    )
    .unwrap();
    p.agent_says(
        112,
        "revision.seal",
        json!({ "change": b, "n": 2, "deltas": d }),
    )
    .unwrap();
    queue(&p, "calc", &b);
    match lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap()
    {
        Step::Landed { merged, .. } => assert!(
            merged.is_empty(),
            "a resolution from the head lands as it is"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(text(&p, &head(&p, "calc"), "src/lib.rs"), resolved);
    std::fs::remove_dir_all(&p.dir).ok();
}

#[test]
fn rechecks_hold_a_merged_head_while_the_next_is_merged_on_top() {
    let (p, derived, lander) = setup("land-recheck");
    let opts = Options {
        recheck: true,
        stall_ms: u64::MAX,
        batch: 2,
        ..Default::default()
    };
    let docs = p.lane(
        "calc",
        107,
        "t2",
        "Docs",
        &[vec![p.put("README.md", "# calc\n\nAdds and subtracts.\n")]],
    );
    let mul = p.lane(
        "calc",
        112,
        "t3",
        "Multiply",
        &[vec![p.put(
            "src/lib.rs",
            &format!("{LIB}\npub fn mul(a: u64, b: u64) -> u64 {{\n    a * b\n}}\n"),
        )]],
    );
    let neg = p.lane(
        "calc",
        107,
        "t4",
        "Negate docs",
        &[vec![
            p.put("NOTES.md", "negative numbers are not supported\n"),
        ]],
    );
    queue(&p, "calc", &docs);
    // docs is based on the head: it lands without a re-check.
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", opts)
            .unwrap()
            .outcome(),
        "landed"
    );
    queue(&p, "calc", &mul);
    queue(&p, "calc", &neg);
    // mul started before docs landed: merged, so it waits for its re-check. The batch is mul and neg (merged on top of
    // it), and the batch's check is its top's: neg's.
    let digest = match lander
        .step(&p.store, &derived, "calc", "main", opts)
        .unwrap()
    {
        Step::Recheck { change, digest } => {
            assert_eq!(change, neg);
            digest
        }
        other => panic!("{other:?}"),
    };
    let cands = lander.candidates("calc", "main");
    assert_eq!(cands.len(), 2);
    assert!(cands[1].speculative && cands[1].change == neg);
    // The checks read the candidate's tree.
    let tree = lander.candidate_tree(&p.store, &digest).unwrap();
    assert!(
        tree.contains_key("README.md")
            && tree.contains_key("src/lib.rs")
            && tree.contains_key("NOTES.md")
    );
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", opts)
            .unwrap()
            .outcome(),
        "waiting"
    );
    assert!(lander.report(&digest, true));
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", opts)
            .unwrap()
            .outcome(),
        "landed"
    );
    // neg's candidate is now on the head's tree: the batch's check covered it, so it needs no check of its own.
    let next = lander.candidates("calc", "main");
    assert_eq!(next.len(), 1);
    assert!(!next[0].speculative && next[0].checked == Some(true));
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", opts)
            .unwrap()
            .outcome(),
        "landed"
    );
    let h = head(&p, "calc");
    assert!(
        text(&p, &h, "src/lib.rs").contains("fn mul")
            && text(&p, &h, "NOTES.md").contains("negative")
            && text(&p, &h, "README.md").contains("Adds")
    );
    std::fs::remove_dir_all(&p.dir).ok();
}

#[test]
fn a_stalled_head_lets_a_commuting_change_pass() {
    let (p, derived, lander) = setup("land-commute");
    // slow starts now; main moves under it before it is queued, so it lands rebased and needs a re-check.
    let slow = p.lane(
        "calc",
        112,
        "t3",
        "Slow",
        &[vec![p.put(
            "src/lib.rs",
            &LIB.replace("a - b", "a.saturating_sub(b)"),
        )]],
    );
    let moved = p.lane(
        "calc",
        107,
        "t2",
        "Docs",
        &[vec![p.put("README.md", "# calc\n\nmoved\n")]],
    );
    queue(&p, "calc", &moved);
    lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap();
    // quick starts from the new head and touches only a new file.
    let quick = p.lane(
        "calc",
        107,
        "t4",
        "Quick",
        &[vec![p.put("CHANGELOG.md", "- quick\n")]],
    );
    queue(&p, "calc", &slow);
    queue(&p, "calc", &quick);
    let opts = Options {
        recheck: true,
        stall_ms: 0,
        ..Default::default()
    };
    let first = lander
        .step(&p.store, &derived, "calc", "main", opts)
        .unwrap();
    // The head waits on its re-check; with a zero stall, quick (disjoint, on the head) passes it.
    assert!(
        matches!(first, Step::Landed { ref change, .. } if *change == quick),
        "{first:?}"
    );
    assert_eq!(
        p.store.state().change(&slow).unwrap().state,
        ChangeState::Queued
    );
    // The head's candidate is merged again onto the moved head, and still waits for its check.
    assert_eq!(
        lander
            .step(
                &p.store,
                &derived,
                "calc",
                "main",
                Options {
                    recheck: true,
                    stall_ms: u64::MAX,
                    ..Default::default()
                }
            )
            .unwrap()
            .outcome(),
        "recheck"
    );
    std::fs::remove_dir_all(&p.dir).ok();
}

#[test]
fn a_stacked_lane_moves_onto_the_change_below_when_it_lands() {
    let (p, derived, lander) = setup("land-stack");
    let below = p.lane(
        "calc",
        107,
        "t2",
        "Multiply",
        &[vec![p.put(
            "src/lib.rs",
            &format!("{LIB}\npub fn mul(a: u64, b: u64) -> u64 {{\n    a * b\n}}\n"),
        )]],
    );
    // t5 runs after t2: its lane is stacked on t2's.
    let change = p
        .store
        .open_lane(
            "calc",
            "main",
            Some("t5"),
            "Square",
            p.agent(112).actor.clone(),
            Some(&below),
            "normal",
        )
        .unwrap();
    let below_rev = format!("{below}.r1");
    assert_eq!(p.store.state().change(&change).unwrap().base, below_rev);
    let sq = format!(
        "{}\npub fn square(a: u64) -> u64 {{\n    mul(a, a)\n}}\n",
        text(&p, &below_rev, "src/lib.rs")
    );
    p.agent_says(
        112,
        "revision.draft",
        json!({ "change": change, "n": 1, "deltas": [p.put("src/lib.rs", &sq)] }),
    )
    .unwrap();
    // Meanwhile main moves, so `below` lands merged.
    let docs = p.lane(
        "calc",
        107,
        "t3",
        "Docs",
        &[vec![p.put("README.md", "# calc v2\n")]],
    );
    queue(&p, "calc", &docs);
    lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap();
    queue(&p, "calc", &below);
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", Options::default())
            .unwrap()
            .outcome(),
        "landed"
    );
    let st = p.store.state();
    let c = st.change(&change).unwrap();
    assert_eq!(
        c.base,
        format!("{below}.l"),
        "the stacked lane now starts from the landed change"
    );
    let restacked = c.drafts.last().unwrap().clone();
    drop(st);
    let lib = text(&p, &restacked, "src/lib.rs");
    assert!(lib.contains("fn square") && lib.contains("fn mul"));
    assert!(text(&p, &restacked, "README.md").contains("v2"));
    std::fs::remove_dir_all(&p.dir).ok();
}

#[test]
fn a_revert_undoes_one_change_and_keeps_later_work() {
    let (p, derived, lander) = setup("land-revert");
    let bad = p.lane(
        "calc",
        107,
        "t2",
        "Wrapping sub",
        &[vec![p.put(
            "src/lib.rs",
            &LIB.replace("a - b", "a.wrapping_sub(b)"),
        )]],
    );
    queue(&p, "calc", &bad);
    lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap();
    let later = p.lane(
        "calc",
        112,
        "t3",
        "Docs",
        &[vec![p.put("README.md", "# calc\n\nlater\n")]],
    );
    queue(&p, "calc", &later);
    lander
        .step(&p.store, &derived, "calc", "main", Options::default())
        .unwrap();
    let rev = weft::landing::revert(
        &p.store,
        &derived,
        &bad,
        weft::record::Actor::Hub,
        "the post-land suite failed",
    )
    .unwrap();
    assert_eq!(
        lander
            .step(&p.store, &derived, "calc", "main", Options::default())
            .unwrap()
            .outcome(),
        "landed"
    );
    let h = head(&p, "calc");
    assert_eq!(text(&p, &h, "src/lib.rs"), LIB);
    assert!(text(&p, &h, "README.md").contains("later"));
    let st = p.store.state();
    let c = st.change(&rev).unwrap();
    assert_eq!(
        (c.kind.as_str(), c.reverts),
        ("revert", Some(st.change(&bad).unwrap().id))
    );
    std::fs::remove_dir_all(&p.dir).ok();
}
