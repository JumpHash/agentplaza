//! The work engine end to end on a scratch hub (Engine 0.3–0.5, 0.8, 0.10): a project's task runs as sessions filling
//! its seats, a builder and then a reviewer on another model. The builder's model call past its task's cap pauses the
//! session and asks the owner in the task's chat with a budget card; the owner's raise resumes it. A message the
//! builder sends to the reviewer's empty seat is the reviewer session's first read. The reviewer's verdict finishes the
//! task: its sessions end, its seats close and what it did not spend goes back to its project.
//!
//! noded and the agent gateway are stood in for: noded takes the sessions' VM starts (each its own CID) and reports
//! them; the gateway takes the work the hub pushes, which the test answers as the harness would. Weft is off, so the
//! task has no lane and is done on its review.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// The CID noded gives the first VM it starts; each next one gets the next.
const FIRST_CID: u64 = 500;
const WAIT: Duration = Duration::from_secs(20);

/// The VMs noded started, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// noded and the agent gateway, sharing the VMs noded started.
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

/// Waits for a work item pushed to `cid`; the first one.
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

/// `cid`'s model call for `work`: the hub's answer.
fn model_call(hub: &Hub, cid: u64, work: u64, req: u64) -> Value {
    let request = json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Do the task." }] }] });
    let message = json!({ "type": "model_call", "req": req, "work": work, "request": request });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// `cid`'s tool request: the hub's decision.
fn tool(hub: &Hub, cid: u64, work: u64, name: &str, input: Value) -> Value {
    let message = json!({ "type": "tool_request", "work": work, "call": "toolu_1", "name": name, "input": input });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// The task's sessions, oldest first.
fn sessions(hub: &Hub, task: &str) -> Vec<Value> {
    let (_, list) = hub.call("GET", &format!("/v1/sessions?task={task}"), None);
    let mut list = list.as_array().cloned().unwrap_or_default();
    list.sort_by_key(|s| s["created_ms"].as_u64().unwrap_or(0));
    list
}

#[test]
fn a_task_runs_as_sessions_in_its_seats_and_waits_at_its_budget_until_the_owner_raises_it() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "run_tests", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } });
    let scratch = Scratch::new("work-engine");
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );

    // A project with a budget in tokens, its builder and reviewer on two models.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Release notes", "goal": "Write the 0.9 release notes.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, budget) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/budget"),
        Some(json!({ "unit": "tokens", "cap": 10_000_000 })),
    );
    assert_eq!(status, 200, "{budget}");
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": "mock/agent", "reviewer": "mock/echo" } })),
    );
    assert_eq!(status, 200, "{settings}");

    // The task, with a cap its first model call does not fit in.
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Draft the release notes", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);
    let (status, budget) = hub.call(
        "PUT",
        &format!("/v1/tasks/{tid}/budget"),
        Some(json!({ "unit": "tokens", "cap": 300 })),
    );
    assert_eq!(status, 200, "{budget}");

    // The work loop opens the task's seats and starts the builder's session: a VM of its own.
    let seats = hub.wait_for(&format!("/v1/tasks/{tid}/seats"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|seat| seat["name"] == "builder" && seat["state"] == "filled")
        })
    });
    assert!(
        seats.as_array().is_some_and(|s| s.len() == 2),
        "a builder and a reviewer: {seats}\n{}",
        hub.log()
    );
    // The seat is filled when its session is made; the session has its VM once noded's start returns.
    hub.wait_for(&format!("/v1/sessions?task={tid}"), WAIT, |s| {
        s.as_array()
            .is_some_and(|s| s.iter().any(|s| s["cid"].as_u64().is_some()))
    });
    let builder = sessions(&hub, &tid)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("a session: {}", hub.log()));
    assert_eq!(builder["seat"], "builder", "{builder}");
    assert_eq!(builder["model"], "mock/agent", "{builder}");
    let builder_cid = builder["cid"].as_u64().expect("its VM");
    let (status, hello) = hub.guest(builder_cid, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200, "{hello}");
    let (work, prompt) = first_work(&gateway, builder_cid);
    assert!(prompt.contains("Draft the release notes"), "{prompt}");
    assert!(prompt.contains("You are this task's builder"), "{prompt}");

    // Its first call does not fit the task's 300 tokens: the session waits, and the owner is asked in the task's chat.
    let reply = model_call(&hub, builder_cid, work, 1);
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(text(&reply["error"]).starts_with("paused: "), "{reply}");
    let (_, paused) = hub.call(
        "GET",
        &format!("/v1/sessions/{}", text(&builder["id"])),
        None,
    );
    assert_eq!(paused["state"], "paused", "{paused}");
    assert_eq!(paused["paused_by"], "budget", "{paused}");
    let (_, cards) = hub.call("GET", "/v1/cards?open=true", None);
    let card = cards
        .as_array()
        .and_then(|c| {
            c.iter()
                .find(|c| c["subject"] == format!("task:{tid}"))
                .cloned()
        })
        .unwrap_or_else(|| panic!("a budget card: {cards}"));
    assert_eq!(card["kind"], "budget", "{card}");
    let chat = text(&card["chat"]);
    let (_, messages) = hub.call(
        "GET",
        &format!("/v1/chats/{chat}/messages?after=0&limit=50"),
        None,
    );
    assert!(
        messages
            .as_array()
            .is_some_and(|m| m.iter().any(|m| m["card"] == card["id"])),
        "the question is in the task's chat: {messages}"
    );
    // A second refused call asks no second time.
    assert_eq!(model_call(&hub, builder_cid, work, 2)["ok"], false);
    let (_, cards) = hub.call("GET", "/v1/cards?open=true", None);
    assert_eq!(cards.as_array().map(Vec::len), Some(1), "{cards}");

    // The owner raises it: the session runs again and the call goes out.
    let (status, answered) = hub.call(
        "POST",
        &format!("/v1/cards/{}", text(&card["id"])),
        Some(json!({ "choice": "raise_to", "amount": 1_000_000 })),
    );
    assert_eq!(status, 200, "{answered}");
    let (_, running) = hub.call(
        "GET",
        &format!("/v1/sessions/{}", text(&builder["id"])),
        None,
    );
    assert_eq!(running["state"], "running", "{running}");
    let reply = model_call(&hub, builder_cid, work, 3);
    assert_eq!(reply["ok"], true, "{reply}");
    // Its transcript and its context are the session's.
    let builder_id = text(&builder["id"]);
    let (_, transcript) = hub.call(
        "GET",
        &format!("/v1/sessions/{builder_id}/transcript"),
        None,
    );
    let entries = transcript.as_array().cloned().unwrap_or_default();
    assert!(
        entries.iter().any(|e| e["kind"] == "model_response"),
        "{transcript}"
    );
    assert!(
        entries.iter().all(|e| e["session"] == builder_id.as_str()),
        "{transcript}"
    );
    let (_, context) = hub.call("GET", &format!("/v1/sessions/{builder_id}/context"), None);
    assert!(context["used"].as_u64().unwrap_or(0) > 0, "{context}");
    let (_, budget) = hub.call("GET", &format!("/v1/tasks/{tid}/budget"), None);
    assert!(
        budget["spent"].as_u64().unwrap_or(0) > 0,
        "the call was charged: {budget}"
    );
    let (_, ledger) = hub.call("GET", &format!("/v1/tasks/{tid}/budget/ledger"), None);
    let kinds: Vec<String> = ledger
        .as_array()
        .map(|e| e.iter().map(|e| text(&e["kind"])).collect())
        .unwrap_or_default();
    assert!(
        kinds.contains(&"hold".to_string()) && kinds.contains(&"charge".to_string()),
        "{ledger}"
    );

    // The builder leaves the reviewer a note: nobody fills that seat yet, so it waits there.
    let note = tool(
        &hub,
        builder_cid,
        work,
        "plaza_message_seat",
        json!({ "to": "reviewer", "text": "Check the upgrade section first." }),
    );
    assert!(
        text(&note["output"]).contains("waits for the next session"),
        "{note}"
    );
    // The builder reads the thread its note opened, by the id the send reported. Nobody else has written in
    // it, so there is no one for the builder's own answer to go to.
    let thread_id = text(&note["output"])
        .split_whitespace()
        .nth(3)
        .unwrap_or_default()
        .trim_end_matches(':')
        .to_string();
    let read = tool(
        &hub,
        builder_cid,
        work,
        "thread_read",
        json!({ "thread": thread_id }),
    );
    assert!(
        text(&read["output"]).contains("Check the upgrade section first."),
        "{read}"
    );
    let answer = tool(
        &hub,
        builder_cid,
        work,
        "thread_answer",
        json!({ "thread": thread_id, "text": "Noted." }),
    );
    assert_eq!(answer["is_error"], true, "{answer}");
    assert!(
        text(&answer["output"]).contains("waits for an answer from you"),
        "{answer}"
    );

    // A tool the builder ran reports how long it took: the hub keeps each tool's latency.
    let report = json!({
        "type": "tool_report",
        "work": work,
        "call": "toolu_9",
        "ok": true,
        "output": "ok",
        "name": "run",
        "input": { "command": "true" },
        "duration_ms": 30,
    });
    let (status, body) = hub.guest(builder_cid, "message", report);
    assert_eq!(status, 200, "{body}");
    let (_, tools) = hub.call("GET", "/v1/evals/tools", None);
    assert_eq!(
        tools,
        json!([{ "tool": "run", "calls": 1, "errors": 0, "p50_ms": 30, "p95_ms": 30 }]),
        "the hub keeps the tool's duration"
    );

    // The builder finishes: the reviewer's session starts on the other model and reads the note first.
    hub.work_done(
        builder_cid,
        work,
        "done",
        "Drafted the notes; the upgrade section is new.",
    );
    let reviewer = hub.wait_for(&format!("/v1/sessions?task={tid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["seat"] == "reviewer" && s["cid"].is_u64())
        })
    });
    let reviewer = reviewer
        .as_array()
        .and_then(|s| s.iter().find(|s| s["seat"] == "reviewer").cloned())
        .unwrap_or_else(|| panic!("a reviewer session: {reviewer}\n{}", hub.log()));
    assert_eq!(
        reviewer["model"], "mock/echo",
        "a model other than the builder's: {reviewer}"
    );
    let (_, ended) = hub.call(
        "GET",
        &format!("/v1/sessions/{}", text(&builder["id"])),
        None,
    );
    assert_eq!(ended["state"], "ended", "{ended}");
    assert!(
        text(&ended["handover"]).contains("upgrade section"),
        "{ended}"
    );
    let reviewer_cid = reviewer["cid"].as_u64().expect("its VM");
    assert_ne!(reviewer_cid, builder_cid, "a VM of its own");
    let (status, _) = hub.guest(
        reviewer_cid,
        "hello",
        json!({ "role": "reviewer", "vm": 1 }),
    );
    assert_eq!(status, 200);
    let (review_work, review_prompt) = first_work(&gateway, reviewer_cid);
    assert!(
        review_prompt.contains("Check the upgrade section first."),
        "{review_prompt}"
    );
    assert!(
        review_prompt.contains("The builder's handover"),
        "{review_prompt}"
    );

    // The verdict: the task is done, its seats close, and its unspent reservation goes back to the project.
    let verdict = tool(
        &hub,
        reviewer_cid,
        review_work,
        "plaza_verdict",
        json!({ "task": tid, "pass": true, "notes": "Accurate and complete." }),
    );
    assert!(text(&verdict["output"]).contains("passes"), "{verdict}");
    let task = hub.wait_for(&format!("/v1/tasks/{tid}"), WAIT, |t| t["state"] == "done");
    assert_eq!(task["state"], "done", "{task}");
    // Its seats close as it ends (a moment after the task reads done).
    let seats = hub.wait_for(&format!("/v1/tasks/{tid}/seats"), WAIT, |s| {
        s.as_array().is_some_and(Vec::is_empty)
    });
    assert_eq!(seats.as_array().map(Vec::len), Some(0), "{seats}");
    assert!(
        sessions(&hub, &tid)
            .iter()
            .all(|s| s["state"] == "ended" || s["state"] == "failed"),
        "{:?}",
        sessions(&hub, &tid)
    );
    let (_, budget) = hub.call("GET", &format!("/v1/tasks/{tid}/budget"), None);
    assert_eq!(budget["reserved"], 0, "{budget}");
    let (_, messages) = hub.call(
        "GET",
        &format!("/v1/chats/{chat}/messages?after=0&limit=50"),
        None,
    );
    assert!(
        messages
            .as_array()
            .is_some_and(|m| m.iter().any(|m| text(&m["text"]).contains("is done"))),
        "the task's chat says it is done: {messages}"
    );
    // Each step session left its eval record: the reviewer's with its verdict, the builder's on its model.
    let runs = hub.wait_for(&format!("/v1/evals/runs?project={pid}"), WAIT, |r| {
        r.as_array().is_some_and(|r| r.len() == 2)
    });
    let runs = runs.as_array().cloned().unwrap_or_default();
    let run_of = |seat: &str| {
        runs.iter()
            .find(|r| r["seat"] == seat)
            .cloned()
            .unwrap_or_default()
    };
    let (builder_run, reviewer_run) = (run_of("builder"), run_of("reviewer"));
    assert_eq!(
        (builder_run["model"].as_str(), builder_run["ok"].as_bool()),
        (Some("mock/agent"), Some(true)),
        "{builder_run}"
    );
    assert!(
        builder_run["tokens_in"].as_u64().unwrap_or(0) > 0,
        "{builder_run}"
    );
    assert_eq!(
        builder_run["tool_ms"], 30,
        "the builder's tool time: {builder_run}"
    );
    assert_eq!(
        (
            reviewer_run["model"].as_str(),
            reviewer_run["verdict"].as_bool(),
            reviewer_run["ok"].as_bool()
        ),
        (Some("mock/echo"), Some(true), Some(true)),
        "{reviewer_run}"
    );
    assert_eq!(
        (
            builder_run["recipe"].as_str(),
            builder_run["engine"].as_str()
        ),
        (Some("change"), Some("harness")),
        "{builder_run}"
    );
    let (_, scores) = hub.call("GET", "/v1/evals/scores", None);
    let builder_score = scores
        .as_array()
        .and_then(|s| s.iter().find(|s| s["seat"] == "builder").cloned())
        .unwrap_or_default();
    assert_eq!(
        (
            builder_score["runs"].as_u64(),
            builder_score["successes"].as_u64()
        ),
        (Some(1), Some(1)),
        "{scores}"
    );
    every_save_succeeded(&hub);
}

