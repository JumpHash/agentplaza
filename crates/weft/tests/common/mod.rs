//! A small plaza for tests: an owner, a hub, certified agents, a repo, and packs on request.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use weft::keys::KeyPair;
use weft::model::Delta;
use weft::record::{Actor, Statement};
use weft::store::Store;

pub fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("weft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

pub struct Agent {
    pub key: KeyPair,
    pub actor: Actor,
}

pub struct Plaza {
    pub dir: PathBuf,
    pub store: Store,
    pub owner: KeyPair,
    pub hub_seed: String,
    pub agents: Vec<Agent>,
}

impl Plaza {
    pub fn agent(&self, cid: u64) -> &Agent {
        self.agents
            .iter()
            .find(|a| a.actor == Actor::Agent { cid })
            .expect("no such agent")
    }

    pub fn owner_says(&self, kind: &str, body: Value) -> Result<String, String> {
        self.store
            .append(Statement::new(kind, Actor::Owner, body).sign(&self.owner))
            .map(|(_, s)| s)
    }

    pub fn agent_says(&self, cid: u64, kind: &str, body: Value) -> Result<String, String> {
        let a = self.agent(cid);
        self.store
            .append(Statement::new(kind, a.actor.clone(), body).sign(&a.key))
            .map(|(_, s)| s)
    }

    pub fn hub_says(&self, kind: &str, body: Value) -> Result<String, String> {
        self.store.hub_append(kind, body).map(|(_, s)| s)
    }

    /// Adds the named packs from `crates/weft/packs`, fetching grammars from npm once (cached under the target dir).
    pub fn add_packs(&self, names: &[&str]) {
        for name in names {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("packs")
                .join(name);
            let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("weft-npm");
            let prepared = weft::packsrc::prepare(&dir, None, Some(&cache))
                .unwrap_or_else(|e| panic!("pack {name}: {e}"));
            self.store.blobs.put(&prepared.wasm).unwrap();
            self.store.blobs.put(&prepared.query).unwrap();
            self.owner_says("pack.add", serde_json::to_value(&prepared.pack).unwrap())
                .unwrap();
        }
    }

    pub fn put(&self, path: &str, text: &str) -> Delta {
        put(&self.store, path, text)
    }

    pub fn delete(&self, path: &str) -> Delta {
        Delta {
            path: path.into(),
            after: None,
            mode: 0o100644,
        }
    }

    /// An agent's lane through drafts and a seal: returns the change name.
    pub fn lane(
        &self,
        repo: &str,
        cid: u64,
        task: &str,
        title: &str,
        drafts: &[Vec<Delta>],
    ) -> String {
        let a = self.agent(cid);
        let change = self
            .store
            .open_lane(
                repo,
                "main",
                Some(task),
                title,
                a.actor.clone(),
                None,
                "normal",
            )
            .unwrap();
        // A revision holds the lane's whole difference from its base: each turn adds to the last.
        for i in 0..drafts.len() {
            let deltas = cumulative(&drafts[..=i]);
            self.agent_says(cid, "revision.draft", json!({ "change": change, "n": i + 1, "deltas": deltas, "tool_calls": [format!("ide_write#{}", i + 1)], "turn": i + 1, "model": "claude-opus-5-5" }))
                .unwrap();
        }
        self.agent_says(cid, "revision.seal", json!({ "change": change, "n": 1, "deltas": cumulative(drafts), "model": "claude-opus-5-5" })).unwrap();
        change
    }

    /// Queues and lands a sealed change at its first round.
    pub fn land(&self, repo: &str, change: &str) -> String {
        let (deltas, expected) = {
            let st = self.store.state();
            (
                st.revisions[&format!("{change}.r1")].deltas.clone(),
                st.repos[repo].head("main"),
            )
        };
        self.hub_says(
            "queue.add",
            json!({ "repo": repo, "ref": "main", "change": change, "n": 1 }),
        )
        .unwrap();
        self.hub_says("land", json!({ "repo": repo, "ref": "main", "change": change, "n": 1, "expected": expected, "deltas": deltas })).unwrap();
        format!("{change}.l")
    }
}

/// The deltas of several turns, later turns winning.
pub fn cumulative(drafts: &[Vec<Delta>]) -> Vec<Delta> {
    let mut by_path = std::collections::BTreeMap::new();
    for d in drafts.iter().flatten() {
        by_path.insert(d.path.clone(), d.clone());
    }
    by_path.into_values().collect()
}

pub fn put(store: &Store, path: &str, text: &str) -> Delta {
    Delta {
        path: path.into(),
        after: Some(store.blobs.put(text.as_bytes()).unwrap()),
        mode: 0o100644,
    }
}

/// An owner, a hub (`certify-agents`, `rule-for-owner`), agents Priya (107, builder), Nell (110, reviewer), Omar
/// (112, builder) and Ada (115, researcher), and the given repos in project pr5.
pub fn plaza(name: &str, repos: &[&str]) -> Plaza {
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
    let mut agents = Vec::new();
    for (cid, name, role) in [
        (107, "Priya", "builder"),
        (110, "Nell", "reviewer"),
        (112, "Omar", "builder"),
        (115, "Ada", "researcher"),
    ] {
        let key = KeyPair::generate();
        store
            .hub_append("cert.issue", json!({ "key": key.public(), "holder": { "actor": { "kind": "agent", "cid": cid }, "name": name, "role": role, "project": "pr5" } }))
            .unwrap();
        agents.push(Agent {
            key,
            actor: Actor::Agent { cid },
        });
    }
    let p = Plaza {
        dir,
        store,
        owner,
        hub_seed,
        agents,
    };
    for repo in repos {
        p.owner_says("repo.create", json!({ "repo": repo, "project": "pr5" }))
            .unwrap();
    }
    p
}
