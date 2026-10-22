//! A conversation that becomes a project, end to end on a scratch hub: the orchestrator drafts it with its tool, the
//! owner edits the draft and starts it, and Start opens the project and posts its tasks. No agent joins at Start: a
//! task's seats start sessions on VMs of their own, so the first ready task's builder seat starts one on the next tick,
//! and that session takes the task. The conversation then stays the project's line: the orchestrator is told there, and
//! a task it posts there goes to the project.
//!
//! noded and the agent gateway are stood in for: noded reports the agents' VMs (so the hub trusts their roles) and
//! starts the VMs the hub asks for, and the gateway takes the work the hub pushes, which the test answers as the agents'
//! harnesses would.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const ORCHESTRATOR: u64 = 109;
/// A builder the fleet runs: it has no session, so no project holds it.
const BUILDER: u64 = 300;
/// The VM the first task's builder seat starts. Each next VM noded starts gets the next CID.
const SEAT_VM: u64 = 400;

/// The VMs noded runs, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// The orchestrator's tool request for `name` in work item `work`: the hub's decision.
fn tool(hub: &Hub, work: u64, name: &str, input: Value) -> Value {
    let message = json!({ "type": "tool_request", "work": work, "call": "toolu_1", "name": name, "input": input });
    let (status, body) = hub.guest(ORCHESTRATOR, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// The work items pushed to agent `cid`, as (work, thread).
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
                .then(|| (work["work"].as_u64().unwrap_or(0), text(&work["thread"])))
        })
        .collect()
}

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

