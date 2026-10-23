//! The release flow against a Weft of its own: an in-process `weft serve` on a scratch store, a landed change, fake
//! builds and a fake CI result (the stand-ins for 7.5's pools and for the checks of 9.4). Each test is one release's life.

use std::path::PathBuf;

use serde_json::{Value, json};
use weft::client::Client;
use weft::keys::KeyPair;
use weft::model::Delta;
use weft::record::{Actor, Statement};
use weft::store::Store;

use super::entity::{Release, ReleaseState, Store as Releases};
use super::flow::{self, Builder, Built, Cut, Deps, Ledger};
use crate::recipes::runs::{RunState, Runs};

const TOKEN: &str = "release-test-token";
const CARGO: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[package.metadata.weft]\ntargets = [\"linux-x86_64\", \"macos-aarch64\"]\n";

/// Weft's client as the flow reads and writes through it.
struct WeftLedger(Client);

impl Ledger for WeftLedger {
    fn read(&self, path: &str) -> Result<Value, String> {
        self.0.get(path)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.0.post(path, body)
    }

    fn statement(&self, kind: &str, body: Value) -> Result<Value, String> {
        self.0.post("/hub", &json!({ "kind": kind, "body": body }))
    }

    fn upload(&self, bytes: &[u8]) -> Result<weft::hash::Hash, String> {
        let hash = weft::hash::Hash::of(bytes);
        self.0
            .upload(&[hash], &|_: &weft::hash::Hash| Ok(bytes.to_vec()))?;
        Ok(hash)
    }
}

/// A builder that builds every target and records what it built.
struct FakeBuilder;

impl Builder for FakeBuilder {
    fn build(&self, release: &Release, target: &str) -> Result<Built, String> {
        Ok(Built {
            bytes: format!("{} for {target}", release.version).into_bytes(),
            pool: "fake-pool".into(),
            base: "fake-base".into(),
            builder: "fake-builder".into(),
            command: "cargo build --release".into(),
        })
    }
}

/// A builder whose machine for one target is gone.
struct BrokenBuilder;

impl Builder for BrokenBuilder {
    fn build(&self, _release: &Release, target: &str) -> Result<Built, String> {
        Err(format!("the pool for {target} went away"))
    }
}

fn put(store: &Store, path: &str, text: &str) -> Delta {
    Delta {
        path: path.into(),
        after: Some(store.blobs.put(text.as_bytes()).unwrap()),
        mode: 0o100644,
    }
}

/// A Weft on a scratch store with one landed change (a Cargo package declaring two targets), served in-process.
/// Returns the client the flow uses, the server's handle (it serves while the handle lives) and the directory.
fn lab(name: &str) -> (Client, apz_http::Handle, PathBuf) {
    let dir = std::env::temp_dir().join(format!("apid-release-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let owner = KeyPair::generate();
    let hub = KeyPair::generate();
    let hub_seed = hub.seed_hex();
    let signing = KeyPair::generate();
    let agent = KeyPair::generate();
    {
        let store = Store::init(
            &dir,
            "test plaza",
            &owner,
            hub,
            &["certify-agents", "rule-for-owner"],
        )
        .unwrap();
        store
            .hub_append(
                "cert.issue",
                json!({ "key": agent.public(), "holder": { "actor": { "kind": "agent", "cid": 107 }, "name": "Priya", "role": "builder", "project": "app" } }),
            )
            .unwrap();
        store
            .append(
                Statement::new(
                    "repo.create",
                    Actor::Owner,
                    json!({ "repo": "app", "project": "app" }),
                )
                .sign(&owner),
            )
            .unwrap();
        let writer = Actor::Agent { cid: 107 };
        let change = store
            .open_lane(
                "app",
                "main",
                Some("t1"),
                "The app",
                writer.clone(),
                None,
                "normal",
            )
            .unwrap();
        let deltas = vec![
            put(&store, "Cargo.toml", CARGO),
            put(&store, "src/main.rs", "fn main() {}\n"),
        ];
        store
            .append(Statement::new("revision.draft", writer.clone(), json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["ide_write#1"], "turn": 1, "model": "claude-opus-5-5" })).sign(&agent))
            .unwrap();
        store
            .append(Statement::new("revision.seal", writer, json!({ "change": change, "n": 1, "deltas": deltas, "model": "claude-opus-5-5" })).sign(&agent))
            .unwrap();
        let landed_deltas = store.state().revisions[&format!("{change}.r1")]
            .deltas
            .clone();
        let head = store.state().repos["app"].head("main");
        store
            .hub_append(
                "queue.add",
                json!({ "repo": "app", "ref": "main", "change": change, "n": 1 }),
            )
            .unwrap();
        store
            .hub_append("land", json!({ "repo": "app", "ref": "main", "change": change, "n": 1, "expected": head, "deltas": landed_deltas }))
            .unwrap();
    }
    // The store is closed; Weft is served on it as the hub does (`weft serve`, in process).
    let weft = weft::server::Hub::open_with(
        &dir,
        Some(KeyPair::from_hex(&hub_seed).unwrap()),
        Some(signing),
    )
    .unwrap();
    let token_file = dir.join("internal.token");
    std::fs::write(&token_file, TOKEN).unwrap();
    let handle =
        weft::server::spawn(weft, 0, apz_http::auth::InternalToken::file(&token_file)).unwrap();
    let client = Client::new(
        &format!("http://127.0.0.1:{}/v1", handle.port()),
        None,
        vec![
            (apz_http::auth::HEADER.to_string(), TOKEN.to_string()),
            (weft::wire::CALLER.to_string(), "hub".to_string()),
        ],
    )
    .unwrap();
    (client, handle, dir)
}

/// The dependencies of one test: its Weft, the recipe runs and the release store.
struct World {
    ledger: WeftLedger,
    runs: Runs,
    releases: Releases,
}

impl World {
    fn new(client: Client) -> World {
        World {
            ledger: WeftLedger(client),
            runs: Runs::default(),
            releases: Releases::default(),
        }
    }

    fn deps(&self) -> Deps<'_> {
        Deps {
            weft: &self.ledger,
            runs: &self.runs,
            releases: &self.releases,
        }
    }
}

