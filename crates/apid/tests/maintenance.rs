//! A finished project looked after, end to end on a scratch hub with Weft. The owner brings in a project that exists (a git
//! repository on this machine) with its environments; it opens delivered and in maintenance. The work loop starts the
//! project's planner, a long-lived session, as it does for a project being built, and the planner sweeps first. The
//! watchers' tools map it,
//! record what they read and propose work; the orchestrator cannot post a task there. The owner picks one proposal (a task
//! of the project), declines another (the next sweep reads why), and makes an ingest address. The picked task's builder
//! seat starts a session, which deploys to staging at once and to production only after the owner's approval, each
//! through the environment's deploy hook. A chat carries the owner's file, which the agent may then read; files the owner
//! drops land in the repository; and back in build the maintenance tools are no longer offered.
//!
//! noded and the agent gateway are stood in for, and so is a deploy hook; the agents' calls are made as their VMs make
//! them.

mod common;

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const ORCHESTRATOR: u64 = 109;
/// The project's planner: noded starts it on this VM when the work loop starts the planner.
const PLANNER: u64 = 300;
/// A builder the fleet runs, with no session of its own: the owner's chat and file go to it.
const BUILDER: u64 = 301;
/// The VM the picked task's builder seat starts.
const SEAT_BUILDER: u64 = 302;
/// The VM a reviewer seat starts (none does in this test: the builder's work is never done here).
const REVIEWER: u64 = 303;
/// The first CID for any other VM noded starts.
const SPARE: u64 = 310;

/// The VMs noded runs, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// Agent `cid`'s call of host tool `name` in work item `work`: the hub's decision and output.
fn tool(hub: &Hub, cid: u64, work: u64, name: &str, input: Value) -> Value {
    let message = json!({ "type": "tool_request", "work": work, "call": "toolu_1", "name": name, "input": input });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// The work items pushed to agent `cid`, as (work, thread, prompt).
fn pushed(gateway: &FakeDaemon, cid: u64) -> Vec<(u64, String, String)> {
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, _)| {
            method == "POST" && path.ends_with(&format!("/agents/{cid}/send"))
        })
        .filter_map(|(_, _, body)| {
            let body: Value = serde_json::from_str(&body).ok()?;
            let work = &body["message"];
            (work["type"] == "work").then(|| {
                (
                    work["work"].as_u64().unwrap_or(0),
                    text(&work["thread"]),
                    text(&work["prompt"]),
                )
            })
        })
        .collect()
}

/// Waits for a work item on `thread` pushed to `cid` after the first `seen` ones; its (work, prompt).
fn next_work(gateway: &FakeDaemon, cid: u64, thread: &str, seen: usize) -> (u64, String) {
    // Up to 10 s, times the test slack.
    for _ in 0..(100.0 * common::slack()) as usize {
        let mine: Vec<_> = pushed(gateway, cid)
            .into_iter()
            .filter(|(_, t, _)| t == thread)
            .collect();
        if mine.len() > seen {
            let (work, _, prompt) = mine[seen].clone();
            return (work, prompt);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("no work on {thread} for {cid}: {:?}", pushed(gateway, cid));
}

/// noded and the agent gateway, sharing the VMs noded runs: a run's VM gets its role's CID (the planner's, the builder
/// seat's, a reviewer's), and any other the next CID from [`SPARE`].
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
            let cid = match role.as_str() {
                "planner" => PLANNER,
                "builder" => SEAT_BUILDER,
                "reviewer" => REVIEWER,
                _ => SPARE + vms.len() as u64,
            };
            vms.push((cid, role));
            let id = vms.len() - 1;
            let vm = json!({ "id": id, "guest_cid": cid, "state": "running" });
            (200, json!({ "id": "r2", "vms": [vm] }).to_string())
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

fn tool_names(profile: &Value) -> Vec<String> {
    profile["tools"]
        .as_array()
        .map(|t| t.iter().map(|s| text(&s["name"])).collect())
        .unwrap_or_default()
}

/// A git repository with two commits and a tag, made with git itself.
fn git_repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Ada Owner")
            .env("GIT_AUTHOR_EMAIL", "ada@example.test")
            .env("GIT_COMMITTER_NAME", "Ada Owner")
            .env("GIT_COMMITTER_EMAIL", "ada@example.test")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(
        dir.join("README.md"),
        "# Shop\n\nNotebooks for field work.\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/checkout.js"),
        "export function pay() { return true; }\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "The shop"]);
    git(&["tag", "-a", "v1.0.0", "-m", "Launch"]);
    std::fs::write(
        dir.join("src/search.js"),
        "export function find(q) { return [q]; }\n",
    )
    .unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "Search"]);
}

