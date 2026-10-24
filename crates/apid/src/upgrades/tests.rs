//! The upgrade flow against a real `cargo` and a local registry (a directory laid out as an index, with `.crate` files):
//! a sample project in a temp dir, a tiny crate published into the registry at each release, and the hub's repository
//! played in memory. The network is never touched: the tests' fetch answers nothing, and cargo runs offline.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::cargo::index_path;
use super::*;

/// The environment that keeps cargo on the local registry.
fn offline() -> Vec<(String, String)> {
    vec![("CARGO_NET_OFFLINE".into(), "true".into())]
}

/// A fetch that reaches nothing: a changelog is not reachable, and the test says so in the record.
struct NoNet;

impl Fetch for NoNet {
    fn bytes(&self, _url: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}

/// The hub's repository, in memory: its main, the changes opened on it and their queueing.
#[derive(Default)]
struct FakeLanes {
    tree: Tree,
    opened: Mutex<Vec<(String, Vec<(String, Option<Vec<u8>>)>, String)>>,
    records: Mutex<Vec<UpgradeBatch>>,
    queued: Mutex<Vec<String>>,
}

impl Lanes for FakeLanes {
    fn main(&self, _project: &str) -> Result<Tree, String> {
        Ok(self.tree.clone())
    }
    fn open(
        &self,
        _project: &str,
        title: &str,
        files: &[(String, Option<Vec<u8>>)],
        summary: &str,
    ) -> Result<String, String> {
        let mut opened = self.opened.lock().unwrap();
        opened.push((title.to_string(), files.to_vec(), summary.to_string()));
        Ok(format!("c{}", opened.len()))
    }
    fn record(&self, _project: &str, batch: &UpgradeBatch) -> Result<(), String> {
        self.records.lock().unwrap().push(batch.clone());
        Ok(())
    }
    fn queue(&self, _project: &str, change: &str) -> Result<(), String> {
        self.queued.lock().unwrap().push(change.to_string());
        Ok(())
    }
}

/// A registry directory: `index/<path>` lines and `<name>-<version>.crate` files, as cargo's local registries are.
struct Registry {
    dir: PathBuf,
}

impl Registry {
    fn new(root: &Path, name: &str) -> Registry {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("index")).unwrap();
        Registry { dir }
    }

    /// Publishes `name` at `version`: a crate whose library returns its version, packed as cargo packs one.
    fn publish(&self, name: &str, version: &str) {
        let stage = self.dir.join("stage");
        let root = stage.join(format!("{name}-{version}"));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\ndescription = \"fixture\"\nlicense = \"MIT\"\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!("pub fn version() -> &'static str {{ \"{version}\" }}\n"),
        )
        .unwrap();
        let crate_file = self.dir.join(format!("{name}-{version}.crate"));
        // COPYFILE_DISABLE keeps macOS's tar from adding its `._` metadata entries, which cargo refuses.
        let status = std::process::Command::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .arg("-czf")
            .arg(&crate_file)
            .arg("-C")
            .arg(&stage)
            .arg(format!("{name}-{version}"))
            .status()
            .unwrap();
        assert!(status.success(), "packing {name} {version}");
        let bytes = std::fs::read(&crate_file).unwrap();
        let cksum = format!("{:x}", Sha256::digest(&bytes));
        let entry = format!(
            "{{\"name\":\"{name}\",\"vers\":\"{version}\",\"deps\":[],\"cksum\":\"{cksum}\",\"features\":{{}},\"yanked\":false}}\n"
        );
        let index = self.dir.join("index").join(index_path(name));
        std::fs::create_dir_all(index.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&index).unwrap_or_default();
        text.push_str(&entry);
        std::fs::write(&index, text).unwrap();
        let _ = std::fs::remove_dir_all(&stage);
    }
}

/// A sample project that depends on `tiny = "1"`, its config pointing cargo at the registry, and no lockfile yet.
fn sample_tree(registry: &Path) -> Tree {
    let mut tree = Tree::new();
    tree.insert(
        "Cargo.toml".into(),
        b"[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntiny = \"1\"\n".to_vec(),
    );
    tree.insert(
        "src/lib.rs".into(),
        b"pub fn v() -> &'static str { tiny::version() }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_works() {\n        assert!(!super::v().is_empty());\n    }\n}\n".to_vec(),
    );
    tree.insert(
        ".cargo/config.toml".into(),
        format!(
            "[source.crates-io]\nreplace-with = \"local\"\n\n[source.local]\nlocal-registry = \"{}\"\n",
            registry.display()
        )
        .into_bytes(),
    );
    tree
}

