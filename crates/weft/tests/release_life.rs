//! A release's life as Weft statements (PLAN 22.2): every state is a statement the hub signs, a candidate holds its
//! artifacts and digest, a release is published only once its checks passed (or it was qualified), and its life is
//! read back after the log is exported (copied) and imported (opened again).

mod common;

use std::path::Path;

use common::{Plaza, plaza};
use serde_json::{Value, json};
use weft::keys::KeyPair;
use weft::record::Actor;
use weft::store::Store;

const TARGETS: [&str; 2] = ["linux-x86_64", "macos-arm64"];

/// A plaza with one landed change and a release `v0.1.0` proposed from it, declaring two targets.
fn proposed(name: &str) -> Plaza {
    let p = plaza(name, &["app"]);
    let change = p.lane(
        "app",
        107,
        "t1",
        "The app",
        &[vec![p.put("src/main.rs", "fn main() {}\n")]],
    );
    p.land("app", &change);
    propose(&p, "v0.1.0");
    p
}

fn propose(p: &Plaza, version: &str) {
    let mut body = weft::release::proposal(&p.store, "app", version, None, None).unwrap();
    body["targets"] = json!(TARGETS);
    p.hub_says("release.propose", body).unwrap();
}

/// One artifact per target, put by the hub. Returns their ids.
fn build(p: &Plaza, version: &str) -> Vec<String> {
    let head = p.store.state().repos["app"].head("main");
    TARGETS
        .iter()
        .map(|target| {
            let bytes = format!("a binary for {target} at {version}").into_bytes();
            let body = weft::release::artifact_body(
                &p.store,
                "app",
                "app",
                Some(version),
                target,
                &bytes,
                &head,
                "node:n1/build-vm",
                "cargo build --release",
            )
            .unwrap();
            let id = body["id"].as_str().unwrap().to_string();
            p.hub_says("artifact.put", body).unwrap();
            id
        })
        .collect()
}

fn candidate(p: &Plaza, version: &str, artifacts: &[String]) -> Result<String, String> {
    let digest = weft::release::release_digest(&p.store.state(), "app", version, artifacts)?;
    p.hub_says(
        "release.candidate",
        json!({ "repo": "app", "version": version, "artifacts": artifacts, "digest": digest, "number": 1 }),
    )
}

fn mv(kind: &str, version: &str, extra: Value) -> (String, Value) {
    let mut body = json!({ "repo": "app", "version": version });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    (kind.to_string(), body)
}

fn say(p: &Plaza, (kind, body): (String, Value)) -> Result<String, String> {
    p.hub_says(&kind, body)
}

fn life(store: &Store, version: &str) -> weft::state::ReleaseLife {
    store.state().release("app", version).unwrap().life.clone()
}

fn ci(p: &Plaza, version: &str, verdict: &str) -> Result<String, String> {
    say(
        p,
        mv(
            "release.verdict",
            version,
            json!({ "source": "ci", "verdict": verdict, "reason": "checks on the release commit" }),
        ),
    )
}

#[test]
fn a_candidate_holds_its_artifacts_and_digest_and_the_hub_signs_it() {
    let p = proposed("rel-candidate");
    let artifacts = build(&p, "v0.1.0");
    candidate(&p, "v0.1.0", &artifacts).unwrap();
    let got = life(&p.store, "v0.1.0");
    assert_eq!(got.state, "candidate");
    let (cand, _) = got.candidate.expect("the candidate statement is there");
    assert_eq!(cand.artifacts, artifacts);
    assert_eq!(cand.number, Some(1));
    let records = p.store.records().unwrap();
    let last = records
        .iter()
        .rev()
        .find(|r| r.signed.statement().unwrap().kind == "release.candidate")
        .unwrap();
    assert_eq!(last.signed.statement().unwrap().actor, Actor::Hub);
}

#[test]
fn a_candidate_needs_every_declared_target_and_the_digest_of_its_artifacts() {
    let p = proposed("rel-targets");
    let artifacts = build(&p, "v0.1.0");
    let one = artifacts[..1].to_vec();
    assert!(
        candidate(&p, "v0.1.0", &one)
            .unwrap_err()
            .contains("no artifact for target")
    );
    let forged = p.hub_says(
        "release.candidate",
        json!({ "repo": "app", "version": "v0.1.0", "artifacts": artifacts, "digest": weft::hash::Hash::of(b"other") }),
    );
    assert!(forged.unwrap_err().contains("digest"));
    assert_eq!(life(&p.store, "v0.1.0").state, "draft");
}

