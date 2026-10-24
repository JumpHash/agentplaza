//! The release flow against a Weft of its own: an in-process `weft serve` on a scratch store, a landed change, fake
//! builds and a fake CI result (the stand-ins for 7.5's pools and for the checks of 9.4). Each test is one release's life.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use weft::client::Client;
use weft::keys::KeyPair;
use weft::model::Delta;
use weft::record::{Actor, Statement};
use weft::store::Store;

use super::builder::build_target;
use super::builder::tests::{FakeHost, STACK, linux_node};
use super::entity::{Release, ReleaseState, Store as Releases};
use super::flow::{self, Builder, Built, Cut, Deps, Ledger, Outcome};
use super::lanes::{self, Agents, Chain};
use super::signing::{Notary, Signing};
use crate::recipes::runs::{RunState, Runs};
use crate::review_graph::{Reviews, Run, RunState as ReviewState};
use apz_blobs::Blobs;
use std::sync::Arc;

const TOKEN: &str = "release-test-token";
/// The lock file of the test package: 9.3's SBOM is read from it at the revision the release is cut from.
pub(super) const CARGO_LOCK: &str = "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.219\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"5f0e2c6ed6606019b4e29e69dbaae37b4f5b6fc1c0a2e2ef3b6f7f4b4d2f7a10\"\n";
const CARGO: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[package.metadata.weft]\ntargets = [\"linux-x86_64\", \"macos-aarch64\"]\n";

/// Weft's client as the flow reads and writes through it.
pub(super) struct WeftLedger(Client);

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

    fn bytes(&self, path: &str) -> Result<Vec<u8>, String> {
        self.0.get_bytes(path)
    }

    fn blob(&self, hash: &weft::hash::Hash) -> Result<Vec<u8>, String> {
        let mut found = None;
        self.0.download(&[*hash], &mut |h, bytes| {
            if h == hash {
                found = Some(bytes.to_vec());
            }
            Ok(())
        })?;
        found.ok_or_else(|| format!("Weft did not send blob {}", hash.short()))
    }
}

/// A builder that builds every target and records what it built.
struct FakeBuilder;

impl Builder for FakeBuilder {
    fn build(&self, release: &Release, target: &str) -> Result<Outcome, String> {
        Ok(Outcome::Built(Built {
            bytes: format!("{} for {target}", release.version).into_bytes(),
            pool: "fake-pool".into(),
            base: "fake-base".into(),
            builder: "fake-builder".into(),
            command: "cargo build --release".into(),
        }))
    }
}

/// A builder whose machine for one target is gone.
struct BrokenBuilder;

impl Builder for BrokenBuilder {
    fn build(&self, _release: &Release, target: &str) -> Result<Outcome, String> {
        Err(format!("the pool for {target} went away"))
    }
}

/// The production builder's placement and fork (`builder::build_target`) over a fake fleet: the project declares a
/// Linux target, and a Linux pool is in service; the Mac target has no declaration and so no pool.
struct FleetBuilder {
    host: FakeHost,
    stack: crate::project_stack::Stack,
    placements: Vec<crate::targets::Placement>,
}

impl Builder for FleetBuilder {
    fn build(&self, release: &Release, target: &str) -> Result<Outcome, String> {
        build_target(
            &self.host,
            &release.project,
            &release.repo,
            &release.from,
            &self.stack,
            &self.placements,
            target,
        )
    }
}

/// The review chain of a test: the backport's task is bound to its change, and its review run is stored as the review
/// graph stores one, running until the test settles it.
struct FakeChain<'a> {
    reviews: &'a Reviews,
    tasks: std::cell::Cell<u64>,
    bound: std::cell::RefCell<BTreeMap<String, String>>,
}

impl<'a> FakeChain<'a> {
    fn new(reviews: &'a Reviews) -> FakeChain<'a> {
        FakeChain {
            reviews,
            tasks: std::cell::Cell::new(0),
            bound: std::cell::RefCell::new(BTreeMap::new()),
        }
    }
}

impl Chain for FakeChain<'_> {
    fn task(&self, _project: &str, _title: &str, _worker: u64) -> Result<String, String> {
        let n = self.tasks.get() + 1;
        self.tasks.set(n);
        Ok(format!("task-{n}"))
    }