#[test]
fn a_second_session_on_a_task_starts_from_the_journal_and_the_handover_its_builder_left() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let role =
        |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } });
    let scratch = Scratch::new("work-engine-journal");
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Release notes", "goal": "Write the 0.9 release notes.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": "mock/agent", "reviewer": "mock/echo" } })),
    );
    assert_eq!(status, 200, "{settings}");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Draft the release notes", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);
    hub.wait_for(&format!("/v1/tasks/{tid}/seats"), WAIT, |s| {
        s.as_array().is_some_and(|s| s.len() == 2)
    });
    hub.wait_for(&format!("/v1/sessions?task={tid}"), WAIT, |s| {
        s.as_array()
            .is_some_and(|s| s.iter().any(|s| s["cid"].as_u64().is_some()))
    });
    let builder = sessions(&hub, &tid)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("a session: {}", hub.log()));
    let builder_cid = builder["cid"].as_u64().expect("its VM");
    let (status, _) = hub.guest(builder_cid, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200);
    let (work, _) = first_work(&gateway, builder_cid);

    // The builder writes its journal as it goes, and ends with its handover.
    let output = |reply: Value| text(&reply["output"]);
    let noted = output(tool(
        &hub,
        builder_cid,
        work,
        "journal_note",
        json!({ "kind": "decision", "text": "keep the notes in one file", "reason": "one place to read" }),
    ));
    assert_eq!(noted, format!("Noted: decision n1 on {tid}."), "{noted}");
    output(tool(
        &hub,
        builder_cid,
        work,
        "journal_note",
        json!({ "kind": "dead_end", "text": "splitting by release", "why": "the links broke" }),
    ));
    output(tool(
        &hub,
        builder_cid,
        work,
        "journal_note",
        json!({ "kind": "open_question", "text": "is 0.8 in scope?", "thread": "th1" }),
    ));
    let written = output(tool(
        &hub,
        builder_cid,
        work,
        "handover_write",
        json!({
            "stands": "The notes draft is in.",
            "done": "Drafted the upgrade section.",
            "next": "Proof the changelog links.",
            "check_first": "The 0.9 anchor."
        }),
    ));
    assert!(written.contains("Handover recorded"), "{written}");
    let read = output(tool(
        &hub,
        builder_cid,
        work,
        "journal_read",
        json!({ "query": "one file" }),
    ));
    assert!(
        read.contains("- decision n1: keep the notes in one file Reason: one place to read"),
        "{read}"
    );
    assert!(
        !read.contains("splitting by release"),
        "the query narrows the notes: {read}"
    );
    // The summary is only a fallback: the handover the builder wrote is the one the next session starts from.
    hub.work_done(builder_cid, work, "done", "A summary the handover beats.");

    // The reviewer's session, the task's second, starts from the brief, the journal and the handover.
    let reviewer = hub.wait_for(&format!("/v1/sessions?task={tid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["seat"] == "reviewer" && s["cid"].is_u64())
        })
    });
    let reviewer_cid = reviewer
        .as_array()
        .and_then(|s| s.iter().find(|s| s["seat"] == "reviewer"))
        .and_then(|s| s["cid"].as_u64())
        .unwrap_or_else(|| panic!("a reviewer session: {reviewer}\n{}", hub.log()));
    let (status, _) = hub.guest(
        reviewer_cid,
        "hello",
        json!({ "role": "reviewer", "vm": 1 }),
    );
    assert_eq!(status, 200);
    let (review_work, prompt) = first_work(&gateway, reviewer_cid);
    assert!(
        prompt.contains(&format!("Task {tid}: Draft the release notes")),
        "{prompt}"
    );
    assert!(prompt.contains("The task's journal:"), "{prompt}");
    assert!(
        prompt.contains("- decision n1: keep the notes in one file Reason: one place to read"),
        "{prompt}"
    );
    assert!(
        prompt.contains("- dead end n2: splitting by release Why: the links broke"),
        "{prompt}"
    );
    assert!(
        prompt.contains("- open question n3: is 0.8 in scope? Thread: th1"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Where things stand: The notes draft is in."),
        "{prompt}"
    );
    assert!(prompt.contains("Proof the changelog links."), "{prompt}");
    assert!(
        !prompt.contains("A summary the handover beats."),
        "{prompt}"
    );
    let brief = output(tool(&hub, reviewer_cid, review_work, "brief", json!({})));
    assert!(
        brief.starts_with(&format!("Task {tid}: Draft the release notes")),
        "{brief}"
    );
    let (_, ended) = hub.call(
        "GET",
        &format!("/v1/sessions/{}", text(&builder["id"])),
        None,
    );
    assert!(
        text(&ended["handover"]).contains("Proof the changelog links."),
        "{ended}"
    );
}

