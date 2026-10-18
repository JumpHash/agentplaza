//! A change's life through the store, and what the store refuses.

use std::path::PathBuf;

use serde_json::json;
use weft::keys::KeyPair;
use weft::model::Delta;
use weft::record::{Actor, Statement};
use weft::state::ChangeState;
use weft::store::Store;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("weft-flow-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

struct Plaza {
    dir: PathBuf,
    store: Store,
    owner: KeyPair,
    hub_seed: String,
    priya: KeyPair,
    nell: KeyPair,
}

fn plaza(name: &str) -> Plaza {
    let dir = tmp(name);
    let owner = KeyPair::generate();
    let hub = KeyPair::generate();
    let hub_seed = hub.seed_hex();
    let store = Store::init(
        &dir,
        "test plaza",
        &owner,
        hub,
        &["certify-agents", "rule-for-owner"],
    )
    .unwrap();
    let priya = KeyPair::generate();
    let nell = KeyPair::generate();
    for (key, cid, name, role) in [
        (&priya, 107, "Priya", "builder"),
        (&nell, 110, "Nell", "reviewer"),
    ] {
        store
            .hub_append(
                "cert.issue",
                json!({ "key": key.public(), "holder": { "actor": { "kind": "agent", "cid": cid }, "name": name, "role": role, "project": "pr5" } }),
            )
            .unwrap();
    }
    store
        .append(
            Statement::new(
                "repo.create",
                Actor::Owner,
                json!({ "repo": "agentplaza", "project": "pr5" }),
            )
            .sign(&owner),
        )
        .unwrap();
    Plaza {
        dir,
        store,
        owner,
        hub_seed,
        priya,
        nell,
    }
}

fn put(store: &Store, path: &str, text: &str) -> Delta {
    Delta {
        path: path.into(),
        after: Some(store.blobs.put(text.as_bytes()).unwrap()),
        mode: 0o100644,
    }
}

#[test]
fn a_change_goes_from_lane_to_landed_with_signed_records() {
    let p = plaza("life");
    let s = &p.store;
    let priya = Actor::Agent { cid: 107 };
    let change = s
        .open_lane(
            "agentplaza",
            "main",
            Some("t51"),
            "Fix the confirmed bugs",
            priya.clone(),
            None,
            "normal",
        )
        .unwrap();
    assert_eq!(change, "w-1");

    // Two turns: two drafts, then the seal.
    let d1 = vec![put(
        s,
        "crates/apid/src/nodes.rs",
        "fn memory_mib() -> Value {\n    Value::from(\"text\")\n}\n",
    )];
    s.append(
        Statement::new(
            "revision.draft",
            priya.clone(),
            json!({ "change": change, "n": 1, "deltas": d1, "tool_calls": ["ide_write#1"] }),
        )
        .sign(&p.priya),
    )
    .unwrap();
    let d2 = vec![put(
        s,
        "crates/apid/src/nodes.rs",
        "fn memory_mib() -> Value {\n    Value::Null\n}\n",
    )];
    s.append(Statement::new("revision.draft", priya.clone(), json!({ "change": change, "n": 2, "deltas": d2.clone(), "tool_calls": ["ide_write#3"] })).sign(&p.priya)).unwrap();
    s.append(
        Statement::new(
            "revision.seal",
            priya.clone(),
            json!({ "change": change, "n": 1, "deltas": d2 }),
        )
        .sign(&p.priya),
    )
    .unwrap();
    s.append(
        Statement::new(
            "intent.set",
            priya.clone(),
            json!({ "change": change, "n": 1, "intents": [{ "title": "Nodes screen works on Mac and Windows hubs", "kind": "fix", "covers": ["crates/apid/src/nodes.rs"] }] }),
        )
        .sign(&p.priya),
    )
    .unwrap();

    let rev = format!("{change}.r1");
    let digest = s.state().revisions[&rev].digest;
    let attest = |pt: &str, actor: Actor| {
        Statement::new(
            "attest",
            actor,
            json!({ "revision": rev, "digest": digest, "predicate_type": pt, "predicate": {} }),
        )
    };
    s.append(attest("written", priya.clone()).sign(&p.priya))
        .unwrap();
    // The author never reviews their own change; another agent does.
    assert!(
        s.append(attest("review", priya.clone()).sign(&p.priya))
            .unwrap_err()
            .contains("own change")
    );
    s.append(attest("review", Actor::Agent { cid: 110 }).sign(&p.nell))
        .unwrap();
    // Only the hub signs the gate; only the owner rules (here from the console, through the hub's capability).
    assert!(
        s.append(attest("gate", Actor::Agent { cid: 110 }).sign(&p.nell))
            .is_err()
    );
    s.hub_append("attest", json!({ "revision": rev, "digest": digest, "predicate_type": "gate", "predicate": { "outcome": "owner" } })).unwrap();
    s.console_owner_append("attest", json!({ "revision": rev, "digest": digest, "predicate_type": "ruling", "predicate": { "decision": "pass" } })).unwrap();

    s.hub_append(
        "queue.add",
        json!({ "repo": "agentplaza", "ref": "main", "change": change, "n": 1 }),
    )
    .unwrap();
    assert_eq!(s.state().changes[&1].state, ChangeState::Queued);
    // Landing must name the head it moves from.
    assert!(s.hub_append("land", json!({ "repo": "agentplaza", "ref": "main", "change": change, "n": 1, "expected": "w-9.l", "deltas": [] })).is_err());
    let landed = s.state().revisions[&rev].deltas.clone();
    s.hub_append("land", json!({ "repo": "agentplaza", "ref": "main", "change": change, "n": 1, "expected": "root", "deltas": landed })).unwrap();

    let st = s.state();
    assert_eq!(st.changes[&1].state, ChangeState::Landed);
    assert_eq!(st.repos["agentplaza"].head("main"), format!("{change}.l"));
    let records: Vec<_> = st.changes[&1]
        .records
        .iter()
        .map(|seq| st.records[seq].attest.predicate_type.clone())
        .collect();
    assert_eq!(records, ["written", "review", "gate", "ruling"]);
    assert!(
        st.records.values().any(|r| r.via_console),
        "the console ruling is marked as such"
    );
    drop(st);
    let text = s
        .read(&format!("{change}.l"), "crates/apid/src/nodes.rs")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(text).unwrap().contains("Value::Null"));

    // Everything verifies, and a reopen gives the same state.
    let report = s.verify_deep().unwrap();
    assert!(report.records > 10);
    let reopened = Store::open(&p.dir, Some(KeyPair::from_hex(&p.hub_seed).unwrap())).unwrap();
    assert_eq!(
        reopened.state().repos["agentplaza"].head("main"),
        format!("{change}.l")
    );
    std::fs::remove_dir_all(&p.dir).ok();
}