#[test]
fn a_draft_the_owner_starts_becomes_a_project_with_its_tasks() {
    let vms: Vms = Arc::new(Mutex::new(vec![
        (ORCHESTRATOR, "orchestrator".to_string()),
        (BUILDER, "builder".to_string()),
    ]));
    let (noded, gateway) = daemons(&vms);
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["bash", "read_file"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "builder": role("builder", "Builder"),
        "qa": role("qa", "QA"),
    } });
    let scratch = Scratch::new("drafts");
    let hub = Hub::start_with_roles(&scratch, Some(gateway.port), Some(noded.port), Some(&roles));
    for (cid, role) in [(ORCHESTRATOR, "orchestrator"), (BUILDER, "builder")] {
        let agent = hub.wait_for(
            &format!("/v1/agents/{cid}"),
            Duration::from_secs(20),
            |agent| agent["role"] == role && agent["status"] == "idle",
        );
        assert_eq!(agent["role"], role, "{agent}\n{}", hub.log());
    }

    // The owner asks; the orchestrator drafts the project in the conversation: its tasks and its budget.
    let (status, conversation) = hub.call(
        "POST",
        "/v1/owner/conversations",
        Some(json!({ "text": "The scheduler tests are flaky; get 0.9 out." })),
    );
    assert_eq!(status, 201, "{conversation}");
    let id = text(&conversation["id"]);
    let asked = conversation["works"][0]
        .as_u64()
        .expect("the first message's work");
    let draft = json!({
        "action": "write",
        "revision": 0,
        "title": "Release 0.9",
        "goal": "Make the scheduler tests deterministic, then tag 0.9.",
        "repo": "agentplaza, branch release-0.9",
        "tasks": [
            { "key": "1", "goal": "Survey the flaky scheduler tests", "role": "qa" },
            { "key": "2", "goal": "Make the scheduler tests deterministic", "role": "builder", "after": ["1"] },
        ],
        "budget": 9000,
    });
    let written = tool(&hub, asked, "plaza_project_draft", draft.clone());
    assert!(
        text(&written["output"]).contains("revision 1 saved"),
        "{written}"
    );
    assert!(
        text(&written["output"]).contains("Nothing stops Start"),
        "{written}"
    );
    let (_, conversation) = hub.call("GET", &format!("/v1/owner/conversations/{id}"), None);
    assert_eq!(conversation["draft"]["revision"], 1, "{conversation}");
    assert_eq!(
        conversation["draft"]["problems"],
        json!([]),
        "{conversation}"
    );

    // The owner lowers the budget; the orchestrator, still on revision 1, is told to read again.
    let (status, edited) = hub.call(
        "PATCH",
        &format!("/v1/owner/conversations/{id}/draft"),
        Some(json!({ "revision": 1, "budget": 8000 })),
    );
    assert_eq!(status, 200, "{edited}");
    assert_eq!(edited["draft"]["revision"], 2);
    assert_eq!(edited["draft"]["budget"], 8000);
    let stale = tool(&hub, asked, "plaza_project_draft", draft);
    assert!(text(&stale["output"]).contains("revision 2"), "{stale}");
    let (_, read) = hub.call("GET", &format!("/v1/owner/conversations/{id}/draft"), None);
    assert_eq!(read["title"], "Release 0.9", "{read}");

    // Start takes the revision the owner looked at.
    let start = |revision: u64| {
        hub.call(
            "POST",
            &format!("/v1/owner/conversations/{id}/start"),
            Some(json!({ "revision": revision })),
        )
    };
    let (status, refused) = start(1);
    assert_eq!(status, 409, "a stale revision does not start: {refused}");
    let (status, started) = start(2);
    assert_eq!(status, 200, "{started}\n{}", hub.log());
    let pid = text(&started["started"]["project"]);
    assert_eq!(started["project"], pid.as_str());
    // Start joins no agent and spawns none: the project's tasks run on the sessions their seats start.
    assert!(started["started"].get("joined").is_none(), "{started}");
    assert!(started["started"].get("spawned").is_none(), "{started}");
    let tasks = started["started"]["tasks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(tasks.len(), 2, "{started}");
    let (status, again) = start(2);
    assert_eq!(status, 409, "a conversation starts one project: {again}");

    // The project: the owner's plan, the orchestrator its lead, its tasks, and no agent on it yet.
    let (_, project) = hub.call("GET", &format!("/v1/projects/{pid}"), None);
    assert_eq!(project["stage"], "implementation", "{project}");
    assert_eq!(project["lead_cid"], ORCHESTRATOR);
    assert_eq!(project["tasks"].as_array().map(Vec::len), Some(2));
    assert!(text(&project["goal"]).contains("release-0.9"), "{project}");
    let (_, builder) = hub.call("GET", &format!("/v1/agents/{BUILDER}"), None);
    assert!(builder["project"].is_null(), "no agent joined: {builder}");
    // Its budget was set aside, and its two tasks (2000 each, the default) are funded from it.
    let (_, budget) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    assert_eq!(budget["balance"], 4000, "{budget}");

    // The first task's builder seat starts a session on a VM of its own, and that session takes the task.
    let seated = hub.wait_for(
        &format!("/v1/sessions?project={pid}"),
        Duration::from_secs(20),
        |s| {
            s.as_array().is_some_and(|s| {
                s.iter()
                    .any(|s| s["seat"] == "builder" && s["cid"] == SEAT_VM)
            })
        },
    );
    assert!(
        seated
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["seat"] == "builder")),
        "a builder session: {seated}\n{}",
        hub.log()
    );
    let spawn = noded
        .seen()
        .into_iter()
        .find(|(method, path, _)| method == "POST" && path.starts_with("/v1/runs"))
        .expect("the seat asked noded for its VM");
    let spawn: Value = serde_json::from_str(&spawn.2).unwrap_or_default();
    assert_eq!(
        (spawn["role"].clone(), spawn["n"].clone()),
        (json!("builder"), json!(1))
    );

    // The seat's VM says hello, and the ready task goes to its session.
    let (status, hello) = hub.guest(SEAT_VM, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200, "{hello}");
    assert!(text(&hello["profile"]["system"]).contains(&format!("You work on project {pid}")));
    let seat_agent = hub.wait_for(
        &format!("/v1/agents/{SEAT_VM}"),
        Duration::from_secs(5),
        |a| a["project"] == pid.as_str(),
    );
    assert_eq!(seat_agent["project"], pid.as_str(), "{seat_agent}");
    let first = text(&tasks[0]["id"]);
    let claimed = hub.wait_for(
        &format!("/v1/tasks/{first}"),
        Duration::from_secs(15),
        |task| task["agent_cid"] == SEAT_VM,
    );
    assert_eq!(claimed["agent_cid"], SEAT_VM, "{claimed}\n{}", hub.log());

    // The orchestrator is told in the conversation, and a task it posts there goes to the project.
    let notice = pushed(&gateway, ORCHESTRATOR)
        .into_iter()
        .rfind(|(_, thread)| *thread == id)
        .expect("the start notice went out on the conversation's thread");
    let (_, conversation) = hub.call("GET", &format!("/v1/owner/conversations/{id}"), None);
    let plaza: Vec<String> = conversation["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["from"] == "plaza")
        .map(|m| text(&m["text"]))
        .collect();
    assert!(
        plaza
            .iter()
            .any(|m| m.contains(&format!("started project {pid}"))),
        "{plaza:?}"
    );
    // The first message's work ended long ago in a real run; here it is still the orchestrator's item, so it ends now
    // and the notice comes next.
    hub.work_done(
        ORCHESTRATOR,
        asked,
        "done",
        "Here is the draft; press Start when it looks right.",
    );
    let notice_work = pushed(&gateway, ORCHESTRATOR)
        .into_iter()
        .rfind(|(_, thread)| *thread == id)
        .map_or(notice.0, |(work, _)| work);
    let posted = tool(
        &hub,
        notice_work,
        "plaza_post_task",
        json!({ "title": "Update the changelog", "body": "List the 0.9 fixes.", "role": "builder" }),
    );
    assert!(!text(&posted["output"]).contains("refused"), "{posted}");
    let (_, project) = hub.call("GET", &format!("/v1/projects/{pid}"), None);
    assert_eq!(
        project["tasks"].as_array().map(Vec::len),
        Some(3),
        "{project}\n{posted}"
    );
    let (_, budget) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    assert_eq!(
        budget["balance"], 2000,
        "the third task drew on the project's budget too: {budget}"
    );
}