    fn bind(&self, task: &str, change: &str) {
        self.bound
            .borrow_mut()
            .insert(task.to_string(), change.to_string());
    }

    fn review(&self, task: &str) -> Option<String> {
        let change = self.bound.borrow().get(task)?.clone();
        let id = format!("rv-{task}");
        self.reviews.put_run(Run {
            id: id.clone(),
            task: task.to_string(),
            project: "app".into(),
            worker: 107,
            change,
            revision: String::new(),
            recipe: "code".into(),
            round: 1,
            rounds: 3,
            kind: "code".into(),
            state: ReviewState::Running,
            nodes: Vec::new(),
            rules: Vec::new(),
            owner_asked: false,
            outcome: None,
            tree_blob: None,
            summary: String::new(),
            purpose: "review".into(),
            repo: String::new(),
            models: Vec::new(),
            created_ms: 1,
            updated_ms: 1,
        });
        Some(id)
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
pub(super) fn lab(name: &str) -> (Client, apz_http::Handle, PathBuf, String, String) {
    lab_with(name, &[])
}

/// [`lab`], with `files` added to the landed change (the build files of other ecosystems: a release declares their targets).
pub(super) fn lab_with(
    name: &str,
    files: &[(&str, &str)],
) -> (Client, apz_http::Handle, PathBuf, String, String) {
    let dir = std::env::temp_dir().join(format!("apid-release-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let owner = KeyPair::generate();
    let hub = KeyPair::generate();
    let hub_seed = hub.seed_hex();
    let signing = KeyPair::generate();
    let agent = KeyPair::generate();
    let agent_seed = agent.seed_hex();
    let change;
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
        change = store
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
        // The change's intent: its notes come from it when no review ran on it.
        store
            .hub_append(
                "intent.set",
                json!({ "change": change, "n": 1, "intents": [{ "title": "Add the app entry point", "summary": "main prints hello", "kind": "feature" }] }),
            )
            .unwrap();
        // The defaults, each replaced by a file of the same path in `files`.
        let defaults = [
            ("Cargo.toml", CARGO),
            ("Cargo.lock", CARGO_LOCK),
            ("src/main.rs", "fn main() {}\n"),
        ];
        let mut deltas: Vec<Delta> = defaults
            .iter()
            .filter(|(path, _)| !files.iter().any(|(f, _)| f == path))
            .map(|(path, text)| put(&store, path, text))
            .collect();
        deltas.extend(files.iter().map(|(path, text)| put(&store, path, text)));
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
    (client, handle, dir, change, agent_seed)
}

/// The dependencies of one test: its Weft, the recipe runs, the review graph's store and the release store.
pub(super) struct World {
    pub(super) ledger: WeftLedger,
    pub(super) runs: Runs,
    pub(super) reviews: Reviews,
    pub(super) releases: Releases,
    pub(super) signing: TestSigning,
}

impl World {
    pub(super) fn new(client: Client, name: &str) -> World {
        let dir =
            std::env::temp_dir().join(format!("apid-release-blobs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let blobs = Arc::new(Blobs::open(&dir).expect("the object store opens"));
        World {
            ledger: WeftLedger(client),
            runs: Runs::default(),
            reviews: Reviews::new(blobs),
            releases: Releases::default(),
            signing: TestSigning::new(),
        }
    }

    pub(super) fn deps(&self) -> Deps<'_> {
        Deps {
            weft: &self.ledger,
            runs: &self.runs,
            reviews: &self.reviews,
            releases: &self.releases,
            signing: &self.signing,
        }
    }
}

/// The release step's keys in a test: generated here, never the owner's or the vault's.
pub(super) struct TestSigning {
    hub: String,
    android: String,
    windows: String,
    pub(super) notary: RecordingNotary,
}

impl TestSigning {
    pub(super) fn new() -> TestSigning {
        TestSigning {
            hub: KeyPair::generate().seed_hex(),
            android: KeyPair::generate().seed_hex(),
            windows: KeyPair::generate().seed_hex(),
            notary: RecordingNotary::default(),
        }
    }
}

impl Signing for TestSigning {
    fn hub(&self) -> Result<KeyPair, String> {
        KeyPair::from_hex(&self.hub)
    }

    fn platform(&self, scheme: &str) -> Result<KeyPair, String> {
        match scheme {
            "apksigner-standin" => KeyPair::from_hex(&self.android),
            "authenticode-standin" => KeyPair::from_hex(&self.windows),
            other => Err(format!("no test key for {other}")),
        }
    }

    fn notary(&self) -> &dyn Notary {
        &self.notary
    }
}

/// A notary that records what it was sent and issues a ticket per digest.
#[derive(Default)]
pub(super) struct RecordingNotary {
    pub(super) submitted: std::sync::Mutex<Vec<String>>,
}

impl Notary for RecordingNotary {
    fn submit(&self, sha256: &str) -> Result<String, String> {
        self.submitted.lock().unwrap().push(sha256.to_string());
        Ok(format!("ticket-{}", &sha256[..12]))
    }

    fn accepted(&self, ticket: &str, sha256: &str) -> bool {
        ticket == format!("ticket-{}", &sha256[..12])
            && self.submitted.lock().unwrap().iter().any(|s| s == sha256)
    }
}

pub(super) fn cut(version: &str) -> Cut {
    Cut {
        version: Some(version.into()),
        changes: vec![],
        ..Cut::default()
    }
}

fn life(world: &World, id: &str) -> ReleaseState {
    world.releases.get(id).unwrap().state
}

#[test]
fn a_release_is_cut_built_per_target_checked_published_and_promoted() {
    let (client, _handle, _dir, _change, _agent) = lab("life");
    let world = World::new(
        client,
        "a_release_is_cut_built_per_target_checked_published_and_promoted",
    );
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
    let (client, _handle, _dir, _change, _agent) = lab("refused");
    let world = World::new(
        client,
        "a_refused_build_leaves_the_draft_and_the_next_cut_runs_again",
    );
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
    let (client, _handle, _dir, _change, _agent) = lab("moves");
    let world = World::new(
        client,
        "a_candidate_is_not_cut_from_a_candidate_and_unlanded_changes_are_refused",
    );
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("v0.3.0")).unwrap();
    flow::candidate(&d, &drafted.id, &FakeBuilder, None).unwrap();
    let again = flow::candidate(&d, &drafted.id, &FakeBuilder, None).unwrap_err();
    assert!(again.contains("a candidate is cut from a draft"), "{again}");
    // A qualified path, checked by Weft: a candidate with no checks cannot be qualified and then published.
    flow::qualify(&d, &drafted.id).unwrap();
    assert_eq!(life(&world, &drafted.id), ReleaseState::Qualifying);
    let unlanded = Cut {
        version: Some("v0.3.1".into()),
        changes: vec!["w-99".into()],
        ..Cut::default()
    };
    // Weft has no such change to land (or it is not landed): the draft is refused before anything is proposed.
    assert!(flow::draft(&d, "app", "app", &unlanded).is_err());
    assert!(d.releases.list("app").iter().all(|r| r.version != "v0.3.1"));
}

#[test]
fn a_change_without_a_review_contributes_its_intents_to_the_notes() {
    let (client, _handle, _dir, change, _agent) = lab("intents");
    let world = World::new(client, "intents_notes");
    let d = world.deps();
    let cut_with_change = Cut {
        version: Some("v0.4.0".into()),
        changes: vec![change],
        ..Cut::default()
    };
    let drafted = flow::draft(&d, "app", "app", &cut_with_change).unwrap();
    assert_eq!(drafted.changes.len(), 1);
    assert_eq!(
        drafted.changes[0].review_run, None,
        "no review ran on this change"
    );
    assert!(drafted.notes.contains("## The app"), "{}", drafted.notes);
    assert!(
        drafted.notes.contains("- Add the app entry point"),
        "{}",
        drafted.notes
    );
    assert!(
        drafted.notes.contains("main prints hello"),
        "{}",
        drafted.notes
    );
}

#[test]
fn a_review_graph_run_writes_the_notes_of_its_change_before_the_intents() {
    let run: Run = serde_json::from_value(json!({
        "id": "rv-1", "task": "t1", "project": "app", "worker": 107, "change": "w-1", "revision": "w-1.r1",
        "recipe": "review", "round": 1, "rounds": 2, "kind": "code", "state": "passed", "nodes": [],
        "outcome": { "decision": "pass", "reasons": ["the entry point prints its greeting"], "at_ms": 1 },
        "summary": "Reviewed on two models: the greeting is right and the tests cover it.",
        "purpose": "review", "created_ms": 1, "updated_ms": 1
    }))
    .unwrap();
    let notes = flow::change_notes(
        "The app",
        &json!({ "1": [{ "title": "Ignored intent" }] }),
        Some(&run),
    );
    assert!(notes.contains("Reviewed on two models"), "{notes}");
    assert!(
        notes.contains("- the entry point prints its greeting"),
        "{notes}"
    );
    assert!(
        !notes.contains("Ignored intent"),
        "the review's account wins: {notes}"
    );
    let fallback = flow::change_notes(
        "The app",
        &json!({ "1": [{ "title": "First", "summary": "" }], "2": [{ "title": "Latest", "summary": "the last round" }] }),
        None,
    );
    assert!(
        fallback.contains("- Latest")
            && fallback.contains("the last round")
            && !fallback.contains("First"),
        "{fallback}"
    );
}

/// The lab's agent 107 writes its statements with its own key, as the hub's `sign_as_agent` does in production.
struct TestAgents {
    client: Client,
    key: KeyPair,
}

impl Agents for TestAgents {
    fn sign(&self, worker: u64, kind: &str, body: Value) -> Result<Value, String> {
        if worker != 107 {
            return Err(format!("the lab certified agent 107's key, not {worker}'s"));
        }
        let signed = Statement::new(kind, Actor::Agent { cid: worker }, body).sign(&self.key);
        self.client.post("/records", &signed.to_json())
    }
}

/// Agent 107 lands a change of one file on `reference` the way the lander does: opened on the reference, drafted and
/// sealed, queued, and stepped until it has landed. Returns the change's name.
fn land_on(
    world: &World,
    agents: &TestAgents,
    reference: &str,
    title: &str,
    path: &str,
    text: &str,
) -> String {
    let opened = world
        .ledger
        .post(
            "/lanes",
            &json!({ "repo": "app", "ref": reference, "title": title, "author": { "kind": "agent", "cid": 107 } }),
        )
        .unwrap();
    let change = opened["change"].as_str().unwrap().to_string();
    let deltas = vec![Delta {
        path: path.into(),
        after: Some(world.ledger.upload(text.as_bytes()).unwrap()),
        mode: 0o100644,
    }];
    agents
        .sign(107, "revision.draft", json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["ide_write#1"], "turn": 1, "model": "claude-opus-5-5" }))
        .unwrap();
    agents
        .sign(
            107,
            "revision.seal",
            json!({ "change": change, "n": 1, "deltas": deltas, "model": "claude-opus-5-5" }),
        )
        .unwrap();
    world
        .ledger
        .statement(
            "queue.add",
            json!({ "repo": "app", "ref": reference, "change": change, "n": 1 }),
        )
        .unwrap();
    for _ in 0..4 {
        let step = world
            .ledger
            .post("/repos/app/queue/step", &json!({ "ref": reference }))
            .unwrap();
        if step["outcome"] == "landed" {
            return change;
        }
    }
    panic!("{change} did not land on {reference}");
}

/// The head of a ref of the test repository, as Weft's repo view names it.
fn head_of(world: &World, reference: &str) -> String {
    world.ledger.read("/repos/app").unwrap()["refs"][reference]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_fix_is_backported_to_a_release_line_and_a_patch_release_is_cut_from_that_line() {
    let (client, _handle, _dir, change, agent) = lab("backport");
    let agents = TestAgents {
        client: client.clone(),
        key: KeyPair::from_hex(&agent).unwrap(),
    };
    let world = World::new(client, "a_fix_is_backported_to_a_release_line");
    let d = world.deps();

    // 1.4.0 is cut from main, built, checked and published; its line is cut at its revision.
    let released = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            version: Some("1.4.0".into()),
            changes: vec![change],
            ..Cut::default()
        },
    )
    .unwrap();
    let candidate = flow::candidate(&d, &released.id, &FakeBuilder, Some(1)).unwrap();
    flow::checks(&d, &candidate.id, true, "fake CI: every check passed").unwrap();
    let published = flow::publish(&d, &candidate.id).unwrap();
    assert_eq!(published.state, ReleaseState::Published);
    let line_cut = lanes::cut_lane(&d, &published.id).unwrap();
    assert_eq!(lanes::line_of(&line_cut), "release/1.4");
    assert_eq!(head_of(&world, "release/1.4"), published.from);

    // The fix lands on main first; the line does not have it yet.
    let fix = land_on(
        &world,
        &agents,
        "main",
        "Fix the greeting",
        "src/main.rs",
        "fn main() { println!(\"fixed\") }\n",
    );
    let main_after_fix = head_of(&world, "main");
    assert_ne!(main_after_fix, published.from);

    // The backport is a change on the line with its own review, started through the review chain; the review has not
    // passed, so it does not land.
    let chain = FakeChain::new(&world.reviews);
    let backport = lanes::backport(&d, &agents, &chain, "app", "release/1.4", &fix, 107).unwrap();
    assert_eq!(backport.state, "reviewing");
    assert_eq!(backport.line, "release/1.4");
    assert_eq!(
        d.reviews.run(&backport.review_run).unwrap().state,
        ReviewState::Running
    );
    assert_eq!(
        d.reviews.run(&backport.review_run).unwrap().change,
        backport.change,
        "the review is the change's"
    );
    let early = lanes::land_backport(&d, &backport.change).unwrap_err();
    assert!(early.contains("has not passed yet"), "{early}");
    assert_eq!(head_of(&world, "release/1.4"), published.from);

    // The review passes, and the line's queue lands the backport on the line only.
    d.reviews
        .settle_run(&backport.review_run, ReviewState::Passed);
    assert_eq!(
        d.reviews.run(&backport.review_run).unwrap().state,
        ReviewState::Passed
    );
    let landing = lanes::land_backport(&d, &backport.change).unwrap();
    assert_eq!(landing.line, "release/1.4");
    assert_eq!(head_of(&world, "release/1.4"), landing.head);
    assert_ne!(landing.head, published.from);
    assert_eq!(
        head_of(&world, "main"),
        main_after_fix,
        "main does not move for a backport"
    );
    assert!(
        lanes::land_backport(&d, &backport.change)
            .unwrap_err()
            .contains("has landed already")
    );

    // The patch release is cut from the line: numbered from 1.4.0 by the fix (a patch), drafted from the line's head.
    let patch = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            base: Some(published.id.clone()),
            lane: Some("release/1.4".into()),
            changes: vec![backport.change.clone()],
            ..Cut::default()
        },
    )
    .unwrap();
    assert_eq!(patch.version, "1.4.1");
    assert_eq!(patch.lane, "release/1.4");
    assert_eq!(patch.from, landing.head);
    assert!(
        patch.notes.contains("Backport: Fix the greeting"),
        "{}",
        patch.notes
    );
    let built = flow::candidate(&d, &patch.id, &FakeBuilder, None).unwrap();
    assert_eq!(built.state, ReleaseState::Candidate);
    flow::checks(&d, &built.id, true, "fake CI: every check passed").unwrap();
    let patched = flow::publish(&d, &built.id).unwrap();
    assert_eq!(patched.state, ReleaseState::Published);
    assert_eq!(
        flow::refresh(&d, &published.id).unwrap().state,
        ReleaseState::Superseded,
        "the patch takes the place of 1.4.0 on its line"
    );
}