/// The sample's lockfile, made by cargo against the registry as it stands.
fn with_lock(tree: &mut Tree, scratch: &Path) {
    let dir = scratch.join("lock");
    materialize(tree, &dir).unwrap();
    let status = std::process::Command::new("cargo")
        .args(["generate-lockfile"])
        .current_dir(&dir)
        .envs(offline())
        .status()
        .unwrap();
    assert!(status.success());
    tree.insert(
        "Cargo.lock".into(),
        std::fs::read(dir.join("Cargo.lock")).unwrap(),
    );
}

/// A scratch directory of the test, removed when it drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("apid-upgrade-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ports<'a>(
    lanes: &'a FakeLanes,
    exec: &'a Local,
    fetch: &'a NoNet,
    auto_land: bool,
) -> Ports<'a> {
    Ports {
        lanes,
        exec,
        fetch,
        path_var: std::env::var("PATH").unwrap_or_default(),
        env: offline(),
        auto_land,
        now: 1_000,
    }
}

/// A project with tiny 1.0.0 locked, and the registry offering 1.1.0, 1.2.0 (minor) and 2.0.0 (major) of it.
fn scenario(scratch: &Scratch) -> (Tree, PathBuf) {
    let registry_dir = scratch.0.join("registry");
    let registry = Registry::new(&scratch.0, "registry");
    registry.publish("tiny", "1.0.0");
    let mut tree = sample_tree(&registry_dir);
    with_lock(&mut tree, &scratch.0);
    registry.publish("tiny", "1.1.0");
    registry.publish("tiny", "1.2.0");
    registry.publish("tiny", "2.0.0");
    (tree, registry_dir)
}

#[test]
fn a_scheduled_run_lands_a_passing_minor_batch_through_the_queue() {
    let scratch = Scratch::new("minor");
    let (tree, _) = scenario(&scratch);
    let lanes = FakeLanes {
        tree,
        ..FakeLanes::default()
    };
    let report = process(
        &ports(&lanes, &Local, &NoNet, true),
        "p1",
        Ecosystem::Cargo,
        &Request::Schedule,
    );
    assert_eq!(report.skipped, None, "{report:?}");
    assert!(report.plan.is_ok(), "{report:?}");
    let minor = report
        .batches
        .iter()
        .find(|b| b.kind == "minor")
        .expect("a minor batch");
    assert_eq!(minor.state, "queued", "{minor:?}");
    assert!(!minor.hold);
    assert_eq!(minor.tests, "selected: sample");
    assert!(
        minor.summary.contains("tiny 1.0.0 -> 1.2.0"),
        "{}",
        minor.summary
    );
    // The change: the lockfile only, and it is the batch's; the queue has it.
    let opened = lanes.opened.lock().unwrap();
    let (title, files, summary) = opened
        .iter()
        .find(|(t, _, _)| t.contains("patch and minor"))
        .unwrap();
    assert!(title.contains("cargo"), "{title}");
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["Cargo.lock"]);
    let lock = String::from_utf8(files[0].1.clone().unwrap()).unwrap();
    assert!(
        lock.contains("name = \"tiny\"\nversion = \"1.2.0\""),
        "{lock}"
    );
    assert!(
        summary.contains("tiny 1.0.0 -> 1.2.0: no changelog reachable"),
        "{summary}"
    );
    // Every batch's change is queued: the major's hold is the queue's to honour, as the minor's is when it waits.
    let queued = lanes.queued.lock().unwrap().clone();
    let changes: Vec<String> = report.batches.iter().map(|b| b.change.clone()).collect();
    assert_eq!(queued, changes);
    // The major is its own change, with the whole suite and the manifest moved to the new requirement.
    let major = report
        .batches
        .iter()
        .find(|b| b.kind == "major")
        .expect("a major batch");
    assert_eq!(major.state, "waiting");
    assert!(major.hold, "a major always waits for the owner");
    assert_eq!(major.tests, "full");
    let (_, files, _) = opened
        .iter()
        .find(|(t, _, _)| t.contains("(major)"))
        .unwrap();
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["Cargo.lock", "Cargo.toml"]);
}

