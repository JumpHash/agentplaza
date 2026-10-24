//! Review records as Weft statements (PLAN 8.4): a review run's outcome and its findings are `review.outcome`
//! statements the hub signs. They travel with a change's git export (as notes on the exported commit) and come back
//! with an import into a fresh repository, where the store checks every carried review before it records it. A
//! review the owner's key signs, or one that was tampered with, is refused.

mod common;

use std::path::Path;
use std::process::Command;

use common::{Plaza, plaza, tmp};
use serde_json::{Value, json};
use weft::derived::Derived;
use weft::export::{Exporter, Keys};
use weft::import::{Source, import};
use weft::keys::KeyPair;
use weft::landing::{Lander, Options, Step};
use weft::record::{Actor, Statement};
use weft::store::Store;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A tree of the given `mktree` listing, in a repository.
fn mktree(dir: &Path, listing: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .args(["mktree"])
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("git runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(listing.as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_string()
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The outcome of a review the hub recorded: one confirmed finding (with its proof and the seats that raised and
/// verified it) and one dropped finding (with its reason).
fn review_body(change: &str, revision: &str) -> Value {
    json!({
        "change": change,
        "run": "review-1",
        "revision": revision,
        "round": 1,
        "recipe": "code",
        "decision": "pass",
        "reasons": ["the confirmed finding was fixed in the next round"],
        "models": [
            { "seat": "builder", "model": "claude-opus-5-5", "session": "s-builder" },
            { "seat": "Nell", "model": "claude-sonnet-5", "session": "s-nell" },
            { "seat": "Ada", "model": "gpt-5", "session": "s-ada" }
        ],
        "findings": [
            {
                "id": "f1", "symbol": "src/main.rs::main", "severity": "major", "round": 1,
                "scenario": "an empty name makes the first argument an empty string and the greeting panics",
                "kind": "panic", "verdict": "confirmed",
                "proof": "test empty_name_panics fails on this revision",
                "by": { "seat": "Nell", "model": "claude-sonnet-5", "session": "s-nell" },
                "verified_by": { "seat": "Ada", "model": "gpt-5", "session": "s-ada" }
            },
            {
                "id": "f2", "symbol": "src/main.rs::main", "severity": "minor", "round": 1,
                "scenario": "a name with spaces is passed in one argument",
                "kind": "logic", "verdict": "dropped",
                "reason": "the shell splits the arguments before main sees them: not a defect",
                "by": { "seat": "Nell", "model": "claude-sonnet-5", "session": "s-nell" },
                "verified_by": { "seat": "Ada", "model": "gpt-5", "session": "s-ada" }
            }
        ]
    })
}

/// A plaza with a bare git remote connected for the `app` repository. Returns the plaza and the remote's path.
fn connected(name: &str) -> (Plaza, std::path::PathBuf) {
    let p = plaza(name, &["app"]);
    let remote = p.dir.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    p.owner_says(
        "connection.add",
        weft::connect::connection_body(
            "app",
            "origin",
            &format!("file://{}", remote.display()),
            "git",
            "main",
            "Irfan Saric",
            "owner@example.com",
            None,
        ),
    )
    .unwrap();
    (p, remote)
}

/// Priya's lane with two intents, sealed, reviewed by the hub (when `review` is set), queued and landed, then
/// exported to the connected remote. Returns the change's name and the git id of its exported last commit.
fn landed_and_exported(p: &Plaza, derived: &Derived, review: bool) -> (String, String) {
    let lane = p.lane(
        "app",
        107,
        "t70",
        "Greet by name",
        &[vec![
            p.put("src/main.rs", "fn main() {\n    let name = std::env::args().nth(1).unwrap_or(\"world\".into());\n    println!(\"hello, {name}\");\n}\n"),
            p.put("README.md", "# app\n\nGreets you.\n"),
        ]],
    );
    p.agent_says(107, "intent.set", json!({ "change": lane, "n": 1, "intents": [
        { "title": "Greet people by name", "summary": "The first argument names who to greet.", "kind": "feature", "covers": ["src/**"] },
        { "title": "Document greeting by name", "kind": "docs", "covers": ["README.md"] }
    ] }))
    .unwrap();
    if review {
        p.hub_says("review.outcome", review_body(&lane, &format!("{lane}.r1")))
            .unwrap();
    }
    p.hub_says(
        "queue.add",
        json!({ "repo": "app", "ref": "main", "change": lane, "n": 1 }),
    )
    .unwrap();
    let lander = Lander::default();
    loop {
        match lander
            .step(&p.store, derived, "app", "main", Options::default())
            .unwrap()
        {
            Step::Landed { .. } => continue,
            Step::Empty => break,
            other => panic!("{other:?}"),
        }
    }
    let report = Exporter::default()
        .export(
            &p.store,
            derived,
            "app",
            "origin",
            Keys {
                token: None,
                signing: None,
            },
            false,
        )
        .unwrap();
    assert_eq!(report.state, "pushed", "{report:?}");
    let st = p.store.state();
    let landed = st.change(&lane).unwrap();
    let last = landed.exported["origin"].last().unwrap().git.clone();
    (lane, last)
}

/// A fresh repository on a store with the same owner (and so the same plaza), holding `app`.
fn fresh_repo(p: &Plaza, name: &str) -> Store {
    let dir = tmp(name);
    let store = Store::init(
        &dir,
        "fresh plaza",
        &p.owner,
        KeyPair::generate(),
        &["certify-agents", "rule-for-owner"],
    )
    .unwrap();
    store
        .append(
            Statement::new(
                "repo.create",
                Actor::Owner,
                json!({ "repo": "app", "project": "pr5", "description": "" }),
            )
            .sign(&p.owner),
        )
        .unwrap();
    store
}

fn imported_change<'a>(st: &'a weft::state::State, git_id: &str) -> &'a weft::state::Change {
    st.changes
        .values()
        .find(|c| c.import.as_ref().is_some_and(|i| i.git == git_id))
        .expect("the commit was imported as a change")
}

#[test]
fn a_reviewed_landing_travels_with_its_export_into_a_fresh_repository() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let (p, remote) = connected("review-travel");
    let derived = Derived::new(p.dir.join("derived"));
    let (_lane, exported) = landed_and_exported(&p, &derived, true);

    // The remote carries the review as a note on the exported commit, and git is still sound.
    git(&remote, &["fsck", "--strict", "--no-dangling"]);
    assert!(git(&remote, &["show-ref"]).contains("refs/notes/weft-reviews"));

    // A fresh repository imports the remote: the review comes with the commit it reviewed.
    let fresh = fresh_repo(&p, "review-travel-fresh");
    let url = format!("file://{}", remote.display());
    let report = import(
        &fresh,
        "app",
        &Source::open(&url, None, "main").unwrap(),
        "main",
        "main",
    )
    .unwrap();
    assert_eq!(report.reviews, 1, "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");

    let st = fresh.state();
    let change = imported_change(&st, &exported);
    assert_eq!(change.reviews.len(), 1);
    let outcome = &change.reviews[0].outcome;
    assert_eq!(outcome.decision, "pass");
    assert_eq!(outcome.recipe, "code");
    assert_eq!(outcome.models.len(), 3);
    assert_eq!(outcome.models[1].model, "claude-sonnet-5");
    assert_eq!(outcome.findings.len(), 2);
    let confirmed = &outcome.findings[0];
    assert_eq!(confirmed.verdict, "confirmed");
    assert_eq!(
        confirmed.proof,
        "test empty_name_panics fails on this revision"
    );
    assert_eq!(confirmed.by.as_ref().unwrap().seat, "Nell");
    assert_eq!(confirmed.verified_by.as_ref().unwrap().model, "gpt-5");
    let dropped = &outcome.findings[1];
    assert_eq!(dropped.verdict, "dropped");
    assert!(dropped.reason.starts_with("the shell splits"));
    assert!(
        outcome.origin.is_some(),
        "a carried review says where it came from"
    );
    // The fresh log holds the review as a hub statement, and replays to the same state.
    let record = fresh.records().unwrap();
    assert!(
        record
            .iter()
            .any(|r| r.signed.statement().unwrap().kind == "review.outcome")
    );
    fresh.verify_deep().unwrap();
    assert_eq!(weft::server::reviews_view(&st, "app").len(), 1);
}