/// After a saver pass (every second), no component's write failed (a child row without its parent, a bad value).
fn every_save_succeeded(hub: &Hub) {
    std::thread::sleep(Duration::from_millis(1500));
    let log = hub.log();
    assert!(!log.contains("store: saving"), "a save failed:\n{log}");
}

/// The fleet's orchestrator VM.
const ORCHESTRATOR: u64 = 109;

#[test]
fn the_main_orchestrator_and_a_projects_planner_are_long_sessions_their_chats_talk_to() {
    let vms: Vms = Arc::new(Mutex::new(vec![(ORCHESTRATOR, "orchestrator".to_string())]));
    let (noded, gateway) = daemons(&vms);
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "run_tests", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "planner": role("planner", "Planner"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } });
    let scratch = Scratch::new("work-engine-long");
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );

    // The fleet's orchestrator says hello: it is the main orchestrator's long-lived session, and the main chat is with it.
    let (status, _) = hub.guest(
        ORCHESTRATOR,
        "hello",
        json!({ "role": "orchestrator", "vm": 0 }),
    );
    assert_eq!(status, 200);
    let (_, plaza) = hub.call("GET", "/v1/sessions?project=plaza", None);
    let main = plaza
        .as_array()
        .and_then(|s| s.first().cloned())
        .unwrap_or_else(|| panic!("the main orchestrator's session: {plaza}\n{}", hub.log()));
    assert_eq!(main["kind"], "long", "{main}");
    assert_eq!(main["state"], "running", "{main}");
    assert_eq!(main["cid"], ORCHESTRATOR, "{main}");
    let (status, _) = hub.call(
        "POST",
        "/v1/chats/main/messages",
        Some(json!({ "text": "What runs now?" })),
    );
    assert_eq!(status, 201);
    let (work, prompt) = first_work(&gateway, ORCHESTRATOR);
    assert!(prompt.contains("What runs now?"), "{prompt}");
    hub.work_done(
        ORCHESTRATOR,
        work,
        "done",
        "Nothing yet: no project is open.",
    );
    let main_chat = hub.wait_for("/v1/chats/main/messages?after=0&limit=50", WAIT, |m| {
        m.as_array()
            .is_some_and(|m| m.iter().any(|m| m["author"] == "session"))
    });
    assert!(
        main_chat.as_array().is_some_and(|m| m
            .iter()
            .any(|m| text(&m["text"]).contains("no project is open"))),
        "the answer is in the main chat: {main_chat}"
    );

    // A project opens: it gets its planner, a long-lived session on a VM of its own.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Docs", "goal": "Write the guide.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let planners = hub.wait_for(&format!("/v1/sessions?project={pid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["role"] == "planner" && s["cid"].is_u64())
        })
    });
    let planner = planners
        .as_array()
        .and_then(|s| s.iter().find(|s| s["role"] == "planner").cloned())
        .unwrap_or_else(|| panic!("a planner session: {planners}\n{}", hub.log()));
    assert_eq!(planner["kind"], "long", "{planner}");
    let planner_cid = planner["cid"].as_u64().expect("its VM");
    let (status, _) = hub.guest(planner_cid, "hello", json!({ "role": "planner", "vm": 1 }));
    assert_eq!(status, 200);

    // The owner's message in a project chat goes to the planner, and its answer comes back there.
    let (status, chat) = hub.call(
        "POST",
        "/v1/chats",
        Some(json!({ "project": pid, "title": "Scope" })),
    );
    assert_eq!(status, 201, "{chat}");
    let chat = text(&chat["id"]);
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/chats/{chat}/messages"),
        Some(json!({ "text": "Keep it short." })),
    );
    assert_eq!(status, 201);
    let (work, prompt) = first_work(&gateway, planner_cid);
    assert!(prompt.contains("Keep it short."), "{prompt}");
    hub.work_done(planner_cid, work, "done", "Two pages, then.");
    let answered = hub.wait_for(
        &format!("/v1/chats/{chat}/messages?after=0&limit=50"),
        WAIT,
        |m| {
            m.as_array()
                .is_some_and(|m| m.iter().any(|m| text(&m["text"]).contains("Two pages")))
        },
    );
    assert!(
        answered
            .as_array()
            .is_some_and(|m| m.iter().any(|m| text(&m["text"]).contains("Two pages"))),
        "{answered}"
    );
    // One planner per project: the next ticks start no second one.
    std::thread::sleep(Duration::from_secs(5));
    let (_, sessions) = hub.call("GET", &format!("/v1/sessions?project={pid}"), None);
    let planners = sessions
        .as_array()
        .map(|s| s.iter().filter(|s| s["role"] == "planner").count());
    assert_eq!(planners, Some(1), "{sessions}");
    every_save_succeeded(&hub);
}