#[test]
fn with_auto_land_off_the_minor_batch_waits_for_the_owner() {
    let scratch = Scratch::new("waits");
    let (tree, _) = scenario(&scratch);
    let lanes = FakeLanes {
        tree,
        ..FakeLanes::default()
    };
    let report = process(
        &ports(&lanes, &Local, &NoNet, false),
        "p1",
        Ecosystem::Cargo,
        &Request::Schedule,
    );
    let minor = report.batches.iter().find(|b| b.kind == "minor").unwrap();
    assert_eq!(minor.state, "waiting");
    assert!(minor.hold, "held: the landing queue asks the owner first");
    // The record is what the queue reads for the hold.
    let store = Store::default();
    store.record("p1", minor.clone());
    assert_eq!(store.holds(&minor.change), Some(true));
    assert_eq!(store.holds("not-a-batch"), None);
    // The change is still queued: the hold is the queue's to honour.
    assert_eq!(lanes.queued.lock().unwrap().len(), report.batches.len());
}

#[test]
fn a_cve_fix_moves_its_crate_to_the_fixed_release() {
    let scratch = Scratch::new("cve");
    let registry = Registry::new(&scratch.0, "registry");
    let registry_dir = scratch.0.join("registry");
    registry.publish("tiny", "1.0.0");
    let mut tree = sample_tree(&registry_dir);
    with_lock(&mut tree, &scratch.0);
    registry.publish("tiny", "1.0.1");
    let lanes = FakeLanes {
        tree,
        ..FakeLanes::default()
    };
    let advisory = Advisory {
        id: "ADV-1".into(),
        ecosystem: Ecosystem::Cargo,
        package: "tiny".into(),
        fixed: "1.0.1".into(),
        priority: "high".into(),
    };
    let report = process(
        &ports(&lanes, &Local, &NoNet, true),
        "p1",
        Ecosystem::Cargo,
        &Request::Fix(advisory.clone()),
    );
    assert_eq!(report.batches.len(), 1, "{report:?}");
    let fix = &report.batches[0];
    assert_eq!(fix.kind, "fix");
    assert_eq!(fix.state, "queued", "{fix:?}");
    assert!(
        fix.reason.contains("ADV-1") && fix.reason.contains("high"),
        "{}",
        fix.reason
    );
    let opened = lanes.opened.lock().unwrap();
    let lock = String::from_utf8(opened[0].1[0].1.clone().unwrap()).unwrap();
    assert!(
        lock.contains("name = \"tiny\"\nversion = \"1.0.1\""),
        "{lock}"
    );

    // A fix the lockfile already has is no batch at all.
    let done = Advisory {
        fixed: "1.0.0".into(),
        ..advisory
    };
    let lanes = FakeLanes {
        tree: lanes.tree.clone(),
        ..FakeLanes::default()
    };
    let report = process(
        &ports(&lanes, &Local, &NoNet, true),
        "p1",
        Ecosystem::Cargo,
        &Request::Fix(done),
    );
    assert!(report.batches.is_empty(), "{report:?}");
}

#[test]
fn an_ecosystem_whose_tool_is_missing_is_skipped_with_the_reason() {
    let scratch = Scratch::new("skip");
    let (tree, _) = scenario(&scratch);
    let lanes = FakeLanes {
        tree,
        ..FakeLanes::default()
    };
    let mut p = ports(&lanes, &Local, &NoNet, true);
    p.path_var = String::new();
    let report = process(&p, "p1", Ecosystem::Cargo, &Request::Schedule);
    assert!(report.batches.is_empty());
    assert!(
        report
            .skipped
            .unwrap()
            .contains("`cargo` is not on the hub's PATH")
    );
    assert!(lanes.opened.lock().unwrap().is_empty());
}

