//! The project gate, end to end on a scratch hub. An agent works on a project through its live session: a project's planner
//! is one, and a task's seat another. An agent with no session is on no project and takes no task; a paused project holds
//! its sessions' calls and chat until the owner resumes it; an archived project ends its agents' sessions, cancels their
//! work and withdraws their open tasks. The agents' VMs are played by calls to the internal API, as the agent gateway
//! makes them, and noded and the gateway are stood in for: noded starts a project's planner on the CID the test names.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(20);

/// The roles the planner's project needs: a project's planner is the `planner` role.
fn roles() -> Value {
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["bash", "read_file"] });
    json!({ "roles": {
        "builder": role("builder", "Builder"),
        "planner": role("planner", "Planner"),
    } })
}

/// Agent `cid`'s model call, as its harness sends one mid-turn: the hub's answer.
fn model_call(hub: &Hub, cid: u64) -> Value {
    let message = json!({
        "type": "model_call",
        "req": 1,
        "work": 1,
        "request": { "model": "", "messages": [] },
    });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// Agent `cid`'s request for a tool no role is granted, as its harness sends one: the hub's decision. The gate
/// answers before the grants do, and a granted tool is never reached, so nothing is run or checkpointed.
fn tool_request(hub: &Hub, cid: u64) -> Value {
    let message = json!({
        "type": "tool_request",
        "work": 1,
        "call": "toolu_1",
        "name": "no_such_tool",
        "input": {},
    });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// The VMs noded started, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// noded and the agent gateway, sharing the VMs noded started: each new run's VM gets the next CID from `first`.
fn daemons(first: u64) -> (FakeDaemon, FakeDaemon) {
    let vms: Vms = Arc::default();
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

/// The work items the hub pushed to agent `cid` through the gateway.
fn work_pushed(gateway: &FakeDaemon, cid: u64) -> usize {
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, body)| {
            method == "POST"
                && path.ends_with(&format!("/agents/{cid}/send"))
                && serde_json::from_str::<Value>(body).is_ok_and(|b| b["message"]["type"] == "work")
        })
        .count()
}

/// The project's planner session, once noded has started its VM: returns its CID.
fn planner_of(hub: &Hub, pid: &str) -> u64 {
    let found = hub.wait_for(&format!("/v1/sessions?project={pid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["role"] == "planner" && s["cid"].is_u64())
        })
    });
    found
        .as_array()
        .and_then(|s| s.iter().find(|s| s["role"] == "planner").cloned())
        .and_then(|s| s["cid"].as_u64())
        .unwrap_or_else(|| panic!("a planner session: {found}\n{}", hub.log()))
}

#[test]
fn an_agent_with_no_session_is_on_no_project_and_takes_no_task() {
    let scratch = Scratch::new("project-gate-none");
    let hub = Hub::start(&scratch, None, None);
    // A builder says hello with no session: it is on no project, and no project holds it.
    let (status, body) = hub.guest(310, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200, "{body}");
    assert!(
        text(&body["profile"]["system"]).contains("You have no project"),
        "{body}"
    );
    let (_, agent) = hub.call("GET", "/v1/agents/310", None);
    assert_eq!(agent["project"], Value::Null, "{agent}");
    assert_eq!(agent["held"], Value::Null, "{agent}");

    // Its tool call is decided by the grants alone: no project holds it, so the refusal is the grants'.
    let decision = tool_request(&hub, 310);
    assert_eq!(decision["decision"], "deny", "{decision}");
    assert!(!text(&decision["output"]).contains("paused"), "{decision}");

    // A task of a paused project (no seat starts on it) is claimed by no one with no session: the claim names no project.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Gate", "goal": "Check the gate" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, paused) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200, "{paused}");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "First", "project": pid, "role": "builder" })),
    );
    assert_eq!(status, 201, "{task}");
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/tasks/{}/claim", text(&task["id"])),
        Some(json!({ "agent_cid": 310 })),
    );
    assert_eq!(
        status, 409,
        "an agent with no session takes no task: {refused}"
    );
    assert!(
        text(&refused["error"]).contains("has no project"),
        "{refused}"
    );
    let (_, on) = hub.call("GET", &format!("/v1/agents?project={pid}"), None);
    assert_eq!(on, json!([]), "{on}");
}