/// A VM that runs no session (one the owner spawned) spends from the plaza's own budget: past its cap the call waits
/// and the owner is asked in the main chat, as for a session's call; the owner's raise lets the call go out.
#[test]
fn a_vm_with_no_session_spends_from_the_plazas_own_budget() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let roles = json!({ "roles": {
        "builder": { "role": "builder", "title": "Builder", "tools": ["run", "read"] },
    } });
    let scratch = Scratch::new("work-engine-no-session");
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );
    let (status, budget) = hub.call(
        "PUT",
        "/v1/projects/plaza/budget",
        Some(json!({ "unit": "tokens", "cap": 300 })),
    );
    assert_eq!(status, 200, "{budget}");

    // The owner spawns a builder by hand: its VM runs no session.
    let (status, spawned) = hub.call(
        "POST",
        "/v1/agents",
        Some(json!({ "role": "builder", "model": "mock/agent" })),
    );
    assert_eq!(status, 200, "{spawned}\n{}", hub.log());
    let cid = vms
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(_, role)| role == "builder")
        .map(|(cid, _)| *cid)
        .expect("noded started its VM");
    let (status, hello) = hub.guest(cid, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200, "{hello}");
    let (_, listed) = hub.call("GET", "/v1/sessions", None);
    assert!(
        listed
            .as_array()
            .is_some_and(|s| s.iter().all(|s| s["cid"].as_u64() != Some(cid))),
        "no session runs in it: {listed}"
    );

    // Its first call does not fit the plaza's 300 tokens: it waits, and the owner is asked in the main chat.
    let reply = model_call(&hub, cid, 1, 1);
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(text(&reply["error"]).starts_with("paused: "), "{reply}");
    let (_, cards) = hub.call("GET", "/v1/cards?open=true", None);
    let card = cards
        .as_array()
        .and_then(|c| c.iter().find(|c| c["kind"] == "budget").cloned())
        .unwrap_or_else(|| panic!("a budget card: {cards}"));
    assert_eq!(card["project"], "plaza", "{card}");
    let (_, chats) = hub.call("GET", "/v1/chats", None);
    let main = chats
        .as_array()
        .and_then(|c| c.iter().find(|c| c["kind"] == "main").cloned())
        .unwrap_or_else(|| panic!("the main chat: {chats}"));
    assert_eq!(card["chat"], main["id"], "{card}");

    // The owner raises it: the call goes out and is charged to the plaza.
    let (status, answered) = hub.call(
        "POST",
        &format!("/v1/cards/{}", text(&card["id"])),
        Some(json!({ "choice": "raise_to", "amount": 1_000_000 })),
    );
    assert_eq!(status, 200, "{answered}");
    let reply = model_call(&hub, cid, 1, 2);
    assert_eq!(reply["ok"], true, "{reply}");
    let (_, budget) = hub.call("GET", "/v1/projects/plaza/budget", None);
    assert!(
        budget["spent"].as_u64().unwrap_or(0) > 0,
        "the call was charged to the plaza: {budget}"
    );
    every_save_succeeded(&hub);
}

