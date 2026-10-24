//! Upgrades through a booted hub (PLAN 24.2, Engine 12.2 round 3): the project's tree on main is seeded by a builder's
//! task, as intake's items are built; the owner's run plans the sample's Cargo ecosystem against a local registry, and each
//! batch becomes an ordinary task whose spec names the commands to apply it. The task's builder (a scripted VM, as in
//! `intake`) applies the commands in its lane; the reviewers and the verifier pass it through the review chain; the minor
//! batch lands by itself when `upgrade_auto_land` is on, and the major (always held) waits for the owner's approval.
//! With auto-land off the minor batch waits too. No network: the registry is a directory, and cargo runs offline.

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// The CID noded gives the first VM it starts; each next one gets the next.
const FIRST_CID: u64 = 500;
/// How long a step waits for the hub (scaled by the slack of a slow run).
const WAIT: Duration = Duration::from_secs(40);
/// The models the seats use: the reviewer's differs from the builder's.
const BUILDER_MODEL: &str = "mock/agent";
const REVIEWER_MODEL: &str = "mock/echo";
const VERIFIER_MODEL: &str = "mock/third";

/// The VMs noded started, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// The roles the seats run: a builder and a reviewer.
fn roles() -> Value {
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "run_tests", "read"] });
    json!({ "roles": {
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
        "verifier": role("verifier", "Verifier"),
    } })
}

/// noded (which starts each session's VM, the next CID) and the agent gateway (which takes the work pushed to them).
fn daemons(vms: &Vms) -> (FakeDaemon, FakeDaemon) {
    let started = Arc::clone(vms);
    let noded = FakeDaemon::serve(move |method, path, body| {
        let mut vms = started.lock().unwrap_or_else(|p| p.into_inner());
        if method == "GET" && path.starts_with("/v1/runs") {
            let list: Vec<Value> = vms
                .iter()
                .enumerate()
                .map(|(i, (cid, role))| json!({ "id": i, "guest_cid": cid, "role": role }))
                .collect();
            (
                200,
                json!({ "runs": [{ "id": "r1", "ended_at": null, "vms": list }] }).to_string(),
            )
        } else if method == "POST" && path == "/v1/runs" {
            let role = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|b| b["role"].as_str().map(str::to_string))
                .unwrap_or_default();
            let cid = FIRST_CID + vms.iter().filter(|(cid, _)| *cid >= FIRST_CID).count() as u64;
            vms.push((cid, role));
            let vm = json!({ "id": vms.len() - 1, "guest_cid": cid, "state": "running" });
            (200, json!({ "id": "r1", "vms": [vm] }).to_string())
        } else {
            (200, "{}".into())
        }
    });
    let reported = Arc::clone(vms);
    let gateway = FakeDaemon::serve(move |_, path, _| {
        if path.starts_with("/v1/status") {
            let vms = reported.lock().unwrap_or_else(|p| p.into_inner());
            let agents: Vec<Value> = vms
                .iter()
                .map(|(cid, role)| json!({ "cid": cid, "role": role }))
                .collect();
            (200, json!({ "agents": agents }).to_string())
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    });
    (noded, gateway)
}

/// The work items the hub pushed to `cid`'s VM, oldest first.
fn pushed(gateway: &FakeDaemon, cid: u64) -> Vec<Value> {
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, _)| {
            method == "POST" && path.ends_with(&format!("/agents/{cid}/send"))
        })
        .filter_map(|(_, _, body)| serde_json::from_str::<Value>(&body).ok())
        .map(|sent| sent["message"].clone())
        .filter(|message| message["type"] == "work")
        .collect()
}

