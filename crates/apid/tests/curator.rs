//! The project's curator on a scratch hub (PLAN 9.4, Engine 2.3): a finished task's journal and the proposals its builder
//! made become entries of the project's memory, landed on main by the curator's one answer (`memory_land`). The planner
//! records a decision the owner made; the owner edits it (which supersedes it) and deletes an entry, each as a change to
//! main; the owner keeps the memory card the decision opened.
//!
//! The sessions' VMs are noded's and the gateway's stand-ins, as in `project_memory`; the curator is scripted through the
//! same tool requests the harness would make. Weft runs, so the landings are real changes on the project's repo.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(20);

/// The roles the project needs: a builder and a reviewer for its task, a planner and a curator.
fn roles() -> Value {
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["bash", "read_file"] });
    json!({ "roles": {
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
        "planner": role("planner", "Planner"),
        "curator": role("curator", "Curator"),
    } })
}

/// noded and the agent gateway: each VM noded starts gets the next CID from `first`, and the gateway reports them.
fn daemons(first: u64) -> (FakeDaemon, FakeDaemon) {
    let vms: Arc<Mutex<Vec<(u64, String)>>> = Arc::default();
    let started = Arc::clone(&vms);
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
            let cid = first + vms.len() as u64;
            vms.push((cid, role));
            let vm = json!({ "id": vms.len() - 1, "guest_cid": cid, "state": "running" });
            (200, json!({ "id": "r1", "vms": [vm] }).to_string())
        } else {
            (200, "{}".into())
        }
    });
    let reported = Arc::clone(&vms);
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

/// The work items pushed to `cid`, as (work, prompt).
fn pushed(gateway: &FakeDaemon, cid: u64) -> Vec<(u64, String)> {
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, _)| {
            method == "POST" && path.ends_with(&format!("/agents/{cid}/send"))
        })
        .filter_map(|(_, _, body)| {
            let body: Value = serde_json::from_str(&body).ok()?;
            let work = &body["message"];
            (work["type"] == "work")
                .then(|| (work["work"].as_u64().unwrap_or(0), text(&work["prompt"])))
        })
        .collect()
}