/// How long a test lets the loop tick (every 2 s) after the team reaches its size, so a team that would overshoot does.
const SETTLE: Duration = Duration::from_secs(5);

/// A project with a budget of `cap` tokens and its builder and reviewer on two models.
fn team_project(hub: &Hub, cap: u64) -> String {
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Changelog", "goal": "Keep the changelog.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, budget) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/budget"),
        Some(json!({ "unit": "tokens", "cap": cap })),
    );
    assert_eq!(status, 200, "{budget}");
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": "mock/agent", "reviewer": "mock/echo" } })),
    );
    assert_eq!(status, 200, "{settings}");
    pid
}

/// `count` tasks of project `pid`, none after another: each is ready at once.
fn ready_tasks(hub: &Hub, pid: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            let (status, task) = hub.call(
                "POST",
                "/v1/tasks",
                Some(json!({ "goal": format!("Write changelog entry {i}"), "project": pid })),
            );
            assert_eq!(status, 201, "{task}");
            text(&task["id"])
        })
        .collect()
}

/// How many of `tasks` hold seats now, which is how many the work loop has started.
fn holding(hub: &Hub, tasks: &[String]) -> usize {
    tasks
        .iter()
        .filter(|tid| {
            let (_, seats) = hub.call("GET", &format!("/v1/tasks/{tid}/seats"), None);
            seats.as_array().is_some_and(|s| !s.is_empty())
        })
        .count()
}