/// The first work item pushed to `cid`'s VM: its session's step.
fn first_work(hub: &Hub, gateway: &FakeDaemon, cid: u64) -> Value {
    let deadline = Instant::now() + WAIT.mul_f64(common::slack());
    loop {
        if let Some(work) = pushed(gateway, cid).into_iter().next() {
            return work;
        }
        assert!(
            Instant::now() < deadline,
            "no work reached agent {cid}\n{}",
            hub.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The session of `seat` on `task` that runs on a VM `known` does not have.
fn next_session(hub: &Hub, task: &str, seat: &str, known: &HashSet<u64>) -> Value {
    // A seat of several items is named `reviewer#1`, `reviewer#2`, …: a session of any of them is `seat`'s.
    let of_seat = |name: &str| name == seat || name.starts_with(&format!("{seat}#"));
    let fresh = |s: &Value| {
        s["seat"].as_str().is_some_and(of_seat)
            && matches!(s["state"].as_str(), Some("forking" | "running"))
            && s["cid"].as_u64().is_some_and(|cid| !known.contains(&cid))
    };
    let list = hub.wait_for(&format!("/v1/sessions?task={task}"), WAIT, |list| {
        list.as_array().is_some_and(|l| l.iter().any(&fresh))
    });
    list.as_array()
        .and_then(|l| {
            l.iter()
                .filter(|s| fresh(s))
                .min_by_key(|s| s["seat"].as_str().unwrap_or("").to_string())
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!(
                "no new {seat} session for task {task} (known {known:?}): {list}\n{}",
                hub.log()
            )
        })
}

/// The VM says hello: its session runs, and its step's work goes out.
fn hello(hub: &Hub, cid: u64, role: &str) {
    let (status, body) = hub.guest(cid, "hello", json!({ "role": role, "vm": 0 }));
    assert_eq!(status, 200, "{body}");
}

/// A session's VM: its workspace and the `weft` commands its harness runs, through the agent gateway.
struct Vm<'a> {
    hub: &'a Hub,
    cid: u64,
    home: std::path::PathBuf,
    token: std::path::PathBuf,
}

fn vm_of<'a>(hub: &'a Hub, cid: u64, home: &Path, token: &Path) -> Vm<'a> {
    Vm {
        hub,
        cid,
        home: home.join(format!("vm{cid}")),
        token: token.to_path_buf(),
    }
}

impl Vm<'_> {
    fn weft(&self, cwd: &Path, args: &[&str]) {
        let env = std::collections::BTreeMap::from([
            (
                "WEFT_HUB".to_string(),
                format!(
                    "http://127.0.0.1:{}/internal/v1/agents/{}/gateway/weft",
                    self.hub.port, self.cid
                ),
            ),
            (
                "WEFT_TOKEN_FILE".to_string(),
                self.token.display().to_string(),
            ),
            (
                "WEFT_CACHE".to_string(),
                self.home.join(".cache").display().to_string(),
            ),
        ]);
        weft::cli::run_in(
            args.iter().map(|s| s.to_string()).collect(),
            cwd.to_path_buf(),
            env,
        )
        .unwrap_or_else(|e| {
            panic!(
                "agent {}: weft {}: {e}\n{}",
                self.cid,
                args.join(" "),
                self.hub.log()
            )
        });
    }

    /// Checks `change` out in this VM's own workspace; returns the workspace.
    fn checkout(&self, change: &str, repo: &str) -> std::path::PathBuf {
        let ws = self.home.join(repo);
        self.weft(
            &self.home,
            &["checkout", change, "--dir", ws.to_str().unwrap()],
        );
        ws
    }

    /// A tool call the model makes in work item `work`, which must succeed: the hub's decision.
    fn tool(&self, work: u64, name: &str, input: Value) -> Value {
        let message = json!({ "type": "tool_request", "work": work, "call": format!("toolu_{name}_{work}"), "name": name, "input": input });
        let (status, body) = self.hub.guest(self.cid, "message", message);
        assert_eq!(status, 200, "{body}");
        let reply = body["reply"].clone();
        assert_eq!(
            reply["is_error"], false,
            "{name} for agent {}: {reply}",
            self.cid
        );
        reply
    }
}

/// Whether the change carries a record of `kind` (its `written`, its `landed`).
fn has_record(change: &Value, kind: &str) -> bool {
    change["records"]
        .as_array()
        .is_some_and(|records| records.iter().any(|r| r["type"] == kind))
}