#[test]
fn a_backport_is_only_of_a_landed_fix_on_main_to_a_cut_line() {
    let (client, _handle, _dir, change, agent) = lab("backport_refusals");
    let agents = TestAgents {
        client: client.clone(),
        key: KeyPair::from_hex(&agent).unwrap(),
    };
    let world = World::new(
        client,
        "a_backport_is_only_of_a_landed_fix_on_main_to_a_cut_line",
    );
    let d = world.deps();
    let chain = FakeChain::new(&world.reviews);
    let not_a_line = lanes::backport(&d, &agents, &chain, "app", "main", &change, 107).unwrap_err();
    assert!(not_a_line.contains("is not a release line"), "{not_a_line}");
    let no_line =
        lanes::backport(&d, &agents, &chain, "app", "release/9.9", &change, 107).unwrap_err();
    assert!(no_line.contains("has no release/9.9"), "{no_line}");
    assert!(lanes::backport(&d, &agents, &chain, "app", "release/1.4", "w-99", 107).is_err());

    // A draft has no line to cut: only a published release does.
    let drafted = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            version: Some("1.4.0".into()),
            changes: vec![change],
            ..Cut::default()
        },
    )
    .unwrap();
    let cut = lanes::cut_lane(&d, &drafted.id).unwrap_err();
    assert!(cut.contains("from a published release"), "{cut}");
}

