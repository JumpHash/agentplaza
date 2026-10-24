//! The benchmark's replay through a booted hub (PLAN 25.3, W32): an archived case is replayed as a task in its project at
//! the commit it started from; the work loop runs it like any task, the scripted VMs of the booted tests play its builder
//! and reviewers (as `intake`'s and `weft_review`'s do), and when its change lands it is scored against the case's known
//! outcome and recorded in the benchmark's replays. The replay here lands, so its score is a match.

mod common;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// The CID noded gives the first VM it starts; each next one gets the next.
const FIRST_CID: u64 = 500;
/// How long a step waits for the hub (scaled by the slack of a slow run).
const WAIT: Duration = Duration::from_secs(40);
const BUILDER_MODEL: &str = "mock/agent";
const VERIFIER_MODEL: &str = "mock/third";

/// The VMs noded started, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// The roles the seats run: a builder and a reviewer.
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

    fn write(&self, rel: &str, text: &str) {
        let p = self.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

/// Whether the change carries a record of `kind` (its `written`, its `landed`).
fn has_record(change: &Value, kind: &str) -> bool {
    change["records"]
        .as_array()
        .is_some_and(|records| records.iter().any(|r| r["type"] == kind))
}

/// The seats' scripted work on task `tid`: the builder writes `note` and finishes, the reviewer passes it, and the
/// change lands. Returns the change's id once the task is done.
fn land(
    hub: &Hub,
    gateway: &FakeDaemon,
    home: &Path,
    token: &Path,
    known: &mut HashSet<u64>,
    tid: &str,
    note: &str,
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
    b_vm.write(&format!("{repo}/notes/{tid}.md"), note);
    b_vm.weft(
        &ws,
        &["snapshot", "--turn", "1", "--model", "claude-opus-5-5"],
    );
    b_vm.tool(
        w_id,
        "weft_describe",
        json!({ "intents": [{ "title": "Record the intake work", "kind": "research", "covers": ["notes/**"] }] }),
    );
    hub.work_done(b_cid, w_id, "done", "noted");
    let sealed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(20),
        |c| c["rounds"].as_array().is_some_and(|r| r.len() == 1) && has_record(c, "written"),
    );
    assert!(has_record(&sealed, "written"), "{sealed}\n{}", hub.log());

    // Two reviewers (`reviewer#1` and `reviewer#2`, as the seat is named when it has several), then the verifier.
    for (seat, model) in [
        ("reviewer", None),
        ("reviewer", None),
        ("verifier", Some(VERIFIER_MODEL)),
    ] {
        let r = next_session(hub, tid, seat, known);
        let r_cid = r["cid"].as_u64().expect("its VM");
        known.insert(r_cid);
        // The reviewers run on models other than the builder's (two of them, for diversity); the verifier on its own.
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
            json!({ "task": tid, "pass": true, "notes": "the note is there" }),
        );
        assert!(!text(&verdict["output"]).is_empty(), "{verdict}");
        hub.work_done(r_cid, rw_id, "done", "passed");
    }
    let landed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(40),
        |c| c["state"] == "landed" && has_record(c, "landed"),
    );
    assert_eq!(landed["state"], "landed", "{landed}\n{}", hub.log());
    let done = hub.wait_for(&format!("/v1/tasks/{tid}"), Duration::from_secs(20), |t| {
        t["state"] == "done"
    });
    assert_eq!(done["state"], "done", "{done}\n{}", hub.log());
    change
}

#[test]
fn a_replay_of_an_archived_case_runs_as_a_task_and_is_scored_when_its_change_lands() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let scratch = Scratch::new("evals-replay");
    let hub = Hub::start_with_roles(
        &scratch,
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

    // A project, and an archived case of its work: the task started at the root (an empty repository) and it landed.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Notes", "goal": "Notes on the intake work", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": BUILDER_MODEL, "reviewer": "mock/echo", "verifier": VERIFIER_MODEL } })),
    );
    assert_eq!(status, 200, "{settings}");
    let (status, refused) = hub.call("POST", "/v1/evals/bench/1", None);
    assert_eq!(status, 404, "{refused}");
    let (status, case) = hub.call(
        "POST",
        "/v1/evals/archive",
        Some(json!({ "project": pid, "task": "old-1", "start": "root", "goal": "Write the notes on the intake work", "check": "the notes are written and reviewed", "outcome": "landed" })),
    );
    assert_eq!(status, 201, "{case}\n{}", hub.log());
    let case_id = case["id"].as_u64().expect("the case's id");

    // The replay is a task in the project: the work loop runs it, and the scripted VMs walk its builder and reviewers.
    let (status, replay) = hub.call("POST", &format!("/v1/evals/bench/{case_id}"), None);
    assert_eq!(status, 201, "{replay}\n{}", hub.log());
    let tid = text(&replay["task"]);
    assert!(
        hub.call("GET", &format!("/v1/tasks/{tid}"), None)
            .1
            .to_string()
            .contains("the commit the task started from"),
        "the replay's goal names the commit it starts from"
    );
    let mut known = HashSet::new();
    let _change = land(
        &hub,
        &gateway,
        &home,
        &token,
        &mut known,
        &tid,
        "the replayed notes",
    );

    // Its change landed, which is the outcome the case says it had: the replay is scored as a match.
    let bench = hub.wait_for("/v1/evals/bench", Duration::from_secs(20), |rows| {
        rows.as_array().is_some_and(|rows| {
            rows.iter()
                .any(|b| b["task"] == json!(tid) && b["outcome"] == "passed")
        })
    });
    let row = bench
        .as_array()
        .and_then(|rows| rows.iter().find(|b| b["task"] == json!(tid)).cloned())
        .unwrap_or_else(|| panic!("no replay row for {tid}: {bench}\n{}", hub.log()));
    assert_eq!(row["archive"], json!(case_id), "{row}");
    assert_eq!(
        (
            row["outcome"].as_str(),
            row["known"].as_str(),
            row["matched"].as_bool()
        ),
        (Some("passed"), Some("landed"), Some(true)),
        "{row}"
    );
    assert!(
        row["replayed_ms"].as_u64().is_some_and(|ms| ms > 0),
        "{row}"
    );
}
