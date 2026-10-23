//! Weft through the work engine, end to end on a scratch hub, with the sessions' VMs played by this test. A session's VM
//! is a run noded starts for the hub: the test takes the CID noded gives it, hellos as that VM, reads the work the hub
//! pushes to it (through the agent gateway), and plays what its harness does: the `weft` commands (through the gateway,
//! where apid signs with the VM's key) and the tool calls its model makes.
//!
//! The change recipe's review is the adversarial chain, through the seats (8.2): two sceptic reviewers on models other
//! than the builder's review the sealed change in parallel; a verifier then proves each finding or drops it, and a
//! confirmed finding sends the builder back with it. Then the chain re-reviews what changed.
//!
//! - A code task: the builder's queue has a seeded bug (next() takes the newest item). One reviewer finds it (with its
//!   scenario); the other reports a false alarm (next() on an empty queue panics: it does not, pop() returns None). The
//!   verifier proves the bug with a failing test and drops the false alarm with its reason. The builder is sent back
//!   with the confirmed finding and the dropped one, fixes the queue and adds the test; the second round passes, the
//!   verifier finds nothing to prove, and the change lands. It is exported to a git remote as one commit per intent,
//!   authored by the owner, with no AI credits.
//! - A research brief (no code): a reviewer finds the wrong number, the verifier confirms it with the data's own
//!   lines, the builder corrects it, and the second round passes it, and it lands.
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
/// The verifier's model: a third one, which no other seat of the task runs on.
const VERIFIER_MODEL: &str = "mock/third";

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
        "verifier": role("verifier", "Verifier"),
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
    // A seat of several items is named `reviewer#1`, `reviewer#2`, …: a session of any of them is `seat`'s.
    let of_seat = |name: &str| name == seat || name.starts_with(&format!("{seat}#"));
    let fresh = |s: &Value| {
        s["seat"].as_str().is_some_and(of_seat)
            && s["state"] == "forking"
            && s["cid"].as_u64().is_some_and(|cid| !known.contains(&cid))
    };
    let list = hub.wait_for(&format!("/v1/sessions?task={task}"), WAIT, |list| {
        list.as_array().is_some_and(|l| l.iter().any(&fresh))
    });
    // The first of the fresh sessions by seat name: `reviewer#1` before `reviewer#2`.
    list.as_array()
        .and_then(|l| {
            l.iter()
                .filter(|s| fresh(s))
                .min_by_key(|s| s["seat"].as_str().unwrap_or("").to_string())
                .cloned()
        })
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

/// The next session of `seat` on `task`, on a VM of its own: hellos it and reads its first work. Returns the VM's cid,
/// the work item, the session, and the work the hub pushed to the VM.
fn seat_vm(
    hub: &Hub,
    gateway: &FakeDaemon,
    task: &str,
    seat: &str,
    known: &mut HashSet<u64>,
) -> (u64, u64, Value, Value) {
    let session = next_session(hub, task, seat, known);
    let cid = session["cid"].as_u64().expect("its VM");
    known.insert(cid);
    hello(hub, cid, seat);
    let work = first_work(hub, gateway, cid);
    let id = work["work"].as_u64().expect("the work item");
    (cid, id, session, work)
}