#[test]
fn publish_waits_for_passing_checks_and_then_promotion_and_yank_follow_the_life() {
    let p = proposed("rel-life");
    let artifacts = build(&p, "v0.1.0");
    candidate(&p, "v0.1.0", &artifacts).unwrap();
    assert!(
        say(&p, mv("release.publish", "v0.1.0", json!({})))
            .unwrap_err()
            .contains("cannot be published")
    );
    ci(&p, "v0.1.0", "passed").unwrap();
    assert_eq!(life(&p.store, "v0.1.0").state, "candidate");
    say(&p, mv("release.publish", "v0.1.0", json!({}))).unwrap();
    assert_eq!(life(&p.store, "v0.1.0").state, "published");
    assert!(
        say(&p, mv("release.promote", "v0.1.0", json!({})))
            .unwrap_err()
            .contains("environment or store track")
    );
    say(
        &p,
        mv(
            "release.promote",
            "v0.1.0",
            json!({ "environment": "production" }),
        ),
    )
    .unwrap();
    let promoted = life(&p.store, "v0.1.0");
    assert_eq!(promoted.state, "promoted");
    assert_eq!(promoted.promotions.len(), 1);
    assert!(
        say(&p, mv("release.yank", "v0.1.0", json!({})))
            .unwrap_err()
            .contains("says why")
    );
    say(
        &p,
        mv(
            "release.yank",
            "v0.1.0",
            json!({ "reason": "crashes on start" }),
        ),
    )
    .unwrap();
    assert_eq!(life(&p.store, "v0.1.0").state, "yanked");
}

#[test]
fn a_failed_candidate_is_not_patched_and_a_new_one_is_cut() {
    let p = proposed("rel-failed");
    let artifacts = build(&p, "v0.1.0");
    candidate(&p, "v0.1.0", &artifacts).unwrap();
    ci(&p, "v0.1.0", "failed").unwrap();
    assert_eq!(life(&p.store, "v0.1.0").state, "failed");
    assert!(candidate(&p, "v0.1.0", &artifacts).is_err());
    assert!(say(&p, mv("release.publish", "v0.1.0", json!({}))).is_err());
    propose(&p, "v0.1.1");
    let next = build(&p, "v0.1.1");
    candidate(&p, "v0.1.1", &next).unwrap();
    assert_eq!(life(&p.store, "v0.1.1").state, "candidate");
}

#[test]
fn a_qualified_candidate_publishes_without_checks_and_a_newer_publish_supersedes_the_older() {
    let p = proposed("rel-qualify");
    let artifacts = build(&p, "v0.1.0");
    candidate(&p, "v0.1.0", &artifacts).unwrap();
    say(&p, mv("release.qualify", "v0.1.0", json!({}))).unwrap();
    assert_eq!(life(&p.store, "v0.1.0").state, "qualifying");
    say(
        &p,
        mv(
            "release.verdict",
            "v0.1.0",
            json!({ "source": "qualification", "verdict": "passed" }),
        ),
    )
    .unwrap();
    assert_eq!(life(&p.store, "v0.1.0").state, "qualified");
    say(&p, mv("release.publish", "v0.1.0", json!({}))).unwrap();

    propose(&p, "v0.1.1");
    let next = build(&p, "v0.1.1");
    candidate(&p, "v0.1.1", &next).unwrap();
    ci(&p, "v0.1.1", "passed").unwrap();
    say(&p, mv("release.publish", "v0.1.1", json!({}))).unwrap();
    assert_eq!(life(&p.store, "v0.1.0").state, "superseded");
    assert_eq!(life(&p.store, "v0.1.1").state, "published");
}

#[test]
fn the_life_is_read_back_after_the_log_is_exported_and_imported() {
    let p = proposed("rel-export");
    let artifacts = build(&p, "v0.1.0");
    candidate(&p, "v0.1.0", &artifacts).unwrap();
    ci(&p, "v0.1.0", "passed").unwrap();
    say(&p, mv("release.publish", "v0.1.0", json!({}))).unwrap();
    say(
        &p,
        mv(
            "release.promote",
            "v0.1.0",
            json!({ "environment": "staging" }),
        ),
    )
    .unwrap();

    // The export is the log's directory; the import opens a copy of it, which replays every statement.
    let copy = p.dir.with_extension("imported");
    let _ = std::fs::remove_dir_all(&copy);
    copy_dir(&p.dir, &copy);
    let imported = Store::open(&copy, Some(KeyPair::from_hex(&p.hub_seed).unwrap())).unwrap();
    let got = life(&imported, "v0.1.0");
    assert_eq!(got.state, "promoted");
    assert_eq!(got.candidate.unwrap().0.artifacts, artifacts);
    assert_eq!(got.promotions[0].0, "staging");
    let kinds: Vec<String> = imported
        .records()
        .unwrap()
        .iter()
        .map(|r| r.signed.statement().unwrap().kind)
        .filter(|k| k.starts_with("release."))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "release.propose",
            "release.candidate",
            "release.verdict",
            "release.publish",
            "release.promote"
        ]
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
