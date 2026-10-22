//! Weft and the review graph, end to end on a scratch hub, with the agents' VMs played by this test: the `weft`
//! commands a harness runs (through the agent gateway, where apid signs with each agent's key) and the tool calls a
//! model makes.
//!
//! - A code task: claimed (its lane opens), two turns as drafts, intents, done (sealed, `written`), reviewed by the
//!   code recipe; the qa step fails it, so it goes back to its worker (`rework`) with the finding; round 2 scores a
//!   security risk, so the owner gets the review card and passes it; the change lands and is exported to a git
//!   remote as one commit per intent, authored by the owner, with no AI credits.
//! - A research brief (no code): the research recipe re-runs its numbers, finds them wrong (a measured accuracy
//!   score), sends it back; the next round passes and lands.
//! - Every record verifies, from each signature to the owner's key.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const AGENTS: &[(u64, &str)] = &[
    (300, "builder"),
    (301, "reviewer"),
    (302, "security-reviewer"),
    (303, "qa"),
    (305, "researcher"),
    (306, "reviewer"),
    (307, "reviewer"),
];

fn daemons() -> (FakeDaemon, FakeDaemon) {
    let noded = FakeDaemon::serve(|method, path, _| {
        if method == "GET" && path.starts_with("/v1/runs") {
            let vms: Vec<Value> = AGENTS
                .iter()
                .enumerate()
                .map(|(i, (cid, role))| json!({ "id": i, "guest_cid": cid, "role": role }))
                .collect();
            (
                200,
                json!({ "runs": [{ "id": "r1", "ended_at": null, "vms": vms }] }).to_string(),
            )
        } else {
            (200, "{}".into())
        }
    });
    let gateway = FakeDaemon::serve(|_, path, _| {
        if path.starts_with("/v1/status") {
            let agents: Vec<Value> = AGENTS
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

/// The work items the hub sent to `cid`'s VM, oldest first.
fn works(gateway: &FakeDaemon, cid: u64) -> Vec<Value> {
    gateway
        .seen()
        .into_iter()
        .filter(|(m, p, _)| m == "POST" && p == &format!("/v1/agents/{cid}/send"))
        .filter_map(|(_, _, body)| serde_json::from_str::<Value>(&body).ok())
        .map(|v| v["message"].clone())
        .filter(|m| m["type"] == "work")
        .collect()
}

/// Answers the chats the hub sends (an agent's first one asks it to pick a name), so its tasks come next.
fn answer_chats(hub: &Hub, gateway: &FakeDaemon, cid: u64) {
    static DONE: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    for w in works(gateway, cid) {
        let id = w["work"].as_u64().unwrap();
        if w["kind"] == "chat" && !DONE.lock().unwrap().contains(&id) {
            DONE.lock().unwrap().push(id);
            hub.work_done(cid, id, "done", "Ada");
        }
    }
}

fn wait_work(hub: &Hub, gateway: &FakeDaemon, cid: u64, seen: &HashSet<u64>, what: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        answer_chats(hub, gateway, cid);
        if let Some(w) = works(gateway, cid).into_iter().find(|w| {
            w["kind"] != "chat" && w["work"].as_u64().is_some_and(|id| !seen.contains(&id))
        }) {
            return w;
        }
        assert!(
            Instant::now() < deadline,
            "no new work for {cid} ({what})\n{}",
            hub.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A VM: the agent's workspace and the `weft` commands its harness runs, through the agent gateway.
struct Vm<'a> {
    hub: &'a Hub,
    cid: u64,
    home: PathBuf,
    token: PathBuf,
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

/// How a stand-in reviewer answers one step: its scores, findings and verdict.
struct Answer {
    pass: bool,
    scores: Vec<Value>,
    findings: Vec<Value>,
    notes: &'static str,
}

fn pass() -> Answer {
    Answer {
        pass: true,
        scores: vec![],
        findings: vec![],
        notes: "fine",
    }
}

/// What the reviewers of task `tid` were sent and what the hub holds for it: printed when a review stalls.
fn review_state(
    hub: &Hub,
    gateway: &FakeDaemon,
    cids: &[u64],
    tid: &str,
    answered: &HashSet<u64>,
) -> String {
    let mut out = String::new();
    for &cid in cids {
        let sent: Vec<String> = works(gateway, cid)
            .iter()
            .map(|w| {
                let id = w["work"].as_u64().unwrap_or(0);
                let mark = if answered.contains(&id) {
                    "answered"
                } else {
                    "open"
                };
                format!("{id} {} {} ({mark})", text(&w["kind"]), text(&w["thread"]))
            })
            .collect();
        let agent = hub.call("GET", &format!("/v1/agents/{cid}"), None).1;
        out += &format!("agent {cid} sent {sent:?}\n  {agent}\n");
    }
    let task = hub.call("GET", &format!("/v1/tasks/{tid}"), None).1;
    let runs = hub.call("GET", &format!("/v1/tasks/{tid}/reviews"), None).1;
    out += &format!("task {tid}: {}\nreviews: {runs}\n", text(&task["state"]));
    out
}

/// Plays the reviewers of task `tid`: every review step sent to `cids` is answered by `answer(run, node, tree)`,
/// until `done`.
#[allow(clippy::too_many_arguments)]
fn review(
    hub: &Hub,
    gateway: &FakeDaemon,
    cids: &[u64],
    tid: &str,
    token: &Path,
    home: &Path,
    answered: &Arc<Mutex<HashSet<u64>>>,
    answer: &dyn Fn(&str, &str, &Value) -> Answer,
    done: &dyn Fn() -> bool,
) {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(90);
    while !done() {
        if Instant::now() >= deadline {
            let held = answered.lock().unwrap().clone();
            panic!(
                "the review of {tid} did not finish\n{}\n{}",
                review_state(hub, gateway, cids, tid, &held),
                hub.log()
            );
        }
        for &cid in cids {
            answer_chats(hub, gateway, cid);
            for w in works(gateway, cid) {
                let id = w["work"].as_u64().unwrap();
                let thread = text(&w["thread"]);
                let Some(rest) = thread.strip_prefix("review-") else {
                    continue;
                };
                if !answered.lock().unwrap().insert(id) {
                    continue;
                }
                let (run, node) = rest.split_once('-').unwrap();
                let vm = Vm {
                    hub,
                    cid,
                    home: home.join(format!("vm{cid}")),
                    token: token.to_path_buf(),
                };
                let reply = vm.tool_reply(id, "review_tree", json!({}));
                // A step whose work item ended without a verdict is handed on under a new work item (work_ended). On a
                // slow runner the hub can count a delivery as failed while the test still holds the old item: the old
                // item's verdict then reaches the step (an agent's tool calls go to its running step), and the new item
                // finds no step. A reviewer with no step ends its turn, or its next work waits behind the item forever.
                if reply["is_error"] == true && text(&reply["output"]).contains("no review step") {
                    hub.work_done(cid, id, "done", "no review step left");
                    continue;
                }
                assert_eq!(
                    reply["is_error"], false,
                    "review_tree for agent {cid}: {reply}"
                );
                let tree: Value = serde_json::from_str(&text(&reply["output"])).unwrap();
                assert_eq!(tree["step"], node, "{tree}");
                let a = answer(run, node, &tree);
                if !a.scores.is_empty() || !a.findings.is_empty() {
                    vm.tool(
                        id,
                        "review_score",
                        json!({ "scores": a.scores, "findings": a.findings }),
                    );
                }
                vm.tool(
                    id,
                    "review_verdict",
                    json!({ "pass": a.pass, "notes": a.notes }),
                );
                hub.work_done(cid, id, "done", a.notes);
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    println!("review of {tid}: {:.1} s", started.elapsed().as_secs_f64());
}

fn first_symbol(tree: &Value, ends: &str) -> String {
    tree["files"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|f| f["symbols"].as_array().cloned().unwrap_or_default())
        .find(|s| text(&s["qualified"]).ends_with(ends))
        .map(|s| text(&s["id"]))
        .unwrap_or_else(|| panic!("no {ends} in {tree}"))
}

const QUEUE: &str = "pub struct Queue {\n    pub items: Vec<u64>,\n}\n\nimpl Queue {\n    pub fn next(&mut self) -> Option<u64> {\n        self.items.pop()\n    }\n}\n";

fn add_packs(hub: &Hub) {
    // Every store starts with Markdown; the owner adds Rust.
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
fn a_code_task_and_a_research_brief_go_from_claim_to_landed_through_the_review_graph() {
    let (noded, gateway) = daemons();
    let roles: Value = json!({ "roles": AGENTS.iter().map(|(_, r)| (r.to_string(), json!({ "role": r, "title": r, "tools": ["bash"] }))).collect::<serde_json::Map<String, Value>>() });
    let scratch = Scratch::new("weft-review");
    let hub = Hub::start_with_roles(&scratch, Some(gateway.port), Some(noded.port), Some(&roles));
    for (cid, _) in AGENTS {
        let a = hub.wait_for(&format!("/v1/agents/{cid}"), Duration::from_secs(30), |a| {
            a["status"] == "idle"
        });
        assert_eq!(a["status"], "idle", "{a}\n{}", hub.log());
    }
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    add_packs(&hub);
    let token = scratch.0.join("internal.token");
    std::fs::write(&token, hub.token()).unwrap();
    let home = scratch.0.join("vms");

    // ── the code task ──
    let (_, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Queues", "goal": "A work queue", "kind": "code" })),
    );
    let pid = text(&project["id"]);
    for cid in [300, 301, 302, 303] {
        let (status, m) = hub.call(
            "POST",
            &format!("/v1/projects/{pid}/members"),
            Some(json!({ "cid": cid })),
        );
        assert_eq!(status, 200, "{m}");
    }
    // The project's repo exports to a git remote (a bare repository standing in for GitHub).
    let remote = scratch.0.join("remote.git");
    let have_git = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&remote)
        .status()
        .is_ok_and(|s| s.success());
    let (status, task) = hub.call("POST", "/v1/tasks", Some(json!({ "goal": "Give the queue a fair next()", "acceptance": "next() takes the oldest item; tested", "project": pid, "role": "builder" })));
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);
    let mut seen300 = HashSet::new();
    let w1 = wait_work(&hub, &gateway, 300, &seen300, "the task");
    seen300.insert(w1["work"].as_u64().unwrap());
    let lane = w1["lane"].clone();
    let change = text(&lane["change"]);
    let repo = text(&lane["repo"]);
    assert!(
        change.starts_with("w-") && text(&w1["prompt"]).contains(&change),
        "{w1}"
    );
    if have_git {
        let (status, body) = hub.call("POST", "/v1/weft/console", Some(json!({ "kind": "connection.add", "body": { "repo": repo, "id": "origin", "remote": format!("file://{}", remote.display()), "platform": "git", "branch": "main", "author_name": "Irfan Saric", "author_email": "owner@example.com" } })));
        assert_eq!(status, 200, "{body}");
    }

    // The builder's VM: checkout, two turns, intents, done.
    let vm = Vm {
        hub: &hub,
        cid: 300,
        home: home.join("vm300"),
        token: token.clone(),
    };
    let ws = vm.home.join(&repo);
    vm.weft(
        &vm.home,
        &["checkout", &change, "--dir", ws.to_str().unwrap()],
    );
    vm.write(&format!("{repo}/src/queue.rs"), QUEUE);
    vm.weft(
        &ws,
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
    vm.write(&format!("{repo}/src/queue.rs"), &QUEUE.replace("self.items.pop()", "if self.items.is_empty() {\n            return None;\n        }\n        Some(self.items.remove(0))"));
    vm.write(
        &format!("{repo}/README.md"),
        "# Queues\n\nnext() takes the oldest item.\n",
    );
    vm.weft(
        &ws,
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
    let work = w1["work"].as_u64().unwrap();
    vm.tool(work, "weft_describe", json!({ "intents": [
        { "title": "The queue hands out its oldest item first", "kind": "fix", "summary": "next() takes from the front.", "covers": ["src/**"] },
        { "title": "Say how the queue orders work", "kind": "docs", "covers": ["README.md"] }
    ] }));
    let status = vm.tool(work, "weft_status", json!({}));
    assert!(text(&status["output"]).contains("src/queue.rs"), "{status}");
    hub.work_done(300, work, "done", "next() now takes the oldest item");
    let sealed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(20),
        |c| c["rounds"].as_array().is_some_and(|r| r.len() == 1),
    );
    assert_eq!(
        sealed["rounds"].as_array().map(|r| r.len()),
        Some(1),
        "{sealed}\n{}",
        hub.log()
    );

    // Round 1: the qa step finds the fix untested and fails it; everything else passes.
    let answered = Arc::new(Mutex::new(HashSet::new()));
    let reviewers = [301, 302, 303];
    let rework_seen = || hub.call("GET", &format!("/v1/tasks/{tid}"), None).1["state"] == "rework";
    review(
        &hub,
        &gateway,
        &reviewers,
        &tid,
        &token,
        &home,
        &answered,
        &|_, node, tree| match node {
            "quality" => {
                let next = first_symbol(tree, "fn next");
                Answer {
                    pass: false,
                    scores: vec![
                        json!({ "symbol": next, "axis": "tst", "level": -3, "confidence": 0.9, "basis": "measured", "evidence": "no test calls next()" }),
                    ],
                    findings: vec![
                        json!({ "symbol": next, "text": "next() has no test", "severity": "major" }),
                    ],
                    notes: "untested",
                }
            }
            "checks" => Answer {
                pass: true,
                scores: vec![
                    json!({ "symbol": "src/queue.rs", "axis": "tst", "level": 0, "basis": "measured", "evidence": "cargo test: 0 tests" }),
                ],
                findings: vec![],
                notes: "it builds",
            },
            _ => pass(),
        },
        &rework_seen,
    );
    let (_, runs) = hub.call("GET", &format!("/v1/tasks/{tid}/reviews"), None);
    let rv1 = text(&runs["runs"][0]["run"]);
    assert_eq!(runs["runs"][0]["state"], "rework", "{runs}");
    assert_eq!(runs["runs"][0]["recipe"], "code@3");
    // The worker gets it back, with the finding.
    let w2 = wait_work(&hub, &gateway, 300, &seen300, "the rework");
    seen300.insert(w2["work"].as_u64().unwrap());
    assert!(text(&w2["prompt"]).contains("next() has no test"), "{w2}");
    assert_eq!(w2["lane"]["change"], change.as_str());
    vm.write(&format!("{repo}/src/queue.rs"), &format!("{}\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn the_oldest_goes_first() {{\n        let mut q = Queue {{ items: vec![1, 2] }};\n        assert_eq!(q.next(), Some(1));\n    }}\n}}\n", QUEUE.replace("self.items.pop()", "if self.items.is_empty() {\n            return None;\n        }\n        Some(self.items.remove(0))")));
    vm.weft(
        &ws,
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
    vm.tool(w2["work"].as_u64().unwrap(), "weft_describe", json!({ "intents": [
        { "title": "The queue hands out its oldest item first", "kind": "fix", "summary": "next() takes from the front, with a test.", "covers": ["src/**"] },
        { "title": "Say how the queue orders work", "kind": "docs", "covers": ["README.md"] }
    ] }));
    hub.work_done(300, w2["work"].as_u64().unwrap(), "done", "tested");

    // Round 2: the security step sees a risk (a removal from the front of a vector is O(n)): the owner decides.
    let owner_card = || {
        let (_, list) = hub.call("GET", "/v1/approvals", None);
        list.as_array()
            .into_iter()
            .flatten()
            .find(|a| a["kind"] == "review" && a["state"] == "pending")
            .cloned()
    };
    review(
        &hub,
        &gateway,
        &reviewers,
        &tid,
        &token,
        &home,
        &answered,
        &|run, node, tree| {
            assert_ne!(run, rv1, "round 2 is a new run");
            match node {
                "risk" => Answer {
                    pass: true,
                    scores: vec![
                        json!({ "symbol": first_symbol(tree, "fn next"), "axis": "sec", "level": -3, "confidence": 0.6, "basis": "estimated", "evidence": "a huge queue makes next() slow enough to stall the scheduler" }),
                    ],
                    findings: vec![],
                    notes: "a denial of service risk",
                },
                "quality" => {
                    // The finding from round 1 follows next() into round 2.
                    assert!(
                        tree["earlier_findings"]
                            .as_array()
                            .is_some_and(|f| f.iter().any(|x| x["text"] == "next() has no test")),
                        "{tree}"
                    );
                    pass()
                }
                _ => pass(),
            }
        },
        &|| owner_card().is_some(),
    );
    let card = owner_card().unwrap();
    sample("approval-review-card", &card);
    assert!(
        text(&card["detail"]["needs_you"]).contains("security"),
        "{card}"
    );
    assert_eq!(
        card["detail"]["readings"].as_array().map(|r| r.len()),
        Some(4),
        "{card}"
    );
    let (status, decided) = hub.call(
        "POST",
        &format!("/v1/approvals/{}", text(&card["id"])),
        Some(json!({ "approve": true, "reason": "fine for now; queues stay small" })),
    );
    assert_eq!(status, 200, "{decided}");

    // Done, queued, landed, exported.
    let task_done = hub.wait_for(&format!("/v1/tasks/{tid}"), Duration::from_secs(20), |t| {
        t["state"] == "done"
    });
    assert_eq!(task_done["state"], "done", "{task_done}");
    let landed = hub.wait_for(
        &format!("/v1/weft/changes/{change}"),
        Duration::from_secs(30),
        |c| c["state"] == "landed",
    );
    assert_eq!(landed["state"], "landed", "{landed}\n{}", hub.log());
    let kinds: Vec<String> = landed["records"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| text(&r["type"]))
        .collect();
    for k in ["written", "checked", "review", "gate", "ruling", "landed"] {
        assert!(kinds.contains(&k.to_string()), "{k} in {kinds:?}");
    }
    assert!(
        landed["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["type"] == "ruling" && r["via_console"] == true)
    );
    let verified = hub.wait_for(
        &format!("/v1/weft/verify?subject={change}"),
        Duration::from_secs(5),
        |v| v["ok"] == true,
    );
    assert_eq!(verified["ok"], true, "{verified}");
    let signers: HashSet<String> = verified["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| text(&r["actor"]))
        .collect();
    for who in [
        "agent:300",
        "agent:301",
        "agent:302",
        "agent:303",
        "hub",
        "owner",
    ] {
        assert!(signers.contains(who), "{who} signed something: {signers:?}");
    }
    let (_, tree) = hub.call("GET", &format!("/v1/reviews/{}/tree", rv1), None);
    assert!(tree["tree"]["files"].is_array(), "{tree}");
    if have_git {
        let log = hub.wait_for(
            &format!("/v1/weft/repos/{repo}"),
            Duration::from_secs(20),
            |r| r["connections"][0]["exported"] == 1,
        );
        assert_eq!(log["connections"][0]["exported"], 1, "{log}\n{}", hub.log());
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

    // The post-land suite runs on main and fails: a signed revert lands at once, and the author gets a fix task.
    let reverted = || {
        let (_, log) = hub.call(
            "GET",
            &format!("/v1/weft/repos/{repo}/log?q=reverts()%20%26%20landed()"),
            None,
        );
        log["total"].as_u64().unwrap_or(0) == 1
    };
    review(
        &hub,
        &gateway,
        &[303],
        &tid,
        &token,
        &home,
        &answered,
        &|_, node, _| {
            assert_eq!(node, "checks");
            Answer {
                pass: false,
                scores: vec![
                    json!({ "symbol": "src/queue.rs", "axis": "tst", "level": -4, "basis": "measured", "evidence": "cargo test --workspace --release: 1 failed" }),
                ],
                findings: vec![],
                notes: "the release build's tests fail",
            }
        },
        &reverted,
    );
    let (_, runs) = hub.call("GET", &format!("/v1/tasks/{tid}/reviews"), None);
    assert!(
        runs["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["recipe"] == "post-land@1" && r["state"] == "failed"),
        "{runs}"
    );
    let (_, tasks) = hub.call("GET", &format!("/v1/tasks?project={pid}"), None);
    assert!(
        tasks
            .to_string()
            .contains(&format!("Fix what {change} broke")),
        "{tasks}"
    );
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
        main_file.status, 404,
        "the revert takes the file back out of main"
    );

    // ── the research brief ──
    let (_, project) = hub.call("POST", "/v1/projects", Some(json!({ "title": "Edge caching", "goal": "Should nodes cache objects?", "kind": "other" })));
    let rpid = text(&project["id"]);
    for cid in [305, 306, 307] {
        let (status, m) = hub.call(
            "POST",
            &format!("/v1/projects/{rpid}/members"),
            Some(json!({ "cid": cid })),
        );
        assert_eq!(status, 200, "{m}");
    }
    let (_, task) = hub.call("POST", "/v1/tasks", Some(json!({ "goal": "Write a brief on edge caching, with numbers", "project": rpid, "role": "researcher" })));
    let rtid = text(&task["id"]);
    let mut seen305 = HashSet::new();
    let r1 = wait_work(&hub, &gateway, 305, &seen305, "the brief");
    seen305.insert(r1["work"].as_u64().unwrap());
    let rchange = text(&r1["lane"]["change"]);
    let rrepo = text(&r1["lane"]["repo"]);
    let rvm = Vm {
        hub: &hub,
        cid: 305,
        home: home.join("vm305"),
        token: token.clone(),
    };
    let rws = rvm.home.join(&rrepo);
    rvm.weft(
        &rvm.home,
        &["checkout", &rchange, "--dir", rws.to_str().unwrap()],
    );
    rvm.write(&format!("{rrepo}/briefs/edge-caching.md"), "# Edge caching\n\n## Findings\n\n- 93% of reads repeat within a task.\n\n## Recommendation\n\nCache on every node.\n");
    rvm.write(
        &format!("{rrepo}/data/reads.csv"),
        "task,reads,repeats\nt1,120,101\nt2,80,65\n",
    );
    rvm.weft(
        &rws,
        &["snapshot", "--turn", "1", "--model", "claude-opus-5-5"],
    );
    rvm.tool(r1["work"].as_u64().unwrap(), "weft_describe", json!({ "intents": [{ "title": "Measure how often reads repeat", "kind": "research", "covers": ["briefs/**", "data/**"] }] }));
    hub.work_done(305, r1["work"].as_u64().unwrap(), "done", "brief written");
    // The numbers step re-runs them: 166 of 200 is 83%, not 93% — a measured accuracy finding sends it back.
    let rreview = [306, 307];
    let back = || hub.call("GET", &format!("/v1/tasks/{rtid}"), None).1["state"] == "rework";
    review(
        &hub,
        &gateway,
        &rreview,
        &rtid,
        &token,
        &home,
        &answered,
        &|_, node, tree| match node {
            "numbers" => {
                let claim = first_symbol(tree, "item 93% of reads repeat within a task.");
                Answer {
                    pass: false,
                    scores: vec![
                        json!({ "symbol": claim, "axis": "acc", "level": -3, "confidence": 0.95, "basis": "measured", "evidence": "data/reads.csv: 166 of 200 reads repeat (83%)" }),
                    ],
                    findings: vec![
                        json!({ "symbol": claim, "text": "the data says 83%, not 93%", "severity": "major" }),
                    ],
                    notes: "the headline number is wrong",
                }
            }
            _ => pass(),
        },
        &back,
    );
    let r2 = wait_work(&hub, &gateway, 305, &seen305, "the brief's rework");
    assert!(text(&r2["prompt"]).contains("83%"), "{r2}");
    rvm.write(&format!("{rrepo}/briefs/edge-caching.md"), "# Edge caching\n\n## Findings\n\n- 83% of reads repeat within a task (166 of 200, data/reads.csv).\n\n## Recommendation\n\nCache on every node.\n");
    rvm.weft(
        &rws,
        &["snapshot", "--turn", "2", "--model", "claude-opus-5-5"],
    );
    hub.work_done(
        305,
        r2["work"].as_u64().unwrap(),
        "done",
        "fixed the number",
    );
    let rdone = || hub.call("GET", &format!("/v1/tasks/{rtid}"), None).1["state"] == "done";
    review(
        &hub,
        &gateway,
        &rreview,
        &rtid,
        &token,
        &home,
        &answered,
        &|_, _, _| pass(),
        &rdone,
    );
    let rlanded = hub.wait_for(
        &format!("/v1/weft/changes/{rchange}"),
        Duration::from_secs(30),
        |c| c["state"] == "landed",
    );
    assert_eq!(rlanded["state"], "landed", "{rlanded}\n{}", hub.log());
    let (_, rruns) = hub.call("GET", &format!("/v1/tasks/{rtid}/reviews"), None);
    assert_eq!(
        rruns["runs"].as_array().map(|r| r.len()),
        Some(2),
        "{rruns}"
    );
    assert!(
        rruns["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["recipe"] == "research@1"),
        "{rruns}"
    );
    // The brief's history: who wrote the corrected line.
    let (_, blame) = hub.call(
        "GET",
        &format!("/v1/weft/repos/{rrepo}/blame?path=briefs/edge-caching.md"),
        None,
    );
    let line = blame["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| text(&l["text"]).contains("83%"))
        .cloned()
        .unwrap();
    assert_eq!(line["by"]["by"]["cid"], 305, "{line}");
    let deep = hub.wait_for("/v1/weft/verify?deep=1", Duration::from_secs(5), |v| {
        v["ok"] == true
    });
    assert_eq!(deep["ok"], true, "{deep}");

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
            (
                "blame",
                format!("/v1/weft/repos/{rrepo}/blame?path=briefs/edge-caching.md"),
            ),
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
            ("task-reviews", format!("/v1/tasks/{tid}/reviews")),
            ("review-run", format!("/v1/reviews/{rv1}")),
            ("review-tree", format!("/v1/reviews/{rv1}/tree")),
            ("research-reviews", format!("/v1/tasks/{rtid}/reviews")),
            ("recipes", "/v1/recipes".to_string()),
            ("recipe-code", "/v1/recipes/code".to_string()),
            ("approvals", "/v1/approvals".to_string()),
        ] {
            let (_, v) = hub.call("GET", &path, None);
            sample(name, &v);
        }
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