#[test]
fn a_code_change_through_the_adversarial_chain_finds_a_seeded_bug_proves_it_fixes_it_and_drops_a_false_alarm()
 {
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

    // A code project: its builder, its reviewers and its verifier, on models of their own, and the task.
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
        Some(json!({ "role_models": { "builder": BUILDER_MODEL, "reviewer": REVIEWER_MODEL, "verifier": VERIFIER_MODEL } })),
    );
    assert_eq!(status, 200, "{settings}");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Give the queue a fair next()", "acceptance": "next() takes the oldest item; tested", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);

    // The builder's session writes the queue, with the seeded bug (next() takes the newest item), and seals it.
    let mut known = HashSet::new();
    let (b1_cid, w1_id, b1_session, w1) = seat_vm(&hub, &gateway, &tid, "builder", &mut known);
    assert_eq!(b1_session["model"], BUILDER_MODEL, "{b1_session}");
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
    b1_vm.tool(
        w1_id,
        "weft_describe",
        code_intents("next() takes from the front."),
    );
    let status = b1_vm.tool(w1_id, "weft_status", json!({}));
    assert!(text(&status["output"]).contains("src/queue.rs"), "{status}");
    hub.work_done(b1_cid, w1_id, "done", "next() takes the oldest item");
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

    // Round 1: two sceptic reviewers, in parallel, on models other than the builder's. The first records the seeded
    // bug with the scenario that shows it; the second records a false alarm (next() on an empty queue panics: it
    // does not, pop() returns None).
    let (r1a_cid, r1a_work, r1a_session, r1a) =
        seat_vm(&hub, &gateway, &tid, "reviewer", &mut known);
    assert_eq!(r1a_session["model"], REVIEWER_MODEL, "{r1a_session}");
    assert!(text(&r1a["prompt"]).contains("sceptic"), "{r1a}");
    let r1a_vm = vm_of(&hub, r1a_cid, &home, &token);
    r1a_vm.tool(
        r1a_work,
        "review_score",
        json!({ "scores": [], "findings": [{
            "symbol": "src/queue.rs::Queue::next",
            "text": "next() takes the newest item, not the oldest",
            "severity": "major",
            "kind": "order",
            "scenario": "push 1, then push 2: next() returns 2, and the oldest item, 1, waits behind it"
        }] }),
    );
    let reported = r1a_vm.tool(
        r1a_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "one defect: the order" }),
    );
    assert!(text(&reported["output"]).contains("reported"), "{reported}");
    hub.work_done(r1a_cid, r1a_work, "done", "reviewed");

    let (r1b_cid, r1b_work, r1b_session, _) = seat_vm(&hub, &gateway, &tid, "reviewer", &mut known);
    assert_ne!(r1b_session["model"], BUILDER_MODEL, "{r1b_session}");
    let r1b_vm = vm_of(&hub, r1b_cid, &home, &token);
    r1b_vm.tool(
        r1b_work,
        "review_score",
        json!({ "scores": [], "findings": [{
            "symbol": "src/queue.rs::Queue::next",
            "text": "next() panics on an empty queue",
            "severity": "minor",
            "kind": "panic",
            "scenario": "next() on an empty queue calls unwrap() on None and panics"
        }] }),
    );
    let reported = r1b_vm.tool(
        r1b_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "one finding" }),
    );
    assert!(text(&reported["output"]).contains("reported"), "{reported}");
    hub.work_done(r1b_cid, r1b_work, "done", "reviewed");

    // The verifier, on a third model, proves the defect with a test and drops the false alarm with its reason.
    let (v1_cid, v1_work, v1_session, v1) = seat_vm(&hub, &gateway, &tid, "verifier", &mut known);
    assert_eq!(v1_session["model"], VERIFIER_MODEL, "{v1_session}");
    assert!(
        text(&v1["prompt"]).contains("f1") && text(&v1["prompt"]).contains("f2"),
        "{v1}"
    );
    let v1_vm = vm_of(&hub, v1_cid, &home, &token);
    v1_vm.tool(
        v1_work,
        "finding_verify",
        json!({ "finding": "f1", "verdict": "confirmed", "proof": "a test: push 1, push 2, next() returns 2, so the oldest item does not come out first" }),
    );
    v1_vm.tool(
        v1_work,
        "finding_verify",
        json!({ "finding": "f2", "verdict": "dropped", "reason": "pop() on an empty Vec returns None: next() does not panic on an empty queue" }),
    );
    let verdict = v1_vm.tool(
        v1_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "f1 is real, f2 is not" }),
    );
    assert!(
        text(&verdict["output"]).contains("goes back to its builder"),
        "{verdict}"
    );
    hub.work_done(v1_cid, v1_work, "done", "f1 confirmed, f2 dropped");
    let sent_back = hub.wait_for(&format!("/v1/tasks/{tid}"), WAIT, |t| {
        t["state"] == "rework"
    });
    assert_eq!(sent_back["state"], "rework", "{sent_back}\n{}", hub.log());

    // The builder's next session is sent the confirmed finding with its scenario and proof, and the dropped one with its
    // reason. It fixes the queue, adds the test, and seals a second revision.
    let (b2_cid, w2_id, _, w2) = seat_vm(&hub, &gateway, &tid, "builder", &mut known);
    let prompt2 = text(&w2["prompt"]);
    assert!(prompt2.contains("Scenario: push 1, then push 2"), "{w2}");
    assert!(
        prompt2.contains("not a defect: pop() on an empty Vec returns None"),
        "{w2}"
    );
    assert_eq!(w2["lane"]["change"], change.as_str(), "{w2}");
    let b2_vm = vm_of(&hub, b2_cid, &home, &token);
    // The author contests the first finding in its rework session: the verifier looks again in the next round.
    b2_vm.tool(
        w2_id,
        "finding_contest",
        json!({ "finding": "f1", "reason": "the scenario does not hold for this queue" }),
    );
    let ws2 = b2_vm.checkout(&change, &repo);
    b2_vm.write(&format!("{repo}/src/queue.rs"), &tested_queue());
    b2_vm.write(
        &format!("{repo}/README.md"),
        "# Queues\n\nnext() takes the oldest item.\n",
    );
    b2_vm.weft(
        &ws2,
        &[
            "snapshot",
            "--turn",
            "2",
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

    // Round 2: the reviewers pass the fix. The verifier re-rules the contested finding: its proof holds, so the verifier
    // holds the contest, and the walk waits for a ruling. No planner is free on another model, so the owner's card
    // rules: the finding is accepted, the walk goes on, and the change passes the chain.
    for _ in 0..2 {
        let (cid, work, session, _) = seat_vm(&hub, &gateway, &tid, "reviewer", &mut known);
        assert_ne!(session["model"], BUILDER_MODEL, "{session}");
        let vm = vm_of(&hub, cid, &home, &token);
        let reported = vm.tool(
            work,
            "plaza_verdict",
            json!({ "task": tid, "pass": true, "notes": "fine" }),
        );
        assert!(text(&reported["output"]).contains("reported"), "{reported}");
        hub.work_done(cid, work, "done", "passed");
    }
    let (v2_cid, v2_work, v2_session, v2) = seat_vm(&hub, &gateway, &tid, "verifier", &mut known);
    assert_eq!(v2_session["model"], VERIFIER_MODEL, "{v2_session}");
    assert!(
        text(&v2["prompt"]).contains("The author contests f1"),
        "{v2}"
    );
    let v2_vm = vm_of(&hub, v2_cid, &home, &token);
    v2_vm.tool(
        v2_work,
        "finding_verify",
        json!({ "finding": "f1", "verdict": "confirmed", "proof": "the same test: next() returns 2, so the oldest item is not first" }),
    );
    let held = v2_vm.tool(
        v2_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "the contest does not hold" }),
    );
    assert!(text(&held["output"]).contains("Held"), "{held}");
    let card = hub.wait_for("/v1/approvals", WAIT, |a| {
        a.as_array().is_some_and(|a| {
            a.iter()
                .any(|x| x["detail"]["gate"] == "contest" && x["state"] == "pending")
        })
    });
    let card = card
        .as_array()
        .and_then(|a| a.iter().find(|x| x["detail"]["gate"] == "contest").cloned())
        .unwrap_or_else(|| panic!("a contest card: {card}\n{}", hub.log()));
    let (status, decided) = hub.call(
        "POST",
        &format!("/v1/approvals/{}", text(&card["id"])),
        Some(json!({ "approve": true, "action": "accept", "reason": "the scenario does not hold for this queue" })),
    );
    assert_eq!(status, 200, "{decided}\n{}", hub.log());
    eventually(&hub, "the task's chat says the owner ruled", || {
        chat_says(&hub, &format!("task-{tid}"), "the owner ruled accept on f1")
    });
    hub.work_done(v2_cid, v2_work, "done", "held, then ruled");

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
fn a_research_brief_is_sent_back_over_a_wrong_number_through_the_chain_and_lands() {
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

    // A project without code, its builder, reviewers and verifier on models of their own, and the brief's task.
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
        Some(json!({ "role_models": { "builder": BUILDER_MODEL, "reviewer": REVIEWER_MODEL, "verifier": VERIFIER_MODEL } })),
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
    let (b1_cid, w1_id, _, w1) = seat_vm(&hub, &gateway, &tid, "builder", &mut known);
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

    // Round 1: a reviewer finds the wrong number (166 of 200 reads repeat, which is 83%, not the brief's 93%); the other
    // passes the brief. The verifier confirms the finding from the data's own lines.
    let (r1a_cid, r1a_work, r1a_session, r1a) =
        seat_vm(&hub, &gateway, &tid, "reviewer", &mut known);
    assert_eq!(r1a_session["model"], REVIEWER_MODEL, "{r1a_session}");
    assert!(text(&r1a["prompt"]).contains("sceptic"), "{r1a}");
    let r1a_vm = vm_of(&hub, r1a_cid, &home, &token);
    r1a_vm.tool(
        r1a_work,
        "review_score",
        json!({ "scores": [], "findings": [{
            "symbol": "briefs/edge-caching.md::Findings",
            "text": "93% of reads repeat, but the data says 83%",
            "severity": "major",
            "kind": "number",
            "scenario": "the brief's 93% against data/reads.csv: 166 of 200 reads repeat, which is 83%"
        }] }),
    );
    let reported = r1a_vm.tool(
        r1a_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "one wrong number" }),
    );
    assert!(text(&reported["output"]).contains("reported"), "{reported}");
    hub.work_done(r1a_cid, r1a_work, "done", "reviewed");
    let (r1b_cid, r1b_work, _, _) = seat_vm(&hub, &gateway, &tid, "reviewer", &mut known);
    let r1b_vm = vm_of(&hub, r1b_cid, &home, &token);
    let reported = r1b_vm.tool(
        r1b_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "the sources are fine" }),
    );
    assert!(text(&reported["output"]).contains("reported"), "{reported}");
    hub.work_done(r1b_cid, r1b_work, "done", "reviewed");

    let (v1_cid, v1_work, _, v1) = seat_vm(&hub, &gateway, &tid, "verifier", &mut known);
    assert!(text(&v1["prompt"]).contains("f1"), "{v1}");
    let v1_vm = vm_of(&hub, v1_cid, &home, &token);
    v1_vm.tool(
        v1_work,
        "finding_verify",
        json!({ "finding": "f1", "verdict": "confirmed", "proof": "data/reads.csv: 166 of 200 reads repeat, which is 83%; the brief says 93%" }),
    );
    let verdict = v1_vm.tool(
        v1_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "the number is wrong" }),
    );
    assert!(
        text(&verdict["output"]).contains("goes back to its builder"),
        "{verdict}"
    );
    hub.work_done(v1_cid, v1_work, "done", "sent back: the number is wrong");
    let sent_back = hub.wait_for(&format!("/v1/tasks/{tid}"), WAIT, |t| {
        t["state"] == "rework"
    });
    assert_eq!(sent_back["state"], "rework", "{sent_back}\n{}", hub.log());

    // The builder corrects the number and seals a second revision.
    let (b2_cid, w2_id, _, w2) = seat_vm(&hub, &gateway, &tid, "builder", &mut known);
    assert!(text(&w2["prompt"]).contains("83%"), "{w2}");
    let b2_vm = vm_of(&hub, b2_cid, &home, &token);
    let ws2 = b2_vm.checkout(&change, &repo);
    b2_vm.write(&format!("{repo}/briefs/edge-caching.md"), BRIEF_RIGHT);
    b2_vm.weft(
        &ws2,
        &["snapshot", "--turn", "2", "--model", "claude-opus-5-5"],
    );
    b2_vm.tool(
        w2_id,
        "weft_describe",
        json!({ "intents": [{ "title": "Measure how often reads repeat", "kind": "research", "covers": ["briefs/**", "data/**"] }] }),
    );
    hub.work_done(b2_cid, w2_id, "done", "corrected");
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

    // Round 2 passes it, and the brief lands.
    for _ in 0..2 {
        let (cid, work, _, _) = seat_vm(&hub, &gateway, &tid, "reviewer", &mut known);
        let vm = vm_of(&hub, cid, &home, &token);
        vm.tool(
            work,
            "plaza_verdict",
            json!({ "task": tid, "pass": true, "notes": "the number is right" }),
        );
        hub.work_done(cid, work, "done", "passed");
    }
    let (v2_cid, v2_work, _, _) = seat_vm(&hub, &gateway, &tid, "verifier", &mut known);
    let v2_vm = vm_of(&hub, v2_cid, &home, &token);
    let verdict = v2_vm.tool(
        v2_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "nothing to prove" }),
    );
    assert!(text(&verdict["output"]).contains("passes"), "{verdict}");
    hub.work_done(v2_cid, v2_work, "done", "passed");
    let landed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(30),
        |c| c["state"] == "landed",
    );
    assert_eq!(landed["state"], "landed", "{landed}\n{}", hub.log());
    let done = hub.wait_for(&format!("/v1/tasks/{tid}"), Duration::from_secs(20), |t| {
        t["state"] == "done"
    });
    assert_eq!(done["state"], "done", "{done}\n{}", hub.log());
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