fn cut(version: &str) -> Cut {
    Cut {
        version: version.into(),
        changes: vec![],
        ..Cut::default()
    }
}

fn life(world: &World, id: &str) -> ReleaseState {
    world.releases.get(id).unwrap().state
}

#[test]
fn a_release_is_cut_built_per_target_checked_published_and_promoted() {
    let (client, _handle, _dir) = lab("life");
    let world = World::new(client);
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("v0.1.0")).unwrap();
    assert_eq!(drafted.state, ReleaseState::Draft);
    assert_eq!(drafted.lane, "main");
    let run = drafted.run.clone().expect("the draft starts its run");
    let view = d.runs.view(&run).unwrap();
    assert_eq!(view.run.recipe, "release");
    assert_eq!(
        view.walk.steps["build"].items, 2,
        "one build item per declared target"
    );
    assert_eq!(view.run.state, RunState::Running);

    let cut_candidate = flow::candidate(&d, &drafted.id, &FakeBuilder, Some(1)).unwrap();
    assert_eq!(cut_candidate.state, ReleaseState::Candidate);
    assert_eq!(cut_candidate.number, Some(1));
    assert_eq!(cut_candidate.artifacts.len(), 2, "one artifact per target");
    let targets: Vec<&str> = cut_candidate
        .artifacts
        .iter()
        .map(|a| a.target.as_str())
        .collect();
    assert_eq!(targets.len(), 2);
    assert!(
        targets[0].ends_with("linux-x86_64") && targets[1].ends_with("macos-aarch64"),
        "{targets:?}"
    );
    for a in &cut_candidate.artifacts {
        assert!(!a.artifact.is_empty() && !a.digest.is_empty() && a.size > 0);
        assert_eq!(a.pool, "fake-pool");
        assert_eq!(a.run, run);
    }
    assert_eq!(
        d.runs.view(&run).unwrap().run.state,
        RunState::Passed,
        "the candidate step completes the run"
    );

    // Publication waits for the checks on the commit (the stand-in CI result is recorded below).
    let early = flow::publish(&d, &drafted.id).unwrap_err();
    assert!(early.contains("cannot be published"), "{early}");
    let checked = flow::checks(
        &d,
        &drafted.id,
        true,
        "fake CI: every check passed on the release commit",
    )
    .unwrap();
    assert_eq!(checked.state, ReleaseState::Candidate);
    assert_eq!(checked.verdicts.len(), 1);
    assert_eq!(checked.verdicts[0].source, "ci");

    let published = flow::publish(&d, &drafted.id).unwrap();
    assert_eq!(published.state, ReleaseState::Published);
    let promoted = flow::promote(&d, &drafted.id, "production").unwrap();
    assert_eq!(promoted.state, ReleaseState::Promoted);
    assert_eq!(promoted.promotions[0].environment, "production");
    let yanked = flow::yank(&d, &drafted.id, "crashes on start").unwrap();
    assert_eq!(yanked.state, ReleaseState::Yanked);
    assert_eq!(life(&world, &drafted.id), ReleaseState::Yanked);
}

#[test]
fn a_refused_build_leaves_the_draft_and_the_next_cut_runs_again() {
    let (client, _handle, _dir) = lab("refused");
    let world = World::new(client);
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("v0.2.0")).unwrap();
    let first_run = drafted.run.clone().unwrap();

    let err = flow::candidate(&d, &drafted.id, &BrokenBuilder, None).unwrap_err();
    assert!(err.contains("went away"), "{err}");
    assert_eq!(
        life(&world, &drafted.id),
        ReleaseState::Draft,
        "a refused build leaves the draft"
    );
    assert_eq!(d.runs.view(&first_run).unwrap().run.state, RunState::Failed);

    let cut_candidate = flow::candidate(&d, &drafted.id, &FakeBuilder, None).unwrap();
    assert_eq!(cut_candidate.state, ReleaseState::Candidate);
    assert_ne!(
        cut_candidate.run.clone().unwrap(),
        first_run,
        "the next attempt is a run of its own"
    );
    assert_eq!(
        d.runs
            .view(cut_candidate.run.as_deref().unwrap())
            .unwrap()
            .run
            .state,
        RunState::Passed
    );
}

#[test]
fn a_candidate_is_not_cut_from_a_candidate_and_unlanded_changes_are_refused() {
    let (client, _handle, _dir) = lab("moves");
    let world = World::new(client);
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("v0.3.0")).unwrap();
    flow::candidate(&d, &drafted.id, &FakeBuilder, None).unwrap();
    let again = flow::candidate(&d, &drafted.id, &FakeBuilder, None).unwrap_err();
    assert!(again.contains("a candidate is cut from a draft"), "{again}");
    // A qualified path, checked by Weft: a candidate with no checks cannot be qualified and then published.
    flow::qualify(&d, &drafted.id).unwrap();
    assert_eq!(life(&world, &drafted.id), ReleaseState::Qualifying);
    let unlanded = Cut {
        version: "v0.3.1".into(),
        changes: vec!["w-99".into()],
        ..Cut::default()
    };
    // Weft has no such change to land (or it is not landed): the draft is refused before anything is proposed.
    assert!(flow::draft(&d, "app", "app", &unlanded).is_err());
    assert!(d.releases.list("app").iter().all(|r| r.version != "v0.3.1"));
}