/// The seats' scripted work on the task `tid`: the builder applies `apply` in its lane and describes the change (its
/// covers), then the reviewer, the second reviewer and the verifier pass it. Returns the change once it is sealed and
/// reviewed (the caller waits for what follows the review: the landing, or the owner).
fn drive(
    hub: &Hub,
    gateway: &FakeDaemon,
    home: &Path,
    token: &Path,
    known: &mut HashSet<u64>,
    tid: &str,
    covers: &[&str],
    apply: impl FnOnce(&Path),
) -> String {
    let b = next_session(hub, tid, "builder", known);
    let b_cid = b["cid"].as_u64().expect("its VM");
    known.insert(b_cid);
    assert_eq!(b["model"], BUILDER_MODEL, "{b}");
    hello(hub, b_cid, "builder");
    let w = first_work(hub, gateway, b_cid);
    let w_id = w["work"].as_u64().expect("the work item");
    let change = text(&w["lane"]["change"]);
    let repo = text(&w["lane"]["repo"]);
    let b_vm = vm_of(hub, b_cid, home, token);
    let ws = b_vm.checkout(&change, &repo);
    apply(&ws);
    b_vm.weft(
        &ws,
        &["snapshot", "--turn", "1", "--model", "claude-opus-5-5"],
    );
    b_vm.tool(
        w_id,
        "weft_describe",
        json!({ "intents": [{ "title": "Upgrade the dependency", "kind": "chore", "covers": covers }] }),
    );
    hub.work_done(b_cid, w_id, "done", "applied");
    let sealed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(20),
        |c| c["rounds"].as_array().is_some_and(|r| r.len() == 1) && has_record(c, "written"),
    );
    assert!(has_record(&sealed, "written"), "{sealed}\n{}", hub.log());

    for (seat, model) in [
        ("reviewer", None),
        ("reviewer", None),
        ("verifier", Some(VERIFIER_MODEL)),
    ] {
        let r = next_session(hub, tid, seat, known);
        let r_cid = r["cid"].as_u64().expect("its VM");
        known.insert(r_cid);
        match model {
            Some(model) => assert_eq!(r["model"], model, "{r}"),
            None => assert_ne!(r["model"], BUILDER_MODEL, "{r}"),
        }
        hello(hub, r_cid, seat);
        let rw = first_work(hub, gateway, r_cid);
        let rw_id = rw["work"].as_u64().expect("the work item");
        let verdict = vm_of(hub, r_cid, home, token).tool(
            rw_id,
            "plaza_verdict",
            json!({ "task": tid, "pass": true, "notes": "the update is as the spec says" }),
        );
        assert!(
            text(&verdict["output"]).starts_with("Recorded"),
            "{verdict}"
        );
        hub.work_done(r_cid, rw_id, "done", "passed");
    }
    change
}

/// The builder's work on an upgrade: the spec's commands, in order, in its own lane. A Cargo requirement the spec moves
/// is edited; every `cargo` line runs with its target directory outside the lane, so only the lockfile changes.
fn apply_spec(ws: &Path, acceptance: &str) {
    for line in acceptance.lines().map(str::trim) {
        if let Some(edit) = line.strip_prefix("edit Cargo.toml: ") {
            let (name, req) = edit.split_once(" = ").expect("a requirement to move");
            let req = req.trim_matches('"');
            let path = ws.join("Cargo.toml");
            let text = std::fs::read_to_string(&path).unwrap();
            let edited: Vec<String> = text
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with(&format!("{name} = ")) {
                        format!("{name} = \"{req}\"")
                    } else {
                        l.to_string()
                    }
                })
                .collect();
            std::fs::write(&path, edited.join("\n") + "\n").unwrap();
        } else if line.starts_with("cargo ") {
            run_in(ws, line);
        } else if let Some(cmd) = line.strip_prefix("Then run the tests: ") {
            run_in(ws, cmd);
        }
    }
}