/// Waits until `want` of `tasks` hold seats, then lets the loop tick for a while: how many hold them then.
fn team_after_settling(hub: &Hub, tasks: &[String], want: usize) -> usize {
    let deadline = std::time::Instant::now() + WAIT.mul_f64(common::slack());
    while holding(hub, tasks) < want && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(SETTLE);
    holding(hub, tasks)
}

#[test]
fn a_project_runs_up_to_its_team_maximum_at_once_and_widens_when_the_maximum_does() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let role =
        |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } });
    let scratch = Scratch::new("work-engine-team");
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );

    // Twelve independent tasks in a project with room in its budget, and a team of four.
    let pid = team_project(&hub, 100_000_000);
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "team_max": 4 })),
    );
    assert_eq!(status, 200, "{settings}");
    let tasks = ready_tasks(&hub, &pid, 12);
    assert_eq!(team_after_settling(&hub, &tasks, 4), 4, "{}", hub.log());

    // The owner widens the team: one more task starts, and no more than the new maximum.
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "team_max": 5 })),
    );
    assert_eq!(status, 200, "{settings}");
    assert_eq!(team_after_settling(&hub, &tasks, 5), 5, "{}", hub.log());
    let (_, effective) = hub.call(
        "GET",
        &format!("/v1/projects/{pid}/settings/effective"),
        None,
    );
    assert_eq!(
        effective["settings"]["team_max"],
        json!({ "value": 5, "source": "project" }),
        "{effective}"
    );
}