#[test]
fn the_schedules_are_made_per_detected_ecosystem_and_a_missing_tool_is_kept_as_its_skip() {
    let paths: Vec<String> = ["Cargo.toml", "package.json", "src/main.rs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let schedules = detect_schedules(&paths, "", 5_000, 60);
    assert_eq!(schedules.len(), 2);
    assert!(
        schedules
            .iter()
            .all(|s| s.next_ms == 0 && s.skipped.contains("not on the hub's PATH"))
    );
    let dir = Scratch::new("tools");
    let tool = dir.0.join("cargo");
    std::fs::write(&tool, "#!/bin/sh\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let schedules = detect_schedules(&paths, &dir.0.display().to_string(), 5_000, 60);
    let cargo = schedules.iter().find(|s| s.ecosystem == "cargo").unwrap();
    assert!(cargo.skipped.is_empty());
    assert_eq!(cargo.next_ms, 5_000 + 60_000);
    let npm = schedules.iter().find(|s| s.ecosystem == "npm").unwrap();
    assert!(npm.skipped.contains("npm"));
}

#[test]
fn a_changelog_is_read_from_the_repository_the_registry_names() {
    struct Host(BTreeMap<String, Vec<u8>>);
    impl Fetch for Host {
        fn bytes(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.get(url).cloned())
        }
    }
    let mut routes = BTreeMap::new();
    routes.insert(
        "https://crates.io/api/v1/crates/tiny".to_string(),
        br#"{"crate":{"repository":"https://github.com/acme/tiny.git"}}"#.to_vec(),
    );
    routes.insert(
        "https://raw.githubusercontent.com/acme/tiny/HEAD/CHANGELOG.md".to_string(),
        b"# Changelog\n\n## 1.2.0\n- adds frobnicate\n\n## 1.1.0\n- speeds up\n\n## 1.0.0\n- first\n".to_vec(),
    );
    let text = changelog(&Host(routes), Ecosystem::Cargo, "tiny", "1.0.0", "1.2.0");
    assert!(
        text.starts_with("## 1.2.0") && text.contains("speeds up") && !text.contains("first"),
        "{text}"
    );
    assert_eq!(
        changelog(&NoNet, Ecosystem::Cargo, "tiny", "1.0.0", "1.2.0"),
        ""
    );
    assert_eq!(
        github_raw("git+https://github.com/a/b.git").as_deref(),
        Some("https://raw.githubusercontent.com/a/b/HEAD")
    );
    assert_eq!(excerpt("## 2.0.0\nx\n", "1.0.0", "1.2.0"), "");
}

#[test]
fn a_changed_tree_names_what_a_batch_wrote_and_skips_the_build_output() {
    let scratch = Scratch::new("changed");
    let mut tree = Tree::new();
    tree.insert("Cargo.lock".into(), b"old".to_vec());
    tree.insert("src/lib.rs".into(), b"same".to_vec());
    tree.insert("gone.txt".into(), b"x".to_vec());
    materialize(&tree, &scratch.0).unwrap();
    std::fs::write(scratch.0.join("Cargo.lock"), b"new").unwrap();
    std::fs::create_dir_all(scratch.0.join("target/debug")).unwrap();
    std::fs::write(scratch.0.join("target/debug/out"), b"build").unwrap();
    std::fs::remove_file(scratch.0.join("gone.txt")).unwrap();
    let changed = changed_files(&scratch.0, &tree).unwrap();
    assert_eq!(
        changed,
        vec![
            ("Cargo.lock".to_string(), Some(b"new".to_vec())),
            ("gone.txt".to_string(), None),
        ]
    );
}

#[test]
fn the_store_keeps_the_batches_its_hold_and_the_due_schedules() {
    let store = Store::default();
    let batch = UpgradeBatch {
        id: "cargo-minor-1".into(),
        change: "c7".into(),
        hold: true,
        state: "waiting".into(),
        ..UpgradeBatch::default()
    };
    store.record("p1", batch.clone());
    store.record(
        "p1",
        UpgradeBatch {
            state: "queued".into(),
            hold: false,
            ..batch.clone()
        },
    );
    assert_eq!(
        store.batches("p1").len(),
        1,
        "a batch is kept once, by its id"
    );
    assert_eq!(store.holds("c7"), Some(false));
    store.set_schedules(
        "p1",
        vec![
            Schedule {
                ecosystem: "cargo".into(),
                next_ms: 100,
                skipped: String::new(),
            },
            Schedule {
                ecosystem: "npm".into(),
                next_ms: 0,
                skipped: "no npm".into(),
            },
        ],
    );
    assert!(store.detected("p1"));
    assert_eq!(store.due(99), Vec::<(String, String)>::new());
    assert_eq!(
        store.due(100),
        vec![("p1".to_string(), "cargo".to_string())]
    );
    store.advance("p1", "cargo", 500);
    assert!(store.due(200).is_empty());
}