/// One file with two functions: a fix edits `main`, and the line edits `greet`, so the two merge three ways.
const GREET_BASE: &str = "fn main() {\n    greet();\n}\n\nfn greet() {\n    print();\n}\n";
const GREET_FIX: &str =
    "fn main() {\n    greet();\n    exit();\n}\n\nfn greet() {\n    print();\n}\n";
const GREET_LINE: &str =
    "fn main() {\n    greet();\n}\n\nfn greet() {\n    println!(\"line\");\n}\n";
/// A second fix that edits the line the release's own change edited: its backport clashes.
const GREET_CLASH: &str =
    "fn main() {\n    greet();\n    exit();\n}\n\nfn greet() {\n    print_twice();\n}\n";

#[test]
fn a_backport_merges_three_ways_with_a_line_that_changed_the_same_file_and_refuses_a_clash() {
    let (client, _handle, _dir, change, agent) = lab("backport_merge");
    let agents = TestAgents {
        client: client.clone(),
        key: KeyPair::from_hex(&agent).unwrap(),
    };
    let world = World::new(client, "a_backport_merges_three_ways");
    let d = world.deps();
    land_on(
        &world,
        &agents,
        "main",
        "Greet from main",
        "src/main.rs",
        GREET_BASE,
    );
    let released = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            version: Some("1.4.0".into()),
            changes: vec![change],
            ..Cut::default()
        },
    )
    .unwrap();
    let candidate = flow::candidate(&d, &released.id, &FakeBuilder, Some(1)).unwrap();
    flow::checks(&d, &candidate.id, true, "fake CI: every check passed").unwrap();
    let published = flow::publish(&d, &candidate.id).unwrap();
    lanes::cut_lane(&d, &published.id).unwrap();

    // The fix edits `main` on main; the line edits `greet` in the same file, so the fix's file changed on the line too.
    let fix = land_on(
        &world,
        &agents,
        "main",
        "Exit after greeting",
        "src/main.rs",
        GREET_FIX,
    );
    let line_change = land_on(
        &world,
        &agents,
        "release/1.4",
        "Name the greeting",
        "src/main.rs",
        GREET_LINE,
    );
    assert_ne!(
        head_of(&world, "release/1.4"),
        published.from,
        "{line_change}"
    );

    let chain = FakeChain::new(&world.reviews);
    let backport = lanes::backport(&d, &agents, &chain, "app", "release/1.4", &fix, 107).unwrap();
    d.reviews
        .settle_run(&backport.review_run, ReviewState::Passed);
    let landing = lanes::land_backport(&d, &backport.change).unwrap();
    let merged = d
        .weft
        .bytes(&format!(
            "/revisions/{}/file?path=src/main.rs",
            landing.head
        ))
        .unwrap();
    assert_eq!(
        String::from_utf8(merged).unwrap(),
        "fn main() {\n    greet();\n    exit();\n}\n\nfn greet() {\n    println!(\"line\");\n}\n",
        "both sides' edits are on the line"
    );

    // A second fix edits the same line the line's change edited: the backport is refused, naming the file.
    let clash = land_on(
        &world,
        &agents,
        "main",
        "Halt the greeting",
        "src/main.rs",
        GREET_CLASH,
    );
    let refused =
        lanes::backport(&d, &agents, &chain, "app", "release/1.4", &clash, 107).unwrap_err();
    assert!(
        refused.contains("conflicts with it in: src/main.rs"),
        "{refused}"
    );
}