#[test]
fn a_finished_project_is_watched_and_the_owner_picks_its_work() {
    let vms: Vms = Arc::new(Mutex::new(vec![
        (ORCHESTRATOR, "orchestrator".to_string()),
        (BUILDER, "builder".to_string()),
    ]));
    let (noded, gateway) = daemons(&vms);
    // The deploy hooks of the shop's environments (a CI's deploy URL, say).
    let hook = FakeDaemon::serve(|_, _, _| (200, r#"{"queued":true}"#.into()));
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "run_tests", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "planner": role("planner", "Planner"),
        "builder": role("builder", "Builder"),
    } });
    let scratch = Scratch::new("maintenance");
    let hub = Hub::start_with_roles(&scratch, Some(gateway.port), Some(noded.port), Some(&roles));
    for (cid, role) in [(ORCHESTRATOR, "orchestrator"), (BUILDER, "builder")] {
        let agent = hub.wait_for(&format!("/v1/agents/{cid}"), Duration::from_secs(20), |a| {
            a["role"] == role && a["status"] == "idle"
        });
        assert_eq!(agent["role"], role, "{agent}\n{}", hub.log());
    }
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());

    // The owner brings the shop in from its folder, with production and staging.
    let source = scratch.0.join("shop");
    git_repo(&source);
    let hook_url = format!("http://127.0.0.1:{}/deploy", hook.port);
    let (status, adopted) = hub.call(
        "POST",
        "/v1/projects/adopt",
        Some(json!({
            "title": "Field shop",
            "goal": "The shop that sells field notebooks",
            "source": { "path": source.to_string_lossy(), "branch": "main" },
            "environments": [
                { "name": "production", "kind": "production", "url": "https://shop.example.test", "hook": hook_url },
                { "name": "staging", "kind": "staging", "url": "https://staging.shop.example.test", "hook": hook_url },
            ],
        })),
    );
    assert_eq!(status, 201, "{adopted}\n{}", hub.log());
    assert_eq!(adopted["problems"], json!([]), "{adopted}");
    let pid = text(&adopted["project"]["id"]);
    assert_eq!(adopted["project"]["stage"], "delivered");
    assert_eq!(adopted["project"]["mode"], "maintenance");
    assert_eq!(adopted["import"]["commits"], 2, "{adopted}");
    assert_eq!(
        adopted["import"]["releases"],
        json!(["v1.0.0"]),
        "{adopted}"
    );
    let envs = adopted["environments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(envs.len(), 2, "{adopted}");
    assert_eq!(
        (
            envs[0]["track"].clone(),
            envs[0]["protected"].clone(),
            envs[0]["hook"].clone()
        ),
        (json!("releases"), json!(true), json!(true))
    );
    assert_eq!(
        (envs[1]["track"].clone(), envs[1]["protected"].clone()),
        (json!("main"), json!(false))
    );

    // The owner adds what support heard.
    let (status, signal) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/signals"),
        Some(json!({ "kind": "report", "title": "Checkout fails on Safari", "text": "Three customers: the pay button does nothing on Safari 18.", "from": "support", "environment": "production" })),
    );
    assert_eq!(status, 201, "{signal}");
    let report = text(&signal["id"]);

    // The planner is the project's long-lived session: in maintenance too it starts on the next tick (it is the one who
    // sweeps).
    let planners = hub.wait_for(
        &format!("/v1/sessions?project={pid}"),
        Duration::from_secs(20),
        |s| {
            s.as_array().is_some_and(|s| {
                s.iter()
                    .any(|s| s["role"] == "planner" && s["cid"] == PLANNER)
            })
        },
    );
    assert!(
        planners.as_array().is_some_and(|s| s
            .iter()
            .any(|s| s["role"] == "planner" && s["cid"] == PLANNER && s["state"] != "failed")),
        "a live planner session on its VM: {planners}\n{}",
        hub.log()
    );

    // Its watchers get the maintenance tools; the planner sweeps first, and the sweep asks for the map.
    let (_, hello) = hub.guest(PLANNER, "hello", json!({ "role": "planner", "vm": 1 }));
    let names = tool_names(&hello["profile"]);
    for name in [
        "plaza_maintenance",
        "plaza_maintenance_map",
        "plaza_maintenance_propose",
        "plaza_maintenance_signal",
        "plaza_environments",
    ] {
        assert!(
            names.contains(&name.to_string()),
            "{name} offered: {names:?}"
        );
    }
    assert!(
        !names.contains(&"plaza_deploy".to_string()),
        "a planner does not deploy"
    );
    // The first sweep is due as the project enters maintenance, so the hub may have started it with the planner; else
    // the owner starts it now.
    let (status, started) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/sweep"),
        None,
    );
    let under_way =
        text(&started["error"]).starts_with(&format!("a sweep is under way (agent {PLANNER}"));
    assert!(
        status == 202 || (status == 409 && under_way),
        "{status} {started}\n{}",
        hub.log()
    );
    let thread = format!("maintenance-{pid}");
    let (sweep, prompt) = next_work(&gateway, PLANNER, &thread, 0);
    assert!(prompt.contains("has no map yet"), "{prompt}");
    assert!(
        prompt.contains("Checkout fails on Safari"),
        "the new signal is in the sweep: {prompt}"
    );
    assert!(
        prompt.contains("production (production) at https://shop.example.test"),
        "{prompt}"
    );

    // The watcher maps the shop, records what it read in the analytics, and proposes.
    let mapped = tool(
        &hub,
        PLANNER,
        sweep,
        "plaza_maintenance_map",
        json!({
            "summary": "A small shop: a catalogue, search and a checkout.",
            "areas": [
                { "name": "Checkout", "summary": "Payment", "paths": ["src/checkout.js"], "health": "poor", "links": ["search"] },
                { "name": "Search", "paths": ["src/search.js"], "health": "good" },
            ],
        }),
    );
    assert!(
        text(&mapped["output"]).contains("checkout (poor)"),
        "{mapped}"
    );
    let digest = tool(
        &hub,
        PLANNER,
        sweep,
        "plaza_maintenance_signal",
        json!({ "kind": "analytics", "text": "Checkout conversion fell from 3.1% to 1.9% this week, all of it on Safari." }),
    );
    assert!(
        text(&digest["output"]).starts_with("recorded s"),
        "{digest}"
    );
    let propose = |title: &str, kind: &str, score: u64| {
        let mut input = json!({
            "kind": kind,
            "title": title,
            "summary": "Safari users cannot pay: the click handler uses an API Safari lacks.",
            "area": "checkout",
            "impact": { "changes": "The pay button works on Safari; nothing changes elsewhere.", "users": "About 40% of buyers", "risk": "low" },
            "value": { "score": score, "why": "Lost sales every day" },
            "effort": { "size": "s" },
            "evidence": [{ "signal": report, "note": "three customers" }],
            "acceptance": "A Safari 18 checkout completes in the end-to-end test.",
        });
        if kind == "instrument" {
            input["instrument"] = json!({ "kind": "mcp", "name": "Sentry", "data": "Every front-end error with its browser" });
        }
        tool(&hub, PLANNER, sweep, "plaza_maintenance_propose", input)
    };
    let fix = propose("Fix checkout on Safari", "fix", 5);
    assert!(
        text(&fix["output"]).contains("waits for the owner"),
        "{fix}"
    );
    let twice = propose("Fix checkout on Safari", "fix", 5);
    assert!(
        text(&twice["output"]).contains("already says"),
        "the same proposal is not added twice: {twice}"
    );
    propose("See front-end errors", "instrument", 4);
    let bad = tool(
        &hub,
        PLANNER,
        sweep,
        "plaza_maintenance_propose",
        json!({ "kind": "fix", "title": "Ghost", "summary": "s", "impact": { "changes": "c", "risk": "low" }, "value": { "score": 3, "why": "w" }, "effort": { "size": "s" }, "evidence": [{ "signal": "s999" }] }),
    );
    assert!(
        text(&bad["output"]).contains("does not hold"),
        "evidence must be a real signal: {bad}"
    );

    // The orchestrator cannot post a task to a project in maintenance: the owner picks its work.
    let posted = tool(
        &hub,
        ORCHESTRATOR,
        1,
        "plaza_tasks_post",
        json!({ "project": pid, "tasks": [{ "key": "1", "goal": "Rewrite the shop" }] }),
    );
    assert!(
        text(&posted["output"]).contains("is in maintenance"),
        "{posted}"
    );

    // The sweep ends with its summary.
    hub.work_done(
        PLANNER,
        sweep,
        "done",
        "Mapped the shop; proposed the Safari fix and Sentry.",
    );
    let view = hub.wait_for(
        &format!("/v1/projects/{pid}/maintenance"),
        Duration::from_secs(10),
        |m| m["sweep"]["runs"] == 1,
    );
    assert_eq!(
        view["sweep"]["last_summary"], "Mapped the shop; proposed the Safari fix and Sentry.",
        "{view}"
    );
    assert_eq!(
        view["map"]["areas"].as_array().map(Vec::len),
        Some(2),
        "{view}"
    );
    assert_eq!(view["signals_total"], 2);
    assert_eq!(
        view["unseen"], 1,
        "the watcher's own digest came after the sweep's start"
    );
    let proposals = view["proposals"].as_array().cloned().unwrap_or_default();
    assert_eq!(proposals.len(), 2, "{view}");
    let id_of = |title: &str| {
        text(
            &proposals
                .iter()
                .find(|p| p["title"] == title)
                .expect("the proposal")["id"],
        )
    };
    let (fix_id, sentry_id) = (
        id_of("Fix checkout on Safari"),
        id_of("See front-end errors"),
    );

    // The owner declines Sentry, and the next sweep reads why.
    let (status, declined) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/proposals/{sentry_id}/decline"),
        Some(json!({ "reason": "We use our own error log" })),
    );
    assert_eq!(status, 200, "{declined}");
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/sweep"),
        None,
    );
    assert_eq!(status, 202);
    let (second, prompt) = next_work(&gateway, PLANNER, &thread, 1);
    assert!(
        prompt.contains("We use our own error log"),
        "the decline's reason: {prompt}"
    );
    assert!(prompt.contains("The map (2 areas)"), "{prompt}");
    hub.work_done(PLANNER, second, "done", "Nothing new.");

    // The owner picks the fix: it becomes a task of the project, for a builder, with its acceptance.
    let (status, picked) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/proposals/{fix_id}/approve"),
        Some(json!({ "note": "Ship it this week" })),
    );
    assert_eq!(status, 200, "{picked}\n{}", hub.log());
    assert_eq!(picked["state"], "approved");
    let task = text(&picked["task"]);
    let (_, t) = hub.call("GET", &format!("/v1/tasks/{task}"), None);
    assert_eq!(t["project"], pid.as_str(), "{t}");
    assert_eq!(t["role"], "builder", "{t}");
    assert!(
        text(&t["goal"]).contains("Fix checkout on Safari")
            && text(&t["goal"]).contains("Ship it this week"),
        "{t}"
    );
    assert_eq!(
        t["acceptance"],
        "A Safari 18 checkout completes in the end-to-end test."
    );
    let (status, again) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/proposals/{fix_id}/approve"),
        None,
    );
    assert_eq!(status, 409, "a proposal is picked once: {again}");

    // The pick's builder seat starts a session on a VM of its own: that session is the builder who deploys.
    let seated = hub.wait_for(
        &format!("/v1/sessions?task={task}"),
        Duration::from_secs(20),
        |s| {
            s.as_array().is_some_and(|s| {
                s.iter()
                    .any(|s| s["seat"] == "builder" && s["cid"] == SEAT_BUILDER)
            })
        },
    );
    assert!(
        seated
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["seat"] == "builder")),
        "a builder session for the pick: {seated}\n{}",
        hub.log()
    );
    let (status, hello) = hub.guest(SEAT_BUILDER, "hello", json!({ "role": "builder", "vm": 2 }));
    assert_eq!(status, 200, "{hello}");
    assert!(tool_names(&hello["profile"]).contains(&"plaza_deploy".to_string()));

    // An ingest address takes a report without the owner's token; a wrong address is unknown.
    let (status, ingest) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/ingest"),
        None,
    );
    assert_eq!(status, 201, "{ingest}");
    let path = text(&ingest["path"]);
    let (status, took) = hub.call(
        "POST",
        &path,
        Some(json!({ "kind": "feedback", "text": "Love the notebooks" })),
    );
    assert_eq!(status, 202, "{took}");
    let (status, _) = hub.call(
        "POST",
        "/v1/ingest/apzi_wrong",
        Some(json!({ "text": "x" })),
    );
    assert_eq!(status, 404);

    // The builder deploys to staging at once, through its hook; production waits for the owner.
    let staged = tool(
        &hub,
        SEAT_BUILDER,
        7,
        "plaza_deploy",
        json!({ "environment": "staging", "version": "w-3", "why": "the Safari fix" }),
    );
    assert!(
        text(&staged["output"]).contains("deployed w-3 to staging"),
        "{staged}"
    );
    let prod = tool(
        &hub,
        SEAT_BUILDER,
        7,
        "plaza_deploy",
        json!({ "environment": "production", "version": "v1.0.1", "why": "release" }),
    );
    assert!(
        text(&prod["output"]).contains("waits for the owner's approval"),
        "{prod}"
    );
    let (_, approvals) = hub.call("GET", "/v1/approvals", None);
    let pending = approvals
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|x| x["kind"] == "deploy" && x["state"] == "pending")
                .cloned()
        })
        .expect("a deploy approval waits");
    let (status, decided) = hub.call(
        "POST",
        &format!("/v1/approvals/{}", text(&pending["id"])),
        Some(json!({ "approve": true })),
    );
    assert_eq!(status, 200, "{decided}\n{}", hub.log());
    let envs = hub.wait_for(
        &format!("/v1/projects/{pid}/environments"),
        Duration::from_secs(10),
        |e| {
            e.as_array().is_some_and(|l| {
                l.iter().all(|x| {
                    x["deploys"]
                        .as_array()
                        .and_then(|d| d.last())
                        .is_some_and(|d| d["status"] == "sent")
                })
            })
        },
    );
    let version = |name: &str| {
        envs.as_array()
            .and_then(|l| l.iter().find(|e| e["name"] == name))
            .map(|e| e["version"].clone())
    };
    assert_eq!(version("staging"), Some(json!("w-3")), "{envs}");
    assert_eq!(version("production"), Some(json!("v1.0.1")), "{envs}");
    let calls: Vec<Value> = hook
        .seen()
        .into_iter()
        .filter_map(|(_, _, body)| serde_json::from_str(&body).ok())
        .collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(
        calls.iter().any(|c| c["environment"] == "production"
            && c["version"] == "v1.0.1"
            && c["project"] == pid.as_str()),
        "{calls:?}"
    );

    // A chat carries the owner's file: the agent's prompt says where it is, and the agent may read it.
    let (status, file) = hub.send_bytes(
        "PUT",
        "/v1/files?name=safari.txt",
        b"Safari 18.0, macOS 15: the button does nothing",
        "text/plain",
    );
    assert_eq!(status, 201, "{file}");
    let fid = text(&file["id"]);
    let (_, hello) = hub.guest(BUILDER, "hello", json!({ "role": "builder", "vm": 1 }));
    assert!(hello["profile"].is_object(), "{hello}");
    let (status, chat) = hub.call(
        "POST",
        &format!("/v1/agents/{BUILDER}/chat"),
        Some(json!({ "text": "Here is what support saw", "attachments": [fid] })),
    );
    assert_eq!(status, 200, "{chat}\n{}", hub.log());
    let (_, prompt) = next_work(&gateway, BUILDER, &format!("op-{BUILDER}"), 0);
    assert!(
        prompt.contains("Attached files") && prompt.contains(&fid) && prompt.contains("safari.txt"),
        "{prompt}"
    );
    let read = tool(&hub, BUILDER, 9, "plaza_get_file", json!({ "id": fid }));
    assert!(
        text(&read["output"]).contains("the button does nothing"),
        "{read}"
    );
    let other = tool(&hub, PLANNER, 9, "plaza_get_file", json!({ "id": fid }));
    assert!(
        text(&other["output"]).contains("cannot read"),
        "only the agent it went to: {other}"
    );

    // Files the owner drops land in the repository as one change.
    let (status, up) = hub.send_bytes(
        "PUT",
        &format!("/v1/projects/{pid}/uploads"),
        b"# Notes\n\nSafari first.\n",
        "application/octet-stream",
    );
    assert_eq!(status, 201, "{up}");
    let (status, added) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/files"),
        Some(json!({ "folder": "docs", "files": [{ "path": "notes.md", "hash": up["hash"] }] })),
    );
    assert_eq!(status, 200, "{added}\n{}", hub.log());
    let landed = text(&added["landed"]["landed"]);
    let (_, files) = hub.call("GET", &format!("/v1/weft/revisions/{landed}/files"), None);
    let paths: Vec<String> = files["files"]
        .as_array()
        .map(|f| f.iter().map(|x| text(&x["path"])).collect())
        .unwrap_or_default();
    assert!(
        paths.contains(&"docs/notes.md".to_string())
            && paths.contains(&"src/checkout.js".to_string()),
        "{paths:?}"
    );

    // Back in build the delivered project opens at implementation, and the maintenance tools are gone.
    let (status, built) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/mode"),
        Some(json!({ "mode": "build" })),
    );
    assert_eq!(status, 200, "{built}");
    assert_eq!(
        (built["mode"].clone(), built["stage"].clone()),
        (json!("build"), json!("implementation"))
    );
    let (_, hello) = hub.guest(PLANNER, "hello", json!({ "role": "planner", "vm": 1 }));
    assert!(
        !tool_names(&hello["profile"])
            .iter()
            .any(|n| n.starts_with("plaza_maintenance")),
        "{:?}",
        tool_names(&hello["profile"])
    );
}

