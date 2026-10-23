//! Weft through the work engine, end to end on a scratch hub, with the sessions' VMs played by this test. A session's VM
//! is a run noded starts for the hub: the test takes the CID noded gives it, hellos as that VM, reads the work the hub
//! pushes to it (through the agent gateway), and plays what its harness does: the `weft` commands (through the gateway,
//! where apid signs with the VM's key) and the tool calls its model makes.
//!
//! - A code task: the builder's session opens the task's lane at its hello, writes files over two turns (drafts),
//!   describes its intents and finishes (sealed, with a `written` attestation). The reviewer's session, on another
//!   model, sends it back with a finding (`plaza_verdict` with pass false). The builder's next session, on a VM of its
//!   own, fixes it; the reviewer's second session passes it, and the change lands. It is exported to a git remote as
//!   one commit per intent, authored by the owner, with no AI credits.
//! - A research brief (no code): the reviewer's first verdict sends it back over a wrong number, the second passes it,
//!   and it lands.
//! - Every record verifies, from each signature to the owner's key.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// The CID noded gives the first VM it starts; each next one gets the next.
const FIRST_CID: u64 = 500;
/// How long a step waits for the hub (scaled by the slack of a slow run).
const WAIT: Duration = Duration::from_secs(40);
/// The models a project's seats use: the reviewer's differs from the builder's.
const BUILDER_MODEL: &str = "mock/agent";
const REVIEWER_MODEL: &str = "mock/echo";

const QUEUE: &str = "pub struct Queue {\n    pub items: Vec<u64>,\n}\n\nimpl Queue {\n    pub fn next(&mut self) -> Option<u64> {\n        self.items.pop()\n    }\n}\n";
const BRIEF_WRONG: &str = "# Edge caching\n\n## Findings\n\n- 93% of reads repeat within a task.\n\n## Recommendation\n\nCache on every node.\n";
const BRIEF_RIGHT: &str = "# Edge caching\n\n## Findings\n\n- 83% of reads repeat within a task (166 of 200, data/reads.csv).\n\n## Recommendation\n\nCache on every node.\n";
const READS: &str = "task,reads,repeats\nt1,120,101\nt2,80,65\n";

/// The VMs noded started, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// The roles the seats run: a builder and a reviewer.
fn roles() -> Value {
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "run_tests", "read"] });
    json!({ "roles": {
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } })
}

/// noded, which starts each session's VM (the next CID) and reports the VMs it started, and the agent gateway, which
/// takes the work the hub pushes to them.
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