#[test]
fn a_paused_projects_session_is_held_until_the_owner_resumes_it() {
    let (noded, gateway) = daemons(300);
    let scratch = Scratch::new("project-gate-paused");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Pause", "goal": "Check the pause" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    // The project's planner: a long-lived session on the VM noded started for it, which the work loop starts on a tick.
    let cid = planner_of(&hub, &pid);
    let (status, body) = hub.guest(cid, "hello", json!({ "role": "planner", "vm": 0 }));
    assert_eq!(status, 200, "{body}");

    // On a live project the agent is on it: it is listed there, and no project holds its calls.
    let (_, on) = hub.call("GET", &format!("/v1/agents?project={pid}"), None);
    assert!(
        on.as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["cid"] == cid)),
        "{on}"
    );
    let (_, agent) = hub.call("GET", &format!("/v1/agents/{cid}"), None);
    assert_eq!(agent["project"], pid.as_str(), "{agent}");
    assert_eq!(agent["held"], Value::Null, "{agent}");
    let decision = tool_request(&hub, cid);
    assert_eq!(decision["decision"], "deny", "{decision}");
    assert!(
        !text(&decision["output"]).contains("paused"),
        "a live project's agent passes the gate: {decision}"
    );

    // Paused: the model call gets the harness's paused answer, the tool call is refused and the chat is refused, each
    // with the reason.
    let (status, paused) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200, "{paused}");
    let reply = model_call(&hub, cid);
    assert_eq!(reply["type"], "model_result", "{reply}");
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(text(&reply["error"]).starts_with("paused: "), "{reply}");
    assert!(text(&reply["error"]).contains("paused"), "{reply}");
    let decision = tool_request(&hub, cid);
    assert_eq!(decision["decision"], "deny", "{decision}");
    assert!(text(&decision["output"]).contains("paused"), "{decision}");
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/agents/{cid}/chat"),
        Some(json!({ "text": "hi" })),
    );
    assert_eq!(status, 409, "{refused}");
    assert!(text(&refused["error"]).contains("paused"), "{refused}");

    // Resumed: the same agent works again. Its chat goes out, and its calls pass the gate.
    let (status, _) = hub.call("POST", &format!("/v1/projects/{pid}/resume"), None);
    assert_eq!(status, 200);
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/agents/{cid}/chat"),
        Some(json!({ "text": "Back to work?" })),
    );
    assert_eq!(status, 200);
    let deadline = Instant::now() + WAIT.mul_f64(common::slack());
    while work_pushed(&gateway, cid) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        work_pushed(&gateway, cid) >= 1,
        "the resumed agent's chat reached it: {}",
        hub.log()
    );
    let decision = tool_request(&hub, cid);
    assert!(
        !text(&decision["output"]).contains("paused"),
        "a resumed project's agent passes the gate again: {decision}"
    );
}