#[test]
fn a_repository_with_no_review_statements_imports_and_reads_as_before() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let (p, remote) = connected("review-absent");
    let derived = Derived::new(p.dir.join("derived"));
    let (_lane, exported) = landed_and_exported(&p, &derived, false);
    git(&remote, &["fsck", "--strict", "--no-dangling"]);
    assert!(!git(&remote, &["show-ref"]).contains("refs/notes/weft-reviews"));

    let fresh = fresh_repo(&p, "review-absent-fresh");
    let url = format!("file://{}", remote.display());
    let report = import(
        &fresh,
        "app",
        &Source::open(&url, None, "main").unwrap(),
        "main",
        "main",
    )
    .unwrap();
    assert_eq!((report.reviews, report.refused.len()), (0, 0));
    let st = fresh.state();
    assert!(imported_change(&st, &exported).reviews.is_empty());
    assert!(weft::server::reviews_view(&st, "app").is_empty());
}

#[test]
fn a_carried_review_that_was_tampered_with_is_refused_on_import() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let (p, remote) = connected("review-tamper");
    let derived = Derived::new(p.dir.join("derived"));
    let (_lane, exported) = landed_and_exported(&p, &derived, true);

    // Someone edits the note's review: the proof is changed in the signed statement, and the notes are re-committed.
    let notes_head = git(&remote, &["rev-parse", "refs/notes/weft-reviews"]);
    let note = git(
        &remote,
        &["show", &format!("refs/notes/weft-reviews:{exported}")],
    );
    let tampered = note.replace(
        "test empty_name_panics fails on this revision",
        "nothing fails",
    );
    assert_ne!(tampered, note);
    let blob = {
        let path = p.dir.join("tampered-note.json");
        std::fs::write(&path, &tampered).unwrap();
        git(&remote, &["hash-object", "-w", path.to_str().unwrap()])
    };
    let tree = mktree(&remote, &format!("100644 blob {blob}\t{exported}\n"));
    let commit = git(
        &remote,
        &[
            "commit-tree",
            &tree,
            "-p",
            &notes_head,
            "-m",
            "Review records",
        ],
    );
    git(&remote, &["update-ref", "refs/notes/weft-reviews", &commit]);

    let fresh = fresh_repo(&p, "review-tamper-fresh");
    let url = format!("file://{}", remote.display());
    let report = import(
        &fresh,
        "app",
        &Source::open(&url, None, "main").unwrap(),
        "main",
        "main",
    )
    .unwrap();
    assert_eq!(report.reviews, 0, "{report:?}");
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(report.refused[0].contains("does not verify"), "{report:?}");
    let st = fresh.state();
    assert!(imported_change(&st, &exported).reviews.is_empty());
}

