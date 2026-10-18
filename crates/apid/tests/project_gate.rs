//! The project gate, end to end on a scratch hub. An agent on the bench makes no model or tool call and takes no
//! chat; a project's task goes only to its agents; a paused project holds them; an archived one sends them back to
//! the bench. The agents' VMs are played by calls to the internal API, as the agent gateway makes them.

mod common;

use serde_json::{Value, json};

use common::{Hub, Scratch, text};

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

#[test]
fn only_a_projects_agents_work_and_only_while_it_runs() {
    let scratch = Scratch::new("project-gate");
    let hub = Hub::start(&scratch, None, None);

    // Two agents say hello. Neither is on a project, so both are on the bench, and their prompt says so.
    for cid in [300, 301] {
        let (status, body) = hub.guest(cid, "hello", json!({ "role": "builder", "vm": 0 }));
        assert_eq!(status, 200, "{body}");
        let system = text(&body["profile"]["system"]);
        assert!(system.contains("You are on the bench"), "{system}");
    }
    let (status, agent) = hub.call("GET", "/v1/agents/300", None);
    assert_eq!(status, 200, "{agent}");
    assert_eq!(agent["project"], Value::Null);
    assert!(text(&agent["held"]).contains("on the bench"), "{agent}");

    // On the bench: no model call, no tool call, no chat.
    let reply = model_call(&hub, 300);
    assert_eq!(reply["type"], "model_result", "{reply}");
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(text(&reply["error"]).contains("on the bench"), "{reply}");
    let decision = tool_request(&hub, 300);
    assert_eq!(decision["decision"], "deny", "{decision}");
    assert!(
        text(&decision["output"]).contains("on the bench"),
        "{decision}"
    );
    let (status, refused) = hub.call("POST", "/v1/agents/300/chat", Some(json!({ "text": "hi" })));
    assert_eq!(status, 409, "{refused}");
    assert!(
        text(&refused["error"]).contains("on the bench"),
        "{refused}"
    );

    // Every task belongs to a project.
    let (status, refused) = hub.call("POST", "/v1/tasks", Some(json!({ "goal": "Loose" })));
    assert_eq!(status, 400, "{refused}");
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Gate", "goal": "Check the gate" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "First", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);
    let claim = |cid: u64| {
        hub.call(
            "POST",
            &format!("/v1/tasks/{tid}/claim"),
            Some(json!({ "agent_cid": cid })),
        )
    };
    let (status, refused) = claim(300);
    assert_eq!(status, 409, "a bench agent takes no task: {refused}");

    // On the project, 300 works; 301 stays on the bench and still takes nothing.
    let (status, member) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/members"),
        Some(json!({ "cid": 300 })),
    );
    assert_eq!(status, 200, "{member}");
    assert_eq!(member["project"], pid.as_str());
    assert_eq!(member["held"], Value::Null);
    let (_, on) = hub.call("GET", &format!("/v1/agents?project={pid}"), None);
    let cids = |rows: &Value| -> Vec<u64> {
        rows.as_array()
            .map(|rows| rows.iter().filter_map(|r| r["cid"].as_u64()).collect())
            .unwrap_or_default()
    };
    assert_eq!(cids(&on), [300]);
    let (_, bench) = hub.call("GET", "/v1/agents?project=bench", None);
    assert_eq!(cids(&bench), [301]);
    let (status, refused) = claim(301);
    assert_eq!(
        status, 409,
        "a project's task goes only to its agents: {refused}"
    );
    let (status, claimed) = claim(300);
    assert_eq!(status, 200, "{claimed}");
    assert_eq!(claimed["state"], "claimed");
    // A member's call passes the gate: only the grants refuse it now. (Its model calls are not tried here: with no
    // keys they would go to the provider's gateway.)
    let decision = tool_request(&hub, 300);
    assert_eq!(decision["decision"], "deny", "{decision}");
    let output = text(&decision["output"]);
    assert!(
        !output.contains("bench") && !output.contains("paused"),
        "a member's tool call passes the gate: {decision}"
    );

    // A paused project holds its agents mid-turn too.
    let (status, paused) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200, "{paused}");
    assert!(text(&model_call(&hub, 300)["error"]).contains("paused"));
    let (status, refused) = hub.call("POST", "/v1/agents/300/chat", Some(json!({ "text": "hi" })));
    assert_eq!(status, 409, "{refused}");
    let (status, _) = hub.call("POST", &format!("/v1/projects/{pid}/resume"), None);
    assert_eq!(status, 200);

    // An archived project sends its agents back to the bench.
    let (status, archived) = hub.call("POST", &format!("/v1/projects/{pid}/archive"), None);
    assert_eq!(status, 200, "{archived}");
    assert_eq!(archived["archived"], true);
    let (_, agent) = hub.call("GET", "/v1/agents/300", None);
    assert_eq!(agent["project"], Value::Null, "{agent}");
    assert!(text(&model_call(&hub, 300)["error"]).contains("on the bench"));
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/members"),
        Some(json!({ "cid": 300 })),
    );
    assert_eq!(
        status, 409,
        "an archived project takes no agents: {refused}"
    );
}

#[test]
fn a_task_from_before_projects_moves_into_one_with_its_links() {
    let scratch = Scratch::new("project-gate");
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
    let (status, suggested) = hub.call(
        "GET",
        &format!("/v1/projects/{pid}/suggested-members"),
        None,
    );
    assert_eq!(status, 200, "{suggested}");
    assert_eq!(suggested, json!([]));
}