fn run_in(ws: &Path, line: &str) {
    let mut parts = line.split_whitespace();
    let program = parts.next().expect("a command");
    let target = ws.parent().expect("the VM's home").join("target");
    let status = Command::new(program)
        .args(parts)
        .current_dir(ws)
        .env("CARGO_TARGET_DIR", target)
        .status()
        .unwrap_or_else(|e| panic!("{line}: {e}"));
    assert!(status.success(), "{line} failed in the lane");
}

/// The acceptance (the spec) of task `tid`.
fn spec_of(hub: &Hub, tid: &str) -> String {
    let task = hub.wait_for(&format!("/v1/tasks/{tid}"), WAIT, |t| {
        t["acceptance"].is_string()
    });
    text(&task["acceptance"])
}

/// Waits until the change lands and its task is done (the queue lands it when its review passed and it may land).
fn landed(hub: &Hub, change: &str, tid: &str) {
    let landed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(60),
        |c| c["state"] == "landed" && has_record(c, "landed"),
    );
    assert_eq!(landed["state"], "landed", "{landed}\n{}", hub.log());
    let done = hub.wait_for(&format!("/v1/tasks/{tid}"), Duration::from_secs(20), |t| {
        t["state"] == "done"
    });
    assert_eq!(done["state"], "done", "{done}\n{}", hub.log());
}

/// The sample project as a directory a registry is published into: `tiny` at 1.0.0 locked by cargo, the project's
/// config naming the registry. Returns the registry's path and the sample's files (lockfile included).
fn sample(scratch: &Path) -> (PathBuf, Vec<(String, String)>) {
    let registry = scratch.join("registry");
    publish(&registry, "tiny", "1.0.0");
    // A TOML basic string reads `\` as an escape, so a Windows path goes in with `/` separators (cargo takes them).
    let config = format!(
        "[source.crates-io]\nreplace-with = \"local\"\n\n[source.local]\nlocal-registry = \"{}\"\n",
        registry.to_string_lossy().replace('\\', "/")
    );
    let files = vec![
        ("Cargo.toml".to_string(), "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntiny = \"1\"\n".to_string()),
        ("src/lib.rs".to_string(), "pub fn v() -> &'static str { tiny::version() }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_works() {\n        assert!(!super::v().is_empty());\n    }\n}\n".to_string()),
        (".cargo/config.toml".to_string(), config),
    ];
    let generated = scratch.join("generated");
    for (path, text) in &files {
        std::fs::create_dir_all(generated.join(path).parent().unwrap()).unwrap();
        std::fs::write(generated.join(path), text).unwrap();
    }
    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(&generated)
        .status()
        .unwrap();
    assert!(status.success(), "cargo generate-lockfile");
    let lock = std::fs::read_to_string(generated.join("Cargo.lock")).unwrap();
    let mut files = files;
    files.push(("Cargo.lock".to_string(), lock));
    (registry, files)
}

/// Publishes `name` at `version` into the registry directory: a `.crate` packed as cargo packs one, and its index line.
fn publish(registry: &Path, name: &str, version: &str) {
    let stage = registry.join("stage");
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
    let crate_file = registry.join(format!("{name}-{version}.crate"));
    let status = Command::new("tar")
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
    let cksum = format!("{:x}", sha2_digest(&bytes));
    let entry = format!(
        "{{\"name\":\"{name}\",\"vers\":\"{version}\",\"deps\":[],\"cksum\":\"{cksum}\",\"features\":{{}},\"yanked\":false}}\n"
    );
    let index = registry.join("index").join(index_path(name));
    std::fs::create_dir_all(index.parent().unwrap()).unwrap();
    let mut text = std::fs::read_to_string(&index).unwrap_or_default();
    text.push_str(&entry);
    std::fs::write(&index, text).unwrap();
    let _ = std::fs::remove_dir_all(&stage);
}

fn sha2_digest(bytes: &[u8]) -> impl std::fmt::LowerHex {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
}

