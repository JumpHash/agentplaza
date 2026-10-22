//! A project's settings, end to end on a scratch hub: the project's overrides reach its agents (their model and its
//! source, the limits in the profile pushed to them, the permission mode, a task's default budget), the effective
//! view says where each value comes from, and the project's budget funds its tasks and goes back to the plaza when
//! the project is archived. noded and the agent gateway are stood in for, as in the other scratch-hub tests.

mod common;

use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const BUILDER: u64 = 300;

/// The last profile the hub pushed to agent `cid`.
fn last_profile(gateway: &FakeDaemon, cid: u64) -> Value {
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, _)| {
            method == "POST" && path.ends_with(&format!("/agents/{cid}/send"))
        })
        .filter_map(|(_, _, body)| serde_json::from_str::<Value>(&body).ok())
        .rfind(|body| body["message"]["type"] == "profile")
        .map(|body| body["message"]["profile"].clone())
        .unwrap_or(Value::Null)
}

#[test]
fn a_projects_settings_reach_its_agents_and_its_budget_funds_its_tasks() {
    let noded = FakeDaemon::serve(|method, path, _| {
        if method == "GET" && path.starts_with("/v1/runs") {
            let runs = json!({ "runs": [{
                "id": "r1", "ended_at": null,
                "vms": [{ "id": 0, "guest_cid": BUILDER, "role": "builder" }],
            }] });
            (200, runs.to_string())
        } else {
            (200, "{}".into())
        }
    });
    let gateway = FakeDaemon::serve(|_, path, _| {
        if path.starts_with("/v1/status") {
            (
                200,
                json!({ "agents": [{ "cid": BUILDER, "role": "builder" }] }).to_string(),
            )
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    });
    let roles = json!({ "roles": {
        "builder": { "role": "builder", "title": "Builder", "tools": ["run", "run_tests", "read"] },
    } });
    let scratch = Scratch::new("project-settings");
    let hub = Hub::start_with_roles(&scratch, Some(gateway.port), Some(noded.port), Some(&roles));
    let agent = hub.wait_for(
        &format!("/v1/agents/{BUILDER}"),
        Duration::from_secs(20),
        |a| a["role"] == "builder" && a["status"] == "idle",
    );
    assert_eq!(agent["role"], "builder", "{agent}\n{}", hub.log());
    let plaza_model = text(&agent["model"]);
    assert_eq!(agent["model_source"], "default");

    let (_, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Settings", "goal": "Check the project layer" })),
    );
    let pid = text(&project["id"]);
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/members"),
        Some(json!({ "cid": BUILDER })),
    );
    assert_eq!(status, 200);

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

    // The agent runs the project's model for its role, then its default once that goes.
    let (_, agent) = hub.call("GET", &format!("/v1/agents/{BUILDER}"), None);
    assert_eq!(
        (agent["model"].clone(), agent["model_source"].clone()),
        (json!("openai/gpt-6-luna"), json!("project"))
    );
    let (_, _) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": null } })),
    );
    let (_, agent) = hub.call("GET", &format!("/v1/agents/{BUILDER}"), None);
    assert_eq!(agent["model"], "anthropic/claude-sonnet-5-5", "{agent}");
    // The profile pushed to it has the project's limits.
    let profile = last_profile(&gateway, BUILDER);
    assert_eq!(
        (profile["max_turns"].clone(), profile["keep_recent"].clone()),
        (json!(40), json!(9)),
        "{profile}"
    );
    assert_eq!(profile["model"], "anthropic/claude-sonnet-5-5");
    // Its permission mode is the project's until the owner sets its own.
    let (_, mode) = hub.call("GET", &format!("/v1/agents/{BUILDER}/permissions"), None);
    assert_eq!(
        (mode["mode"].clone(), mode["source"].clone()),
        (json!("ask"), json!("project"))
    );
    let request = json!({ "type": "tool_request", "work": 1, "call": "toolu_1", "name": "run", "input": { "command": "ls" } });
    let (_, decided) = hub.guest(BUILDER, "message", request);
    assert!(
        text(&decided["reply"]["output"]).contains("approval"),
        "ask mode holds a call: {decided}"
    );

    // A task that names no budget gets the project's; with a budget of its own, the project funds its tasks.
    let (_, before) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    let plaza_at_start = before["plaza"].as_i64().unwrap_or(0);
    assert_eq!(before["own"], false, "{before}");
    let (_, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "First", "project": pid })),
    );
    assert_eq!(task["budget"], 3000, "{task}");
    let (status, budget) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/budget"),
        Some(json!({ "credits": 5000 })),
    );
    assert_eq!(status, 200, "{budget}");
    assert_eq!(budget["balance"], 5000);
    let plaza_before = budget["plaza"].as_i64().unwrap_or(0);
    let (_, second) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Second", "project": pid })),
    );
    assert_eq!(second["budget"], 3000);
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

    // Archived: its open tasks are withdrawn, each escrow back where it came from (the first task's to the plaza, the
    // second's to the project), and what the project has left goes back to the plaza: the plaza is whole again.
    let (status, _) = hub.call("POST", &format!("/v1/projects/{pid}/archive"), None);
    assert_eq!(status, 200);
    let (_, budget) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    assert_eq!(budget["balance"], 0, "{budget}");
    assert_eq!(budget["plaza"].as_i64(), Some(plaza_at_start), "{budget}");
    let (_, second) = hub.call("GET", &format!("/v1/tasks/{}", text(&second["id"])), None);
    assert_eq!(second["state"], "withdrawn", "{second}");
    // Back on the bench, the agent runs the plaza's model again.
    let (_, agent) = hub.call("GET", &format!("/v1/agents/{BUILDER}"), None);
    assert_eq!(
        agent["model"].as_str(),
        Some(plaza_model.as_str()),
        "{agent}"
    );
}