/// Waits for the first work item pushed to `cid`.
fn first_work(gateway: &FakeDaemon, cid: u64) -> (u64, String) {
    let deadline = std::time::Instant::now() + WAIT.mul_f64(common::slack());
    loop {
        if let Some(found) = pushed(gateway, cid).into_iter().next() {
            return found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no work reached {cid}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `cid`'s tool request in `work`: the hub's output for it.
fn tool(hub: &Hub, cid: u64, work: u64, name: &str, input: Value) -> String {
    let message = json!({ "type": "tool_request", "work": work, "call": "toolu_1", "name": name, "input": input });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    text(&body["reply"]["output"])
}

/// The CID of the first session of `role` on project `pid` that has its VM.
fn session_of(hub: &Hub, pid: &str, role: &str) -> u64 {
    let found = hub.wait_for(&format!("/v1/sessions?project={pid}"), WAIT, |s| {
        s.as_array()
            .is_some_and(|s| s.iter().any(|s| s["role"] == role && s["cid"].is_u64()))
    });
    found
        .as_array()
        .and_then(|s| s.iter().find(|s| s["role"] == role).cloned())
        .and_then(|s| s["cid"].as_u64())
        .unwrap_or_else(|| panic!("a {role} session on {pid}: {found}\n{}", hub.log()))
}

/// The project's memory view, once `want` holds for it (or when the wait runs out: the caller asserts on it).
fn memory_when(hub: &Hub, pid: &str, want: impl Fn(&Value) -> bool) -> Value {
    hub.wait_for(&format!("/v1/projects/{pid}/memory"), WAIT, want)
}

/// The entry `id` in the memory view, when it is there.
fn entry_in(memory: &Value, id: &str) -> Option<Value> {
    memory["entries"]
        .as_array()?
        .iter()
        .find(|e| e["id"] == id)
        .cloned()
}

#[test]
fn a_finished_tasks_journal_lands_through_the_curator_and_the_owner_edits_the_memory_as_changes() {
    let (noded, gateway) = daemons(400);
    let scratch = Scratch::new("curator");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Memory", "goal": "Keep the notes' memory" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let planner = session_of(&hub, &pid, "planner");

    // A task: its builder writes a journal and proposes a pitfall, and ends with its handover.
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Draft the notes", "project": pid, "role": "builder" })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);
    let builder = session_of(&hub, &pid, "builder");
    let (status, _) = hub.guest(builder, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200);
    let (work, _) = first_work(&gateway, builder);
    let noted = tool(
        &hub,
        builder,
        work,
        "journal_note",
        json!({ "kind": "decision", "text": "keep the notes in one file", "reason": "one place to read" }),
    );
    assert!(noted.starts_with("Noted: decision n1"), "{noted}");
    tool(
        &hub,
        builder,
        work,
        "journal_note",
        json!({ "kind": "dead_end", "text": "splitting by release", "why": "the links broke" }),
    );
    let proposed = tool(
        &hub,
        builder,
        work,
        "memory_propose",
        json!({
            "type": "pitfall",
            "text": "The changelog links break when a release is split.",
            "why": "seen twice while drafting",
            "anchors": ["sym:apid::journal::run"]
        }),
    );
    assert!(proposed.contains("Proposal mp1 is stored"), "{proposed}");
    let written = tool(
        &hub,
        builder,
        work,
        "handover_write",
        json!({ "stands": "The notes draft is in.", "next": "Review it." }),
    );
    assert!(written.contains("Handover recorded"), "{written}");
    hub.work_done(builder, work, "done", "Drafted the notes.");

    // The reviewer passes it: the change joins the landing queue and lands, and the task is done.
    let reviewer = session_of(&hub, &pid, "reviewer");
    let (status, _) = hub.guest(reviewer, "hello", json!({ "role": "reviewer", "vm": 1 }));
    assert_eq!(status, 200);
    let (review, _) = first_work(&gateway, reviewer);
    let verdict = tool(
        &hub,
        reviewer,
        review,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "Reads well." }),
    );
    assert!(verdict.contains("passes"), "{verdict}");

    // The task's end wakes the project's curator: its prompt has the journal and the proposal.
    let curator = session_of(&hub, &pid, "curator");
    let (status, _) = hub.guest(curator, "hello", json!({ "role": "curator", "vm": 2 }));
    assert_eq!(status, 200);
    let (answer_work, prompt) = first_work(&gateway, curator);
    assert!(prompt.contains("keep the notes in one file"), "{prompt}");
    assert!(prompt.contains("splitting by release"), "{prompt}");
    assert!(
        prompt.contains("mp1 pitfall (anchors: sym:apid::journal::run): The changelog links break"),
        "{prompt}"
    );

    // The hub lands memory through Weft: wait until it runs (it can start late on a busy machine).
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());

    // Its one answer: a decision from the journal, and the proposal accepted as a pitfall.
    let landed = tool(
        &hub,
        curator,
        answer_work,
        "memory_land",
        json!({
            "entries": [
                { "type": "decision", "text": "Keep the notes in one file.\n\nReason: one place to read." },
                { "type": "pitfall", "text": "The changelog links break when a release is split.", "anchors": ["sym:apid::journal::run"], "proposal": "mp1" }
            ],
            "rejected": [],
            "contradictions": []
        }),
    );
    assert!(
        landed.starts_with("Answered: 2 entries land as "),
        "{landed}"
    );
    let memory = memory_when(&hub, &pid, |m| {
        m["entries"].as_array().is_some_and(|e| e.len() == 2)
    });
    assert!(
        memory["entries"].as_array().is_some_and(|e| e.len() == 2),
        "{memory}\n{}",
        hub.log()
    );
    assert_eq!(
        entry_in(&memory, "D-0001").unwrap()["type"],
        "decision",
        "{memory}"
    );
    assert_eq!(
        entry_in(&memory, "P-0001").unwrap()["status"],
        "active",
        "{memory}"
    );
    assert!(
        memory["index"].as_str().is_some_and(|i| i
            .contains("- D-0001 decision: Keep the notes in one file.")
            && i.contains("- P-0001 pitfall: The changelog links break")),
        "{memory}"
    );
    hub.work_done(curator, answer_work, "done", "Landed two entries.");

    // The planner records a decision the owner made in the project's chat: it lands, and a memory card asks the owner.
    let recorded = tool(
        &hub,
        planner,
        0,
        "memory_record_decision",
        json!({
            "text": "Ship the notes on the 30th.",
            "reason": "the owner asked for that date",
            "owner": true
        }),
    );
    assert!(
        recorded.starts_with("Recorded decision D-0002"),
        "{recorded}"
    );
    let memory = memory_when(&hub, &pid, |m| entry_in(m, "D-0002").is_some());
    assert_eq!(
        entry_in(&memory, "D-0002").unwrap()["status"],
        "active",
        "{memory}"
    );
    let (_, cards) = hub.call("GET", "/v1/cards?open=true", None);
    let card = cards
        .as_array()
        .and_then(|c| c.iter().find(|c| c["kind"] == "memory").cloned())
        .unwrap_or_else(|| panic!("a memory card: {cards}"));
    assert_eq!(card["subject"], format!("memory:{pid}:D-0002"), "{card}");

    // The owner keeps it: the card is answered, and nothing else changes.
    let (status, answered) = hub.call(
        "POST",
        &format!("/v1/cards/{}", text(&card["id"])),
        Some(json!({ "choice": "keep" })),
    );
    assert_eq!(status, 200, "{answered}");

    // The owner edits the decision: the new text supersedes it, and the old one is kept as superseded.
    let (status, edit) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/memory/D-0002"),
        Some(json!({ "text": "Ship the notes on the 31st." })),
    );
    assert_eq!(status, 202, "{edit}");
    let memory = memory_when(&hub, &pid, |m| entry_in(m, "D-0003").is_some());
    assert_eq!(
        entry_in(&memory, "D-0003").unwrap()["supersedes"],
        json!(["D-0002"]),
        "{memory}"
    );
    assert_eq!(
        entry_in(&memory, "D-0002").unwrap()["status"],
        "superseded",
        "{memory}"
    );
    assert!(
        !memory["index"].as_str().unwrap_or("").contains("D-0002"),
        "{memory}"
    );

    // The owner deletes the pitfall: its file goes, and the index with it.
    let (status, deleted) = hub.call("DELETE", &format!("/v1/projects/{pid}/memory/P-0001"), None);
    assert_eq!(status, 202, "{deleted}");
    let memory = memory_when(&hub, &pid, |m| entry_in(m, "P-0001").is_none());
    assert!(
        !memory["index"].as_str().unwrap_or("").contains("P-0001"),
        "{memory}"
    );
    assert_eq!(hub.call("GET", "/v1/projects/nope/memory", None).0, 404);
}