#[test]
fn an_archived_project_ends_its_agents_work_and_withdraws_its_open_tasks() {
    let (noded, gateway) = daemons(320);
    let scratch = Scratch::new("project-gate-archive");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Archive", "goal": "Check the archive" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let cid = planner_of(&hub, &pid);
    let (status, body) = hub.guest(cid, "hello", json!({ "role": "planner", "vm": 0 }));
    assert_eq!(status, 200, "{body}");

    // The planner takes two tasks: the first runs, the second waits in its queue behind it.
    let mut tasks = Vec::new();
    for goal in ["Runs", "Waits"] {
        let (status, task) = hub.call(
            "POST",
            "/v1/tasks",
            Some(json!({ "goal": goal, "project": pid, "assignee": cid })),
        );
        assert_eq!(status, 201, "{task}");
        tasks.push(text(&task["id"]));
    }
    let queued = hub.wait_for(&format!("/v1/agents/{cid}"), WAIT, |a| {
        a["queue"] == 1 && a["work"].is_object()
    });
    assert_eq!(queued["queue"], 1, "{queued}");
    // A task nobody has taken: paused, no seat starts on it, so it stays open until the archive.
    let (status, paused) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200, "{paused}");
    let (status, open) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Open", "project": pid })),
    );
    assert_eq!(status, 201, "{open}");
    let open = text(&open["id"]);

    // Archived: the agent is on no project, its running work is cancelled once, and no task is left open or claimed
    // for the next agent. The task nobody took is withdrawn.
    let (status, archived) = hub.call("POST", &format!("/v1/projects/{pid}/archive"), None);
    assert_eq!(status, 200, "{archived}");
    assert_eq!(archived["archived"], true);
    let (_, on) = hub.call("GET", &format!("/v1/agents?project={pid}"), None);
    assert_eq!(on, json!([]), "{on}");
    let (_, live) = hub.call(
        "GET",
        &format!("/v1/sessions?project={pid}&live=true"),
        None,
    );
    assert_eq!(live, json!([]), "{live}");
    let cancels = gateway
        .seen()
        .into_iter()
        .filter(|(_, path, body)| {
            path.ends_with(&format!("/agents/{cid}/send")) && body.contains("\"cancel\"")
        })
        .count();
    assert_eq!(cancels, 1, "the running work is told to stop");
    let (_, untaken) = hub.call("GET", &format!("/v1/tasks/{open}"), None);
    assert_eq!(untaken["state"], "withdrawn", "{untaken}");
    for task in &tasks {
        // The tasks its agent had are closed as the agent goes (failed, "agent went offline") or withdrawn: never
        // left open or claimed.
        let (_, closed) = hub.call("GET", &format!("/v1/tasks/{task}"), None);
        assert!(
            matches!(closed["state"].as_str(), Some("withdrawn" | "failed")),
            "{closed}"
        );
    }
}

#[test]
fn stop_all_holds_a_tool_call_that_was_on_its_way() {
    let (noded, gateway) = daemons(330);
    let scratch = Scratch::new("stop-all-tool");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Stop", "goal": "Check STOP ALL" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    // A planner of a live project: no project gate holds it, so only STOP ALL can.
    let cid = planner_of(&hub, &pid);
    let (status, body) = hub.guest(cid, "hello", json!({ "role": "planner", "vm": 0 }));
    assert_eq!(status, 200, "{body}");
    let egress = || {
        let check = json!({ "cid": cid, "host": "example.com", "port": 443 });
        let (status, answer) = hub.internal("/internal/egress/check", check);
        assert_eq!(status, 200, "{answer}");
        answer
    };
    assert!(!text(&egress()["reason"]).contains("STOP ALL"));
    let (status, state) = hub.call("POST", "/v1/plaza/stop", Some(json!({ "reason": "audit" })));
    assert_eq!(status, 200, "{state}");
    // A tool request the harness sent before its work's cancel arrived is refused, and says why.
    let decision = tool_request(&hub, cid);
    assert_eq!(decision["decision"], "deny", "{decision}");
    assert!(text(&decision["output"]).contains("STOP ALL"), "{decision}");
    // The agent reaches no host until the owner resumes, and the answer says why.
    let answer = egress();
    assert_eq!(answer["allow"], false, "{answer}");
    assert!(text(&answer["reason"]).contains("STOP ALL"), "{answer}");
    let (status, state) = hub.call("POST", "/v1/plaza/resume", None);
    assert_eq!(status, 200, "{state}");
    assert!(!text(&egress()["reason"]).contains("STOP ALL"));
}

#[test]
fn a_task_never_leaves_its_project_and_a_move_needs_an_existing_one() {
    let scratch = Scratch::new("project-gate-move");
    let hub = Hub::start(&scratch, None, None);
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Adopt", "goal": "Take in the old tasks" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    // No route makes a task outside projects any more, so the move's refusals are what this hub can show.
    let (status, refused) = hub.call("POST", "/v1/tasks/t9/move", Some(json!({ "project": pid })));
    assert_eq!(status, 404, "{refused}");
    let (status, refused) = hub.call(
        "POST",
        "/v1/tasks/t9/move",
        Some(json!({ "project": "no-such-project" })),
    );
    assert_eq!(status, 404, "{refused}");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "In the project", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/tasks/{}/move", text(&task["id"])),
        Some(json!({ "project": pid })),
    );
    assert_eq!(status, 409, "a task never leaves its project: {refused}");
}