#[test]
fn a_release_builds_each_target_on_a_pool_that_can_build_it_and_cuts_the_rest() {
    let (client, _handle, _dir, _change, _agent) = lab("fleet");
    let world = World::new(client, "a_release_builds_each_target_on_a_pool");
    let d = world.deps();
    // The lab declares a Linux and a macOS target; the fleet has a Linux pool only.
    let fleet = FleetBuilder {
        host: FakeHost::new(9401, b"\x7fELF built by the fleet"),
        stack: crate::project_stack::parse(STACK).unwrap(),
        placements: crate::targets::place(
            &[
                crate::targets::Platform::LinuxX86_64,
                crate::targets::Platform::Macos,
            ],
            &[linux_node("n-lin")],
        ),
    };
    let drafted = flow::draft(&d, "app", "app", &cut("v0.1.0")).unwrap();
    let candidate = flow::candidate(&d, &drafted.id, &fleet, Some(1)).unwrap();
    assert_eq!(
        candidate.state,
        ReleaseState::Candidate,
        "the release is cut"
    );
    assert_eq!(candidate.artifacts.len(), 1, "{:?}", candidate.artifacts);
    let built = &candidate.artifacts[0];
    assert!(built.target.ends_with("linux-x86_64"), "{}", built.target);
    assert_eq!(built.pool, "x86_64-kvm");
    assert_eq!(candidate.not_run.len(), 1, "{:?}", candidate.not_run);
    assert!(candidate.not_run[0].target.ends_with("macos-aarch64"));
    assert!(
        candidate.not_run[0].reason.contains("a Mac pool"),
        "{}",
        candidate.not_run[0].reason
    );
    assert_eq!(
        d.runs
            .view(&drafted.run.clone().unwrap())
            .unwrap()
            .run
            .state,
        RunState::Passed,
        "a target not run does not fail the build step"
    );
}

