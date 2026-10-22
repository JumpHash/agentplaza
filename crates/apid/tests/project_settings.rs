//! A project's settings, end to end on a scratch hub: the project's overrides reach the agents on its sessions (the model
//! a session starts on for its role, the limits in the profile it starts with, the permission mode), the effective view
//! says where each value comes from, a task's default budget is the project's, and the project's budget funds its tasks
//! and goes back to the plaza when the project is archived. An agent is on a project through its session: the builder
//! seat of the project's task starts one on a VM of its own. noded and the agent gateway are stood in for, as in the
//! other scratch-hub tests.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// A builder the fleet runs: it has no session, so no project's settings reach it.
const BUILDER: u64 = 300;
/// The VM the first builder seat starts. Each next VM noded starts gets the next CID.
const SEAT_VM: u64 = 500;
const WAIT: Duration = Duration::from_secs(20);

/// The VMs noded runs, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// noded and the agent gateway, sharing the VMs noded runs: a new run's VM gets the next CID from [`SEAT_VM`].
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
            let cid = SEAT_VM + vms.iter().filter(|(cid, _)| *cid >= SEAT_VM).count() as u64;
            vms.push((cid, role));
            let vm = json!({ "id": vms.len() - 1, "guest_cid": cid, "state": "running" });
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

/// The builder session the seat of task `task` started: its VM's CID is set once noded has started it.
fn builder_session(hub: &Hub, task: &str) -> Value {
    let found = hub.wait_for(&format!("/v1/sessions?task={task}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["seat"] == "builder" && s["cid"].is_u64())
        })
    });
    found
        .as_array()
        .and_then(|s| {
            s.iter()
                .find(|s| s["seat"] == "builder" && s["cid"].is_u64())
                .cloned()
        })
        .unwrap_or_else(|| panic!("a builder session: {found}\n{}", hub.log()))
}