/// The sparse layout of a crate's index file (`ti/ny/tiny`).
fn index_path(name: &str) -> String {
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// A booted hub with its VMs' daemons, its token and its home, and a project with the seed of its tree on main.
struct Booted {
    hub: Hub,
    gateway: FakeDaemon,
    _noded: FakeDaemon,
    home: PathBuf,
    token: PathBuf,
    pid: String,
    registry: PathBuf,
    known: HashSet<u64>,
}

fn boot(scratch: &Scratch, auto_land: bool) -> Booted {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let hub = Hub::start_with_roles(
        scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    let token = scratch.0.join("internal.token");
    std::fs::write(&token, hub.token()).unwrap();
    let home = scratch.0.join("vms");

    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Sample", "goal": "A project whose dependencies are upgraded", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({
            "role_models": { "builder": BUILDER_MODEL, "reviewer": REVIEWER_MODEL, "verifier": VERIFIER_MODEL },
            "upgrade_auto_land": auto_land,
            "upgrade_minutes": 0
        })),
    );
    assert_eq!(status, 200, "{settings}\n{}", hub.log());
    let (registry, files) = sample(&scratch.0);
    Booted {
        hub,
        gateway,
        _noded: noded,
        home,
        token,
        pid,
        registry,
        known: HashSet::new(),
    }
    .seeded(files)
}

impl Booted {
    /// The sample's files on main: a seed task whose builder writes them, reviewed and landed.
    fn seeded(mut self, files: Vec<(String, String)>) -> Booted {
        let (status, task) = self.hub.call(
            "POST",
            "/v1/tasks",
            Some(json!({ "goal": "Seed the sample project", "project": self.pid, "role": "builder" })),
        );
        assert_eq!(status, 201, "{task}");
        let tid = text(&task["id"]);
        let change = drive(
            &self.hub,
            &self.gateway,
            &self.home,
            &self.token,
            &mut self.known,
            &tid,
            &["**"],
            |ws| {
                for (path, text) in &files {
                    let p = ws.join(path);
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(p, text).unwrap();
                }
            },
        );
        landed(&self.hub, &change, &tid);
        self
    }

    /// Publishes a release of tiny into the registry the project's config names.
    fn release(&self, version: &str) {
        publish(&self.registry, "tiny", version);
    }

    /// The batches of the project, once the owner's run has made them (two: minor and major).
    fn batches(&self) -> Vec<Value> {
        let list = self.hub.wait_for(
            &format!("/v1/projects/{}/upgrades", self.pid),
            Duration::from_secs(180),
            |v| {
                v["batches"].as_array().is_some_and(|b| {
                    b.len() == 2
                        && b.iter()
                            .all(|x| !x["task"].as_str().unwrap_or("").is_empty())
                })
            },
        );
        let batches = list["batches"].as_array().cloned().unwrap_or_default();
        assert!(
            batches.len() == 2,
            "the run made two batches: {list}\n{}",
            self.hub.log()
        );
        batches
    }

    /// The approval of kind `kind` the owner is asked for, once it is listed.
    fn approval(&self, kind: &str) -> Value {
        let list = self
            .hub
            .wait_for("/v1/approvals", Duration::from_secs(120), |list| {
                list.as_array()
                    .is_some_and(|l| l.iter().any(|a| a["kind"] == kind))
            });
        list.as_array()
            .and_then(|l| l.iter().find(|a| a["kind"] == kind).cloned())
            .unwrap_or_else(|| {
                let batches = self
                    .hub
                    .call("GET", &format!("/v1/projects/{}/upgrades", self.pid), None)
                    .1;
                panic!(
                    "no {kind} approval: {list}\nbatches: {batches}\n{}",
                    self.hub.log()
                )
            })
    }
}