#[test]
fn a_draft_without_a_version_is_numbered_from_its_changes_and_a_line_takes_patches_only() {
    let (client, _handle, _dir, change, _agent) = lab("numbering");
    let world = World::new(client, "a_draft_without_a_version_is_numbered");
    let d = world.deps();
    // The lab's change is a feature: the first release is 0.1.0 (the base is 0.0.0).
    let first = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            changes: vec![change.clone()],
            ..Cut::default()
        },
    )
    .unwrap();
    assert_eq!(first.version, "0.1.0");
    assert_eq!(first.channel, "stable");
    // The next is numbered from the release it follows.
    let second = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            base: Some(first.id.clone()),
            changes: vec![change.clone()],
            ..Cut::default()
        },
    )
    .unwrap();
    assert_eq!(second.version, "0.2.0");
    // The project's build numbers count up from the last one.
    let build = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            scheme: Some("build".into()),
            changes: vec![change.clone()],
            ..Cut::default()
        },
    )
    .unwrap();
    assert_eq!(build.version, "1");

    // A line takes patches only: a feature on the line is refused before anything is proposed.
    let feature_on_line = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            base: Some(first.id.clone()),
            lane: Some("release/0.1".into()),
            changes: vec![change.clone()],
            ..Cut::default()
        },
    )
    .unwrap_err();
    assert!(
        feature_on_line.contains("patches only"),
        "{feature_on_line}"
    );

    let nothing = flow::draft(&d, "app", "app", &Cut::default()).unwrap_err();
    assert!(nothing.contains("a version, or changes"), "{nothing}");
    let unknown_channel = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            version: Some("0.9.0".into()),
            channel: Some("Nightly".into()),
            ..Cut::default()
        },
    )
    .unwrap_err();
    assert!(
        unknown_channel.contains("not a channel"),
        "{unknown_channel}"
    );
    let unknown_base = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            base: Some("rel-99".into()),
            changes: vec![change],
            ..Cut::default()
        },
    )
    .unwrap_err();
    assert!(
        unknown_base.contains("is not a release of app"),
        "{unknown_base}"
    );
}