#[test]
fn a_projects_settings_reach_its_agents_and_its_budget_funds_its_tasks() {
    let vms: Vms = Arc::new(Mutex::new(vec![(BUILDER, "builder".to_string())]));
    let (noded, gateway) = daemons(&vms);
    let roles = json!({ "roles": {
        "builder": { "role": "builder", "title": "Builder", "tools": ["run", "read"] },
    } });
    let scratch = Scratch::new("project-settings");
    let hub = Hub::start_with_roles(&scratch, Some(gateway.port), Some(noded.port), Some(&roles));
    let agent = hub.wait_for(&format!("/v1/agents/{BUILDER}"), WAIT, |a| {
        a["role"] == "builder" && a["status"] == "idle"
    });
    assert_eq!(agent["role"], "builder", "{agent}\n{}", hub.log());
    let plaza_model = text(&agent["model"]);
    assert_eq!(agent["model_source"], "default");

    let (_, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Settings", "goal": "Check the project layer" })),
    );
    let pid = text(&project["id"]);

    // Overrides for every group: models, effort, limits, task budget, permission mode, review timeout.
    let settings = json!({
        "default_model": "anthropic/claude-sonnet-5-5",
        "role_models": { "builder": "openai/gpt-6-luna" },
        "effort": "high",
        "max_turns": 40,
        "keep_recent": 9,
        "task_budget": 3000,
        "permission_mode": "ask",
        "review_timeout_minutes": 90,
    });
    let (status, saved) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(settings),
    );
    assert_eq!(status, 200, "{saved}");
    let (status, refused) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "effort": "enormous" })),
    );
    assert_eq!(status, 400, "{refused}");
    let (status, refused) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "orchestrator": "anthropic/claude-opus-5-5" } })),
    );
    assert_eq!(status, 400, "the orchestrator is no project's: {refused}");

    // The effective view: each value with its source.
    let (_, effective) = hub.call(
        "GET",
        &format!("/v1/projects/{pid}/settings/effective"),
        None,
    );
    let source = |key: &str| effective["settings"][key]["source"].clone();
    for key in [
        "default_model",
        "effort",
        "max_turns",
        "keep_recent",
        "task_budget",
        "permission_mode",
        "review_timeout_minutes",
    ] {
        assert_eq!(source(key), "project", "{key}: {effective}");
    }
    for key in ["max_tokens", "compress_at"] {
        assert_eq!(source(key), "plaza", "{key}: {effective}");
    }
    assert_eq!(
        effective["role_models"]["builder"]["value"],
        "openai/gpt-6-luna"
    );

    // The project's first task: a task that names no budget gets the project's, and its builder seat starts a session
    // on a VM of its own, on the project's model for the builder.
    let (status, first) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "First", "project": pid })),
    );
    assert_eq!(status, 201, "{first}");
    assert_eq!(first["budget"], 3000, "{first}");
    let session = builder_session(&hub, &text(&first["id"]));
    assert_eq!(session["model"], "openai/gpt-6-luna", "{session}");
    let cid = session["cid"].as_u64().expect("its VM");
    assert_eq!(cid, SEAT_VM, "{session}");
    let (status, hello) = hub.guest(cid, "hello", json!({ "role": "builder", "vm": 1 }));
    assert_eq!(status, 200, "{hello}");

    // Its agent is on the project through its session. The profile it starts with carries the project's limits.
    let (_, on) = hub.call("GET", &format!("/v1/agents?project={pid}"), None);
    assert!(
        on.as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["cid"] == cid)),
        "{on}"
    );
    let (_, agent) = hub.call("GET", &format!("/v1/agents/{cid}"), None);
    assert_eq!(agent["project"], pid.as_str(), "{agent}");
    assert_eq!(agent["model"], "openai/gpt-6-luna", "{agent}");
    assert_eq!(
        (
            hello["profile"]["max_turns"].clone(),
            hello["profile"]["keep_recent"].clone()
        ),
        (json!(40), json!(9)),
        "{hello}"
    );
    assert_eq!(hello["profile"]["model"], "openai/gpt-6-luna", "{hello}");

    // Its permission mode is the project's until the owner sets its own.
    let (_, mode) = hub.call("GET", &format!("/v1/agents/{cid}/permissions"), None);
    assert_eq!(
        (mode["mode"].clone(), mode["source"].clone()),
        (json!("ask"), json!("project"))
    );
    let request = json!({ "type": "tool_request", "work": 1, "call": "toolu_1", "name": "run", "input": { "command": "ls" } });
    let (_, decided) = hub.guest(cid, "message", request);
    assert!(
        text(&decided["reply"]["output"]).contains("approval"),
        "ask mode holds a call: {decided}"
    );

    // The fleet builder, with no session, keeps the plaza's model: the project's settings reach only its sessions.
    let (_, fleet) = hub.call("GET", &format!("/v1/agents/{BUILDER}"), None);
    assert_eq!(
        (fleet["model"].clone(), fleet["project"].clone()),
        (json!(plaza_model), Value::Null),
        "{fleet}"
    );

    // The budget: its tasks are funded from the project's.
    let (_, before) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    let plaza_at_start = before["plaza"].as_i64().unwrap_or(0);
    assert_eq!(before["own"], false, "{before}");
    let (status, budget) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/budget"),
        Some(json!({ "credits": 5000 })),
    );
    assert_eq!(status, 200, "{budget}");
    assert_eq!(budget["balance"], 5000);
    let plaza_before = budget["plaza"].as_i64().unwrap_or(0);

    // The owner clears the builder's model for the project: the session the project starts next takes the default.
    let (_, _) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": null } })),
    );
    let (_, second) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Second", "project": pid })),
    );
    assert_eq!(second["budget"], 3000);
    let next = builder_session(&hub, &text(&second["id"]));
    assert_eq!(next["model"], "anthropic/claude-sonnet-5-5", "{next}");
    assert_ne!(next["cid"], cid, "{next}");
    let (_, budget) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    assert_eq!(
        budget["balance"], 2000,
        "the second task drew on the project: {budget}"
    );
    assert_eq!(
        budget["plaza"].as_i64(),
        Some(plaza_before),
        "not on the plaza"
    );
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/budget"),
        Some(json!({ "credits": 0 })),
    );
    assert_eq!(status, 400, "{refused}");

    // A task nobody has taken: paused, no seat starts on it, so it stays open until the archive.
    let (status, _) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200);
    // (Its budget fits in what the project has left: 2000.)
    let (status, open) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Third", "project": pid, "budget": 1000 })),
    );
    assert_eq!(status, 201, "{open}");

    // Archived: its sessions end and their agents are off it, its open task is withdrawn and the task its seat had is
    // closed (each escrow back where it came from), and what the project has left goes back to the plaza.
    let (status, _) = hub.call("POST", &format!("/v1/projects/{pid}/archive"), None);
    assert_eq!(status, 200);
    let (_, on) = hub.call("GET", &format!("/v1/agents?project={pid}"), None);
    assert_eq!(on, json!([]), "{on}");
    for session in [&session, &next] {
        let (_, ended) = hub.call(
            "GET",
            &format!("/v1/sessions/{}", text(&session["id"])),
            None,
        );
        assert_eq!(ended["state"], "ended", "{ended}");
    }
    let (_, third) = hub.call("GET", &format!("/v1/tasks/{}", text(&open["id"])), None);
    assert_eq!(third["state"], "withdrawn", "{third}");
    let (_, second) = hub.call("GET", &format!("/v1/tasks/{}", text(&second["id"])), None);
    assert!(
        matches!(second["state"].as_str(), Some("withdrawn" | "failed")),
        "the task its seat had is closed, never left claimed: {second}"
    );
    let (_, budget) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    assert_eq!(budget["balance"], 0, "{budget}");
    assert_eq!(budget["plaza"].as_i64(), Some(plaza_at_start), "{budget}");
}