#[test]
fn a_scheduled_minor_batch_lands_through_the_review_chain_and_a_major_waits_for_the_owner() {
    let scratch = Scratch::new("upgrades-auto");
    let mut b = boot(&scratch, true);
    b.release("1.1.0");
    b.release("1.2.0");
    b.release("2.0.0");
    let (status, run) = b.hub.call(
        "POST",
        &format!("/v1/projects/{}/upgrades/run", b.pid),
        None,
    );
    assert_eq!(status, 202, "{run}\n{}", b.hub.log());
    let batches = b.batches();
    let minor = batches
        .iter()
        .find(|x| x["kind"] == "minor")
        .expect("a minor batch")
        .clone();
    let major = batches
        .iter()
        .find(|x| x["kind"] == "major")
        .expect("a major batch")
        .clone();
    assert_eq!(minor["state"], "task", "{minor}");
    assert!(
        !minor["hold"].as_bool().unwrap_or(true),
        "auto-land on: the minor lands by itself: {minor}"
    );
    assert!(
        major["hold"].as_bool().unwrap_or(false),
        "a major always waits: {major}"
    );

    // The minor: its builder applies the spec's commands in its lane, the chain reviews it, and it lands.
    let minor_tid = text(&minor["task"]);
    let spec = spec_of(&b.hub, &minor_tid);
    assert!(
        spec.contains("cargo update -p tiny --precise 1.2.0"),
        "{spec}"
    );
    let minor_change = drive(
        &b.hub,
        &b.gateway,
        &b.home,
        &b.token,
        &mut b.known,
        &minor_tid,
        &["Cargo.lock"],
        |ws| apply_spec(ws, &spec),
    );
    landed(&b.hub, &minor_change, &minor_tid);
    let lock = b
        .hub
        .wait_for(&format!("/v1/weft/changes/{minor_change}"), WAIT, |c| {
            c["state"] == "landed"
        });
    assert_eq!(lock["state"], "landed", "{lock}");

    // The major: the same chain, then the queue asks the owner, and the change does not land.
    let major_tid = text(&major["task"]);
    let spec = spec_of(&b.hub, &major_tid);
    assert!(spec.contains("edit Cargo.toml: tiny = \"2\""), "{spec}");
    let major_change = drive(
        &b.hub,
        &b.gateway,
        &b.home,
        &b.token,
        &mut b.known,
        &major_tid,
        &["Cargo.lock", "Cargo.toml"],
        |ws| apply_spec(ws, &spec),
    );
    let asked = b.approval("land");
    assert_eq!(asked["kind"], "land", "{asked}");
    let still = b
        .hub
        .call("GET", &format!("/v1/weft/changes/{major_change}"), None)
        .1;
    assert_ne!(
        still["state"], "landed",
        "a major waits for the owner: {still}"
    );

    // The record follows its tasks: the minor is done and names its change; the major names its change and waits.
    let batches = b.batches();
    let minor = batches.iter().find(|x| x["kind"] == "minor").unwrap();
    assert_eq!(minor["change"], minor_change.as_str(), "{minor}");
}

#[test]
fn with_auto_land_off_the_minor_batch_waits_for_the_owner() {
    let scratch = Scratch::new("upgrades-waits");
    let mut b = boot(&scratch, false);
    b.release("1.1.0");
    b.release("1.2.0");
    b.release("2.0.0");
    let (status, run) = b.hub.call(
        "POST",
        &format!("/v1/projects/{}/upgrades/run", b.pid),
        None,
    );
    assert_eq!(status, 202, "{run}\n{}", b.hub.log());
    let batches = b.batches();
    let minor = batches
        .iter()
        .find(|x| x["kind"] == "minor")
        .expect("a minor batch")
        .clone();
    assert!(
        minor["hold"].as_bool().unwrap_or(false),
        "auto-land off: it waits: {minor}"
    );
    let minor_tid = text(&minor["task"]);
    let spec = spec_of(&b.hub, &minor_tid);
    let change = drive(
        &b.hub,
        &b.gateway,
        &b.home,
        &b.token,
        &mut b.known,
        &minor_tid,
        &["Cargo.lock"],
        |ws| apply_spec(ws, &spec),
    );
    let asked = b.approval("land");
    assert_eq!(asked["kind"], "land", "{asked}");
    let still = b
        .hub
        .call("GET", &format!("/v1/weft/changes/{change}"), None)
        .1;
    assert_ne!(
        still["state"], "landed",
        "the owner approves before it lands: {still}"
    );
}