#[test]
fn the_store_refuses_what_its_signers_may_not_say() {
    let p = plaza("refuse");
    let s = &p.store;
    let priya = Actor::Agent { cid: 107 };
    let change = s
        .open_lane(
            "agentplaza",
            "main",
            Some("t52"),
            "Reword docs",
            priya.clone(),
            None,
            "normal",
        )
        .unwrap();

    // An agent key cannot sign as another agent, or as the hub.
    let as_nell = Statement::new(
        "revision.draft",
        Actor::Agent { cid: 110 },
        json!({ "change": change, "n": 1, "deltas": [] }),
    );
    assert!(
        s.append(as_nell.sign(&p.priya))
            .unwrap_err()
            .contains("cannot sign as")
    );
    // An agent writes only its own lane.
    let nell_writes = Statement::new(
        "revision.draft",
        Actor::Agent { cid: 110 },
        json!({ "change": change, "n": 1, "deltas": [] }),
    );
    assert!(
        s.append(nell_writes.sign(&p.nell))
            .unwrap_err()
            .contains("only agent:107")
    );
    // Only the owner creates repos; only the hub opens lanes.
    assert!(
        s.append(
            Statement::new("repo.create", priya.clone(), json!({ "repo": "x" })).sign(&p.priya)
        )
        .is_err()
    );
    assert!(
        s.append(Statement::new("lane.open", priya.clone(), json!({})).sign(&p.priya))
            .unwrap_err()
            .contains("only the hub")
    );
    // Bad paths and draft numbers out of order are refused.
    let bad = Statement::new(
        "revision.draft",
        priya.clone(),
        json!({ "change": change, "n": 1, "deltas": [{ "path": "../etc/passwd", "after": null }] }),
    );
    assert!(
        s.append(bad.sign(&p.priya))
            .unwrap_err()
            .contains("bad path")
    );
    let skip = Statement::new(
        "revision.draft",
        priya.clone(),
        json!({ "change": change, "n": 3, "deltas": [] }),
    );
    assert!(
        s.append(skip.sign(&p.priya))
            .unwrap_err()
            .contains("next draft is 1")
    );
    // A revoked key signs nothing more.
    s.hub_append(
        "cert.revoke",
        json!({ "key": p.priya.id(), "reason": "retired" }),
    )
    .unwrap();
    let after = Statement::new(
        "revision.draft",
        priya,
        json!({ "change": change, "n": 1, "deltas": [] }),
    );
    assert!(
        s.append(after.sign(&p.priya))
            .unwrap_err()
            .contains("revoked")
    );
    // An unknown key signs nothing.
    let stranger = KeyPair::generate();
    assert!(
        s.append(
            Statement::new("repo.create", Actor::Owner, json!({ "repo": "y" })).sign(&stranger)
        )
        .unwrap_err()
        .contains("unknown key")
    );
    // Nothing refused reached the log.
    assert!(s.verify_deep().is_ok());
    let _ = &p.owner;
    std::fs::remove_dir_all(&p.dir).ok();
}