/// A project in maintenance that no one can watch (the plaza has no planner role, so no planner starts, and no agent is
/// on the project) refuses a sweep and says why. In the test above the planner starts on a tick after adoption, so it
/// cannot see this.
#[test]
fn a_project_no_one_can_watch_refuses_a_sweep_and_says_why() {
    let vms: Vms = Arc::new(Mutex::new(vec![
        (ORCHESTRATOR, "orchestrator".to_string()),
        (BUILDER, "builder".to_string()),
    ]));
    let (noded, gateway) = daemons(&vms);
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["bash", "read_file"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "builder": role("builder", "Builder"),
    } });
    let scratch = Scratch::new("maintenance-unwatched");
    let hub = Hub::start_with_roles(&scratch, Some(gateway.port), Some(noded.port), Some(&roles));
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    let source = scratch.0.join("shop");
    git_repo(&source);
    let (status, adopted) = hub.call(
        "POST",
        "/v1/projects/adopt",
        Some(json!({
            "title": "Field shop",
            "goal": "The shop that sells field notebooks",
            "source": { "path": source.to_string_lossy(), "branch": "main" },
        })),
    );
    assert_eq!(status, 201, "{adopted}\n{}", hub.log());
    let pid = text(&adopted["project"]["id"]);
    assert_eq!(adopted["project"]["mode"], "maintenance");

    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/maintenance/sweep"),
        None,
    );
    assert_eq!(status, 409, "{refused}\n{}", hub.log());
    assert!(
        text(&refused["error"]).contains("no agents to watch it"),
        "{refused}"
    );
}