#[test]
fn a_review_statement_from_a_non_hub_key_is_refused() {
    let p = plaza("review-keys", &["app"]);
    let lane = p.lane("app", 107, "t71", "Greet", &[vec![p.put("a.md", "# a\n")]]);
    let body = review_body(&lane, &format!("{lane}.r1"));

    let err = p.owner_says("review.outcome", body.clone()).unwrap_err();
    assert!(err.contains("hub's key"), "{err}");
    let err = p
        .agent_says(107, "review.outcome", body.clone())
        .unwrap_err();
    assert!(err.contains("only the hub"), "{err}");
    let err = p
        .store
        .console_owner_append("review.outcome", body.clone())
        .unwrap_err();
    assert!(err.contains("hub's key"), "{err}");

    // The hub's own key records it, and a finding without its scenario is refused.
    p.hub_says("review.outcome", body.clone()).unwrap();
    let mut no_scenario = body.clone();
    no_scenario["findings"][0]["scenario"] = json!("");
    let err = p.hub_says("review.outcome", no_scenario).unwrap_err();
    assert!(err.contains("no scenario"), "{err}");
    let mut no_proof = body.clone();
    no_proof["findings"][0]["proof"] = json!("");
    assert!(p.hub_says("review.outcome", no_proof).is_err());
    let st = p.store.state();
    assert_eq!(st.change(&lane).unwrap().reviews.len(), 1);
}