/// The session of `seat` on `task` that runs on a VM `known` does not have: forking, its CID set.
fn next_session(hub: &Hub, task: &str, seat: &str, known: &HashSet<u64>) -> Value {
    let fresh = |s: &Value| {
        s["seat"] == seat
            && s["state"] == "forking"
            && s["cid"].as_u64().is_some_and(|cid| !known.contains(&cid))
    };
    let list = hub.wait_for(&format!("/v1/sessions?task={task}"), WAIT, |list| {
        list.as_array().is_some_and(|l| l.iter().any(&fresh))
    });
    list.as_array()
        .and_then(|l| l.iter().find(|s| fresh(s)).cloned())
        .unwrap_or_else(|| {
            panic!(
                "no new {seat} session for task {task}: {list}\n{}",
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
    home: PathBuf,
    token: PathBuf,
}

/// The VM `cid`'s session runs in: its workspace under `home`, the gateway's token at `token`.
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
        let env = BTreeMap::from([
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

    /// Checks `change` out in this VM's own workspace, `repo` under its home.
    fn checkout(&self, change: &str, repo: &str) -> PathBuf {
        let ws = self.home.join(repo);
        self.weft(
            &self.home,
            &["checkout", change, "--dir", ws.to_str().unwrap()],
        );
        ws
    }

    /// A tool call the model makes in work item `work`: the hub's decision, whatever it is.
    fn tool_reply(&self, work: u64, name: &str, input: Value) -> Value {
        let message = json!({ "type": "tool_request", "work": work, "call": format!("toolu_{name}_{work}"), "name": name, "input": input });
        let (status, body) = self.hub.guest(self.cid, "message", message);
        assert_eq!(status, 200, "{body}");
        body["reply"].clone()
    }

    /// A tool call the model makes in work item `work`, which must succeed: the hub's decision.
    fn tool(&self, work: u64, name: &str, input: Value) -> Value {
        let reply = self.tool_reply(work, name, input);
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

/// Whether the change carries an attestation of `kind` (its `written`, its `landed`).
fn has_record(change: &Value, kind: &str) -> bool {
    change["records"]
        .as_array()
        .is_some_and(|records| records.iter().any(|r| r["type"] == kind))
}

/// Whether a message in `chat` says `needle`.
fn chat_says(hub: &Hub, chat: &str, needle: &str) -> bool {
    let (_, messages) = hub.call(
        "GET",
        &format!("/v1/chats/{chat}/messages?after=0&limit=50"),
        None,
    );
    messages
        .as_array()
        .is_some_and(|m| m.iter().any(|m| text(&m["text"]).contains(needle)))
}

/// Polls `ready` until it holds, or the wait is over (a notice lands a moment after the state it reports).
fn eventually(hub: &Hub, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT.mul_f64(common::slack());
    while !ready() {
        assert!(Instant::now() < deadline, "{what}\n{}", hub.log());
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Every record of `change` verifies, from each signature to the owner's key. Returns who signed them.
fn verified_signers(hub: &Hub, change: &str) -> HashSet<String> {
    let verified = hub.wait_for(
        &format!("/v1/weft/verify?subject={change}"),
        Duration::from_secs(5),
        |v| v["ok"] == true,
    );
    assert_eq!(verified["ok"], true, "{verified}\n{}", hub.log());
    let deep = hub.wait_for("/v1/weft/verify?deep=1", Duration::from_secs(5), |v| {
        v["ok"] == true
    });
    assert_eq!(deep["ok"], true, "{deep}");
    verified["records"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| text(&r["actor"]))
        .collect()
}

/// What a code change does, as its builder names it: one intent per thing, with the paths each covers.
fn code_intents(summary: &str) -> Value {
    json!({ "intents": [
        { "title": "The queue hands out its oldest item first", "kind": "fix", "summary": summary, "covers": ["src/**"] },
        { "title": "Say how the queue orders work", "kind": "docs", "covers": ["README.md"] }
    ] })
}

/// The queue after the builder's first fix: next() takes the oldest item from the front.
fn fixed_queue() -> String {
    QUEUE.replace(
        "self.items.pop()",
        "if self.items.is_empty() {\n            return None;\n        }\n        Some(self.items.remove(0))",
    )
}

/// The queue after the builder's second session: the fix, with a test that the oldest item comes out first.
fn tested_queue() -> String {
    format!(
        "{}\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn the_oldest_goes_first() {{\n        let mut q = Queue {{ items: vec![1, 2] }};\n        assert_eq!(q.next(), Some(1));\n    }}\n}}\n",
        fixed_queue()
    )
}

/// The project's repo has the owner's Rust pack, as the owner adds it (every store starts with Markdown).
fn add_packs(hub: &Hub) {
    for name in ["rust"] {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../weft/packs")
            .join(name);
        let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("weft-npm");
        let pack = weft::packsrc::prepare(&dir, None, Some(&cache)).unwrap();
        for bytes in [&pack.wasm, &pack.query] {
            let h = weft::hash::Hash::of(bytes);
            let reply = apz_http::client::send(
                hub.port,
                "PUT",
                &format!("/v1/weft/wire/objects/{}", h.hex()),
                Some(bytes),
                Duration::from_secs(20),
            )
            .unwrap();
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
        }
        let (status, body) = hub.call(
            "POST",
            "/v1/weft/console",
            Some(json!({ "kind": "pack.add", "body": pack.pack })),
        );
        assert_eq!(status, 200, "{body}");
    }
}

#[test]
fn a_code_task_goes_from_its_builder_through_a_review_that_sends_it_back_to_a_landed_change() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let scratch = Scratch::new("weft-review");
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
    add_packs(&hub);
    let token = scratch.0.join("internal.token");
    std::fs::write(&token, hub.token()).unwrap();
    let home = scratch.0.join("vms");
    // The project's repo exports to a git remote (a bare repository standing in for GitHub).
    let remote = scratch.0.join("remote.git");
    let have_git = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&remote)
        .status()
        .is_ok_and(|s| s.success());

    // A code project, its builder and reviewer on two models, and the task.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Queues", "goal": "A work queue", "kind": "code" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": BUILDER_MODEL, "reviewer": REVIEWER_MODEL } })),
    );
    assert_eq!(status, 200, "{settings}");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Give the queue a fair next()", "acceptance": "next() takes the oldest item; tested", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);

    // The builder's session: its VM hellos, and its step's work carries the task's lane.
    let mut known = HashSet::new();
    let b1 = next_session(&hub, &tid, "builder", &known);
    let b1_cid = b1["cid"].as_u64().expect("its VM");
    known.insert(b1_cid);
    assert_eq!(b1["model"], BUILDER_MODEL, "{b1}");
    hello(&hub, b1_cid, "builder");
    let w1 = first_work(&hub, &gateway, b1_cid);
    let w1_id = w1["work"].as_u64().expect("the work item");
    let change = text(&w1["lane"]["change"]);
    let repo = text(&w1["lane"]["repo"]);
    assert!(
        change.starts_with("w-") && text(&w1["prompt"]).contains(&change),
        "{w1}"
    );
    assert!(
        text(&w1["prompt"]).contains("Give the queue a fair next()"),
        "{w1}"
    );
    if have_git {
        let (status, body) = hub.call(
            "POST",
            "/v1/weft/console",
            Some(json!({ "kind": "connection.add", "body": { "repo": repo, "id": "origin", "remote": format!("file://{}", remote.display()), "platform": "git", "branch": "main", "author_name": "Irfan Saric", "author_email": "owner@example.com" } })),
        );
        assert_eq!(status, 200, "{body}");
    }

    // The builder's first session: a checkout in its workspace, two turns (drafts), its intents, and its done.
    let b1_vm = vm_of(&hub, b1_cid, &home, &token);
    let ws1 = b1_vm.checkout(&change, &repo);
    b1_vm.write(&format!("{repo}/src/queue.rs"), QUEUE);
    b1_vm.weft(
        &ws1,
        &[
            "snapshot",
            "--turn",
            "1",
            "--tool-call",
            "toolu_a",
            "--model",
            "claude-opus-5-5",
        ],
    );
    b1_vm.write(&format!("{repo}/src/queue.rs"), &fixed_queue());
    b1_vm.write(
        &format!("{repo}/README.md"),
        "# Queues\n\nnext() takes the oldest item.\n",
    );
    b1_vm.weft(
        &ws1,
        &[
            "snapshot",
            "--turn",
            "2",
            "--tool-call",
            "toolu_b",
            "--model",
            "claude-opus-5-5",
        ],
    );
    b1_vm.tool(
        w1_id,
        "weft_describe",
        code_intents("next() takes from the front."),
    );
    let status = b1_vm.tool(w1_id, "weft_status", json!({}));
    assert!(text(&status["output"]).contains("src/queue.rs"), "{status}");
    hub.work_done(b1_cid, w1_id, "done", "next() now takes the oldest item");
    // Sealed for review, with the builder's `written` attestation.
    let sealed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(20),
        |c| c["rounds"].as_array().is_some_and(|r| r.len() == 1) && has_record(c, "written"),
    );
    assert_eq!(
        sealed["rounds"].as_array().map(Vec::len),
        Some(1),
        "{sealed}\n{}",
        hub.log()
    );
    assert!(has_record(&sealed, "written"), "{sealed}");

    // The reviewer's session, on another model: its first read is the builder's handover. Its verdict sends it back.
    let r1 = next_session(&hub, &tid, "reviewer", &known);
    let r1_cid = r1["cid"].as_u64().expect("its VM");
    known.insert(r1_cid);
    assert_eq!(
        r1["model"], REVIEWER_MODEL,
        "a model other than the builder's: {r1}"
    );
    hello(&hub, r1_cid, "reviewer");
    let rw1 = first_work(&hub, &gateway, r1_cid);
    let rw1_id = rw1["work"].as_u64().expect("the work item");
    assert!(
        text(&rw1["prompt"]).contains("The builder's handover"),
        "{rw1}"
    );
    let r1_vm = vm_of(&hub, r1_cid, &home, &token);
    let verdict = r1_vm.tool(
        rw1_id,
        "plaza_verdict",
        json!({ "task": tid, "pass": false, "notes": "next() has no test: add one that checks the oldest item comes out first." }),
    );
    assert!(
        text(&verdict["output"]).contains("goes back to its builder"),
        "{verdict}"
    );
    hub.work_done(r1_cid, rw1_id, "done", "sent back: next() has no test");
    let sent_back = hub.wait_for(&format!("/v1/tasks/{tid}"), WAIT, |t| {
        t["state"] == "rework"
    });
    assert_eq!(sent_back["state"], "rework", "{sent_back}\n{}", hub.log());

    // The builder's next session runs on a VM of its own, with the finding in its prompt. It checks its lane out in
    // its own workspace, writes the test and finishes: the change is sealed for a second review.
    let b2 = next_session(&hub, &tid, "builder", &known);
    let b2_cid = b2["cid"].as_u64().expect("its VM");
    known.insert(b2_cid);
    hello(&hub, b2_cid, "builder");
    let w2 = first_work(&hub, &gateway, b2_cid);
    let w2_id = w2["work"].as_u64().expect("the work item");
    assert!(text(&w2["prompt"]).contains("next() has no test"), "{w2}");
    assert_eq!(w2["lane"]["change"], change.as_str(), "{w2}");
    let b2_vm = vm_of(&hub, b2_cid, &home, &token);
    let ws2 = b2_vm.checkout(&change, &repo);
    b2_vm.write(&format!("{repo}/src/queue.rs"), &tested_queue());
    b2_vm.weft(
        &ws2,
        &[
            "snapshot",
            "--turn",
            "3",
            "--tool-call",
            "toolu_c",
            "--model",
            "claude-opus-5-5",
        ],
    );
    b2_vm.tool(
        w2_id,
        "weft_describe",
        code_intents("next() takes from the front, with a test."),
    );
    hub.work_done(b2_cid, w2_id, "done", "tested");
    let resealed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(20),
        |c| c["rounds"].as_array().is_some_and(|r| r.len() == 2),
    );
    assert_eq!(
        resealed["rounds"].as_array().map(Vec::len),
        Some(2),
        "{resealed}\n{}",
        hub.log()
    );

    // The second review passes it: the change joins its repo's landing queue and lands.
    let r2 = next_session(&hub, &tid, "reviewer", &known);
    let r2_cid = r2["cid"].as_u64().expect("its VM");
    known.insert(r2_cid);
    hello(&hub, r2_cid, "reviewer");
    let rw2 = first_work(&hub, &gateway, r2_cid);
    let rw2_id = rw2["work"].as_u64().expect("the work item");
    assert!(
        text(&rw2["prompt"]).contains("The builder's handover:\ntested\n"),
        "{rw2}"
    );
    let r2_vm = vm_of(&hub, r2_cid, &home, &token);
    let verdict = r2_vm.tool(
        rw2_id,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "fine" }),
    );
    assert!(text(&verdict["output"]).contains("passes"), "{verdict}");
    hub.work_done(r2_cid, rw2_id, "done", "passed");

    // It lands: the change is on main, the task is done, its chats say so, and the change is exported to the remote.
    let landed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(30),
        // The landing's own sign-off record follows the landing itself.
        |c| c["state"] == "landed" && has_record(c, "landed"),
    );
    assert_eq!(landed["state"], "landed", "{landed}\n{}", hub.log());
    assert!(
        has_record(&landed, "written") && has_record(&landed, "landed"),
        "{landed}"
    );
    assert!(
        landed["records"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r["type"] == "landed" && r["actor"] == "hub"),
        "the landing is the hub's: {landed}"
    );
    let done = hub.wait_for(&format!("/v1/tasks/{tid}"), Duration::from_secs(20), |t| {
        t["state"] == "done"
    });
    assert_eq!(done["state"], "done", "{done}\n{}", hub.log());
    let landing = format!("{change} landed");
    eventually(&hub, "the task's chat says it landed", || {
        chat_says(&hub, &format!("task-{tid}"), &landing)
    });
    eventually(&hub, "the project's chat says it landed", || {
        let (_, chats) = hub.call("GET", &format!("/v1/chats?project={pid}"), None);
        chats
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["kind"] == "project")
            .any(|c| {
                chat_says(
                    &hub,
                    &text(&c["id"]),
                    &format!("{change} landed: task {tid} is done"),
                )
            })
    });
    if have_git {
        let exported = hub.wait_for(
            &format!("/v1/weft/repos/{repo}"),
            Duration::from_secs(20),
            |r| r["connections"][0]["exported"] == 1,
        );
        assert_eq!(
            exported["connections"][0]["exported"],
            1,
            "{exported}\n{}",
            hub.log()
        );
        let out = std::process::Command::new("git")
            .args(["log", "--format=%an <%ae>|%s|%b%x00"])
            .current_dir(&remote)
            .output()
            .unwrap();
        let commits: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(commits.len(), 2, "{commits:?}");
        assert!(commits[1].starts_with("Irfan Saric <owner@example.com>|The queue hands out its oldest item first|next() takes from the front, with a test."), "{commits:?}");
        assert!(
            commits[0].starts_with("Irfan Saric <owner@example.com>|Say how the queue orders work"),
            "{commits:?}"
        );
        assert!(
            commits
                .iter()
                .all(|c| !c.to_lowercase().contains("co-authored") && !c.contains("agent")),
            "{commits:?}"
        );
    }
    let head = hub.call("GET", &format!("/v1/weft/repos/{repo}"), None).1["refs"]["main"].clone();
    let main_file = apz_http::client::send(
        hub.port,
        "GET",
        &format!("/v1/weft/revisions/{}/file?path=src/queue.rs", text(&head)),
        None,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(
        main_file.status,
        200,
        "the landed file is on main: {}",
        String::from_utf8_lossy(&main_file.body)
    );

    // Every record verifies, from each signature to the owner's key: the builder's two VMs and the hub.
    let signers = verified_signers(&hub, &change);
    for who in [
        format!("agent:{b1_cid}"),
        format!("agent:{b2_cid}"),
        "hub".to_string(),
    ] {
        assert!(
            signers.contains(&who),
            "{who} signed something: {signers:?}"
        );
    }

    // With WEFT_SAMPLES set, the answers the consoles draw, kept as examples.
    if std::env::var_os("WEFT_SAMPLES").is_some() {
        let (_, v) = hub.call(
            "POST",
            &format!("/v1/weft/repos/{repo}/releases"),
            Some(json!({ "version": "v0.1.0" })),
        );
        sample("release-proposed", &v);
        for (name, path) in [
            ("weft", "/v1/weft".to_string()),
            ("repos", "/v1/weft/repos".to_string()),
            ("repo", format!("/v1/weft/repos/{repo}")),
            ("log", format!("/v1/weft/repos/{repo}/log?q=all()")),
            ("change", format!("/v1/weft/changes/{change}")),
            ("revision", format!("/v1/weft/revisions/{change}.r2")),
            ("files", format!("/v1/weft/revisions/{change}.l/files")),
            (
                "change-tree",
                format!("/v1/weft/revisions/{change}.r2/tree"),
            ),
            ("diff", format!("/v1/weft/revisions/{change}.r2/diff")),
            ("queue", format!("/v1/weft/repos/{repo}/queue")),
            ("releases", format!("/v1/weft/repos/{repo}/releases")),
            ("map", format!("/v1/weft/repos/{repo}/map")),
            (
                "map-where",
                format!("/v1/weft/repos/{repo}/map/where?q=queue"),
            ),
            ("ops", "/v1/weft/ops?limit=20".to_string()),
            ("keys", "/v1/weft/keys".to_string()),
            ("packs", "/v1/weft/packs".to_string()),
            ("verify-change", format!("/v1/weft/verify?subject={change}")),
            ("artifacts", "/v1/weft/artifacts".to_string()),
            ("task", format!("/v1/tasks/{tid}")),
        ] {
            let (_, v) = hub.call("GET", &path, None);
            sample(name, &v);
        }
    }
}