#[test]
fn a_team_waits_at_its_projects_budget_rate_and_asks_nobody_for_more() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let role =
        |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } });
    let scratch = Scratch::new("work-engine-budget-rate");
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );

    // A budget of 5 000 000 tokens and tasks with none of their own: each expects the default reserve of 2 000 000,
    // so two fit at once, though the team's maximum is the default twenty.
    let pid = team_project(&hub, 5_000_000);
    let tasks = ready_tasks(&hub, &pid, 12);
    assert_eq!(team_after_settling(&hub, &tasks, 2), 2, "{}", hub.log());

    // The task that waits is not short of budget: the project's running tasks hold the rest, so no card is asked.
    let (_, cards) = hub.call("GET", "/v1/cards?open=true", None);
    assert!(
        cards.as_array().is_some_and(|c| c
            .iter()
            .all(|c| !(c["kind"] == "budget" && c["project"].as_str() == Some(pid.as_str())))),
        "a waiting task asks the owner for nothing: {cards}"
    );

    // The owner raises the budget: the team grows to what the new budget holds beside the running tasks' reserves.
    let (status, budget) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/budget"),
        Some(json!({ "unit": "tokens", "cap": 10_000_000 })),
    );
    assert_eq!(status, 200, "{budget}");
    assert_eq!(team_after_settling(&hub, &tasks, 5), 5, "{}", hub.log());
}