#[test]
fn a_review_record_that_was_altered_does_not_verify() {
    let p = plaza("review-altered", &["app"]);
    let lane = p.lane("app", 107, "t72", "Greet", &[vec![p.put("a.md", "# a\n")]]);
    p.hub_says("review.outcome", review_body(&lane, &format!("{lane}.r1")))
        .unwrap();
    let record = p
        .store
        .records()
        .unwrap()
        .into_iter()
        .find(|r| r.signed.statement().unwrap().kind == "review.outcome")
        .unwrap();
    // The statement's bytes change; the signature stays what the hub made.
    let mut altered = record.signed.clone();
    altered.stmt = String::from_utf8(altered.stmt)
        .unwrap()
        .replace("the shell splits", "the shell joins")
        .into_bytes();
    let err = p.store.append(altered).unwrap_err();
    assert!(err.contains("verif") || err.contains("signature"), "{err}");
    // And the log on disk still verifies, with the review as the hub wrote it.
    p.store.verify_deep().unwrap();
}

#[test]
fn a_carried_review_must_be_signed_under_the_repository_owner() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let (p, remote) = connected("review-owner");
    let derived = Derived::new(p.dir.join("derived"));
    let (_lane, _exported) = landed_and_exported(&p, &derived, true);
    // A repository whose owner is someone else refuses the review, and the import does not fail for it.
    let dir = tmp("review-owner-fresh");
    let other = KeyPair::generate();
    let fresh = Store::init(
        &dir,
        "other plaza",
        &other,
        KeyPair::generate(),
        &["certify-agents", "rule-for-owner"],
    )
    .unwrap();
    fresh
        .append(
            Statement::new(
                "repo.create",
                Actor::Owner,
                json!({ "repo": "app", "project": "pr5", "description": "" }),
            )
            .sign(&other),
        )
        .unwrap();
    let url = format!("file://{}", remote.display());
    let report = import(
        &fresh,
        "app",
        &Source::open(&url, None, "main").unwrap(),
        "main",
        "main",
    )
    .unwrap();
    assert_eq!(report.reviews, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(report.refused[0].contains("another owner"), "{report:?}");
}

#[test]
fn a_review_travels_with_adoption_onto_the_adopting_lane() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let (p, remote) = connected("review-adopt");
    let derived = Derived::new(p.dir.join("derived"));
    let (_lane, exported) = landed_and_exported(&p, &derived, true);

    // Someone pushes a commit of their own to the remote's main. Another hub had reviewed that commit: its note comes
    // with the push, so Weft adopts the commit with the review it carries.
    let clone = p.dir.join("hand-clone");
    git(
        &p.dir,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(&clone, &["config", "user.name", "Irfan Saric"]);
    git(&clone, &["config", "user.email", "owner@example.com"]);
    std::fs::write(clone.join("README.md"), "# app\n\nEdited by hand.\n").unwrap();
    git(&clone, &["commit", "-q", "-am", "Edit the readme by hand"]);
    git(&clone, &["push", "-q", "origin", "HEAD:main"]);
    let hand = git(&clone, &["rev-parse", "HEAD"]);
    let blob = git(
        &remote,
        &["rev-parse", &format!("refs/notes/weft-reviews:{exported}")],
    );
    let notes_head = git(&remote, &["rev-parse", "refs/notes/weft-reviews"]);
    let tree = mktree(
        &remote,
        &format!("100644 blob {blob}\t{exported}\n100644 blob {blob}\t{hand}\n"),
    );
    let notes = git(
        &remote,
        &[
            "commit-tree",
            &tree,
            "-p",
            &notes_head,
            "-m",
            "Review records",
        ],
    );
    git(&remote, &["update-ref", "refs/notes/weft-reviews", &notes]);

    let adopted = weft::import::adopt(&p.store, "app", "origin", None).unwrap();
    assert_eq!(adopted.commits, vec!["Edit the readme by hand".to_string()]);
    assert_eq!(
        (adopted.reviews, adopted.refused.len()),
        (1, 0),
        "{adopted:?}"
    );
    let st = p.store.state();
    let lane = st.change(&adopted.change).unwrap();
    assert_eq!(lane.reviews.len(), 1);
    assert_eq!(
        lane.reviews[0].outcome.findings[0].scenario,
        "an empty name makes the first argument an empty string and the greeting panics"
    );
}