#[test]
fn a_research_brief_with_no_code_is_sent_back_over_a_wrong_number_and_lands() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let scratch = Scratch::new("weft-brief");
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

    // A project without code, its builder and reviewer on two models, and the brief's task.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Edge caching", "goal": "Should nodes cache objects?", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": BUILDER_MODEL, "reviewer": REVIEWER_MODEL } })),
    );
    assert_eq!(status, 200, "{settings}");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Write a brief on edge caching, with numbers", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);

    // The builder writes the brief and its data, and says what the brief measures.
    let mut known = HashSet::new();
    let b1 = next_session(&hub, &tid, "builder", &known);
    let b1_cid = b1["cid"].as_u64().expect("its VM");
    known.insert(b1_cid);
    hello(&hub, b1_cid, "builder");
    let w1 = first_work(&hub, &gateway, b1_cid);
    let w1_id = w1["work"].as_u64().expect("the work item");
    let change = text(&w1["lane"]["change"]);
    let repo = text(&w1["lane"]["repo"]);
    assert!(
        change.starts_with("w-") && text(&w1["prompt"]).contains(&change),
        "{w1}"
    );
    let b1_vm = vm_of(&hub, b1_cid, &home, &token);
    let ws1 = b1_vm.checkout(&change, &repo);
    b1_vm.write(&format!("{repo}/briefs/edge-caching.md"), BRIEF_WRONG);
    b1_vm.write(&format!("{repo}/data/reads.csv"), READS);
    b1_vm.weft(
        &ws1,
        &["snapshot", "--turn", "1", "--model", "claude-opus-5-5"],
    );
    b1_vm.tool(
        w1_id,
        "weft_describe",
        json!({ "intents": [{ "title": "Measure how often reads repeat", "kind": "research", "covers": ["briefs/**", "data/**"] }] }),
    );
    hub.work_done(b1_cid, w1_id, "done", "brief written");
    let sealed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(20),
        |c| c["rounds"].as_array().is_some_and(|r| r.len() == 1) && has_record(c, "written"),
    );
    assert_eq!(
        sealed["rounds"].as_array().map(Vec::len),
        Some(1),
        "{sealed}\n{}",
        hub.log()
    );

    // The reviewer's first verdict: 166 of 200 reads repeat, which is 83%, not the brief's 93%.
    let r1 = next_session(&hub, &tid, "reviewer", &known);
    let r1_cid = r1["cid"].as_u64().expect("its VM");
    known.insert(r1_cid);
    assert_eq!(r1["model"], REVIEWER_MODEL, "{r1}");
    hello(&hub, r1_cid, "reviewer");
    let rw1 = first_work(&hub, &gateway, r1_cid);
    let rw1_id = rw1["work"].as_u64().expect("the work item");
    assert!(
        text(&rw1["prompt"]).contains("The builder's handover:\nbrief written\n"),
        "{rw1}"
    );
    let r1_vm = vm_of(&hub, r1_cid, &home, &token);
    let verdict = r1_vm.tool(
        rw1_id,
        "plaza_verdict",
        json!({ "task": tid, "pass": false, "notes": "the data says 83%, not 93%: data/reads.csv has 166 of 200 reads repeating" }),
    );
    assert!(
        text(&verdict["output"]).contains("goes back to its builder"),
        "{verdict}"
    );
    hub.work_done(r1_cid, rw1_id, "done", "sent back: the number is wrong");
    let sent_back = hub.wait_for(&format!("/v1/tasks/{tid}"), WAIT, |t| {
        t["state"] == "rework"
    });
    assert_eq!(sent_back["state"], "rework", "{sent_back}\n{}", hub.log());

    // The builder's next session, on a VM of its own, is sent the finding; it corrects the number in its own
    // workspace, and the brief is sealed for a second review.
    let b2 = next_session(&hub, &tid, "builder", &known);
    let b2_cid = b2["cid"].as_u64().expect("its VM");
    known.insert(b2_cid);
    hello(&hub, b2_cid, "builder");
    let w2 = first_work(&hub, &gateway, b2_cid);
    let w2_id = w2["work"].as_u64().expect("the work item");
    assert!(text(&w2["prompt"]).contains("83%"), "{w2}");
    let b2_vm = vm_of(&hub, b2_cid, &home, &token);
    let ws2 = b2_vm.checkout(&change, &repo);
    b2_vm.write(&format!("{repo}/briefs/edge-caching.md"), BRIEF_RIGHT);
    b2_vm.weft(
        &ws2,
        &["snapshot", "--turn", "2", "--model", "claude-opus-5-5"],
    );
    hub.work_done(b2_cid, w2_id, "done", "fixed the number");
    let resealed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(20),
        |c| c["rounds"].as_array().is_some_and(|r| r.len() == 2),
    );
    assert_eq!(
        resealed["rounds"].as_array().map(Vec::len),
        Some(2),
        "{resealed}\n{}",
        hub.log()
    );

    // The second verdict passes it, and it lands.
    let r2 = next_session(&hub, &tid, "reviewer", &known);
    let r2_cid = r2["cid"].as_u64().expect("its VM");
    known.insert(r2_cid);
    hello(&hub, r2_cid, "reviewer");
    let rw2 = first_work(&hub, &gateway, r2_cid);
    let rw2_id = rw2["work"].as_u64().expect("the work item");
    let r2_vm = vm_of(&hub, r2_cid, &home, &token);
    let verdict = r2_vm.tool(
        rw2_id,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "the number matches data/reads.csv" }),
    );
    assert!(text(&verdict["output"]).contains("passes"), "{verdict}");
    hub.work_done(r2_cid, rw2_id, "done", "passed");
    let landed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(30),
        // The landing's own sign-off record follows the landing itself.
        |c| c["state"] == "landed" && has_record(c, "landed"),
    );
    assert_eq!(landed["state"], "landed", "{landed}\n{}", hub.log());
    assert!(
        has_record(&landed, "written") && has_record(&landed, "landed"),
        "{landed}"
    );
    let done = hub.wait_for(&format!("/v1/tasks/{tid}"), Duration::from_secs(20), |t| {
        t["state"] == "done"
    });
    assert_eq!(done["state"], "done", "{done}\n{}", hub.log());

    // Who wrote the corrected line: the second builder session's VM.
    let (_, blame) = hub.call(
        "GET",
        &format!("/v1/weft/repos/{repo}/blame?path=briefs/edge-caching.md"),
        None,
    );
    let line = blame["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|l| text(&l["text"]).contains("83%"))
        .cloned()
        .unwrap_or_else(|| panic!("no corrected line: {blame}"));
    assert_eq!(line["by"]["by"]["cid"], b2_cid, "{line}");

    let signers = verified_signers(&hub, &change);
    for who in [
        format!("agent:{b1_cid}"),
        format!("agent:{b2_cid}"),
        "hub".to_string(),
    ] {
        assert!(
            signers.contains(&who),
            "{who} signed something: {signers:?}"
        );
    }
}

fn sample(name: &str, v: &Value) {
    if let Some(dir) = std::env::var_os("WEFT_SAMPLES") {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_string_pretty(v).unwrap_or_default(),
        );
    }
}