#[test]
fn a_planner_sizes_its_projects_team_and_a_size_out_of_bounds_is_refused() {
    let vms: Vms = Arc::new(Mutex::new(vec![(ORCHESTRATOR, "orchestrator".to_string())]));
    let (noded, gateway) = daemons(&vms);
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "run_tests", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "planner": role("planner", "Planner"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } });
    let scratch = Scratch::new("work-engine-team-tool");
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );

    // A project opens and gets its planner, a long-lived session on a VM of its own.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Guide", "goal": "Write the guide.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let planners = hub.wait_for(&format!("/v1/sessions?project={pid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["role"] == "planner" && s["cid"].is_u64())
        })
    });
    let planner_cid = planners
        .as_array()
        .and_then(|s| s.iter().find(|s| s["role"] == "planner"))
        .and_then(|s| s["cid"].as_u64())
        .unwrap_or_else(|| panic!("a planner session: {planners}\n{}", hub.log()));
    let (status, _) = hub.guest(planner_cid, "hello", json!({ "role": "planner", "vm": 1 }));
    assert_eq!(status, 200);

    // The planner widens the team and aims it at four.
    let reply = tool(
        &hub,
        planner_cid,
        1,
        "team_set",
        json!({ "max": 6, "target": 4 }),
    );
    assert!(
        text(&reply["output"]).contains("at most 6 tasks at once, aiming for 4"),
        "{reply}"
    );
    let (_, effective) = hub.call(
        "GET",
        &format!("/v1/projects/{pid}/settings/effective"),
        None,
    );
    assert_eq!(effective["settings"]["team_max"]["value"], 6, "{effective}");
    assert_eq!(
        effective["settings"]["team_target"]["value"], 4,
        "{effective}"
    );

    // A maximum past the limit is refused, and the team stays as it was.
    let reply = tool(&hub, planner_cid, 2, "team_set", json!({ "max": 101 }));
    assert_eq!(reply["is_error"], true, "{reply}");
    let (_, effective) = hub.call(
        "GET",
        &format!("/v1/projects/{pid}/settings/effective"),
        None,
    );
    assert_eq!(effective["settings"]["team_max"]["value"], 6, "{effective}");
}