#[test]
fn an_adopted_lane_is_reviewed_through_the_chain_and_lands_once_the_review_passes() {
    let (client, _handle, _dir, _change, agent) = lab("adoption");
    let agents = TestAgents {
        client: client.clone(),
        key: KeyPair::from_hex(&agent).unwrap(),
    };
    let world = World::new(client, "an_adopted_lane_is_reviewed_through_the_chain");
    let d = world.deps();

    // The planner's adoption of a remote's commit: a lane sealed with the adopted file, not yet queued.
    let opened = world
        .ledger
        .post(
            "/lanes",
            &json!({ "repo": "app", "ref": "main", "title": "Adopt 1 commit from the remote", "author": { "kind": "agent", "cid": 107 } }),
        )
        .unwrap();
    let change = opened["change"].as_str().unwrap().to_string();
    let deltas = vec![Delta {
        path: "docs/adopted.md".into(),
        after: Some(world.ledger.upload(b"# adopted\n").unwrap()),
        mode: 0o100644,
    }];
    agents
        .sign(107, "revision.draft", json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["adopt#1"], "turn": 1, "model": "claude-opus-5-5" }))
        .unwrap();
    agents
        .sign(
            107,
            "revision.seal",
            json!({ "change": change, "n": 1, "deltas": deltas, "model": "claude-opus-5-5" }),
        )
        .unwrap();
    let before = head_of(&world, "main");

    // Its review opens through the chain, bound to the change; it does not land while the review runs.
    let chain = FakeChain::new(&world.reviews);
    let run = lanes::review_adoption(
        &chain,
        "app",
        &change,
        107,
        "Adopt 1 commit from the remote",
    )
    .unwrap();
    assert_eq!(d.reviews.run(&run).unwrap().state, ReviewState::Running);
    assert_eq!(
        d.reviews.run(&run).unwrap().change,
        change,
        "the review is the adoption's"
    );
    let early = lanes::land_backport(&d, &change).unwrap_err();
    assert!(early.contains("has not passed yet"), "{early}");
    assert_eq!(
        head_of(&world, "main"),
        before,
        "nothing lands before the review passes"
    );

    // The review passes, and the adoption lands on main.
    d.reviews.settle_run(&run, ReviewState::Passed);
    let landing = lanes::land_backport(&d, &change).unwrap();
    assert_eq!(landing.line, "main");
    assert_eq!(head_of(&world, "main"), landing.head);
    assert_ne!(landing.head, before);
}

#[test]
fn a_release_notes_edit_is_a_weft_statement_and_survives_a_refresh() {
    let (client, _handle, _dir, change, _agent) = lab("notes");
    let world = World::new(client, "a_release_notes_edit_is_a_weft_statement");
    let d = world.deps();
    let released = flow::draft(
        &d,
        "app",
        "app",
        &Cut {
            version: Some("1.0.0".into()),
            changes: vec![change],
            ..Cut::default()
        },
    )
    .unwrap();
    let edited = flow::notes(&d, &released.id, "Edited by the owner.\n").unwrap();
    assert_eq!(edited.notes, "Edited by the owner.\n");
    let refreshed = flow::refresh(&d, &released.id).unwrap();
    assert_eq!(
        refreshed.notes, "Edited by the owner.\n",
        "the owner's notes stay across a refresh"
    );
    let in_weft = world.ledger.read("/repos/app/releases/1.0.0").unwrap();
    assert_eq!(
        in_weft["notes"], "Edited by the owner.\n",
        "Weft holds the edit too"
    );
    assert_eq!(in_weft["notes_edited"], true);
}
