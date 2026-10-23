//! A draft from the owner's main chat to a started project, end to end on a scratch hub: the owner asks the main chat, the
//! orchestrator (played by the test) drafts the project with its tool, and the draft lives in the project's own chat
//! with a start card. The owner edits the draft, and each revision asks again. Start opens the project with its budget,
//! GitHub mode and disk quota, posts its tasks, and its planner's session starts on the next tick. The orchestrator is
//! told in the project's chat, and a task it posts there goes to the project.
//!
//! noded and the agent gateway are stood in for: noded reports the agents' VMs (so the hub trusts their roles) and
//! starts the VMs the hub asks for, and the gateway takes the work the hub pushes, which the test answers as the agents'
//! harnesses would.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const ORCHESTRATOR: u64 = 109;
/// A builder the fleet runs: it has no session, so no project holds it.
const BUILDER: u64 = 300;
/// The first VM noded starts for a session (the planner's, then the seats'): each next one gets the next CID.
const FIRST_SESSION_VM: u64 = 400;

/// The VMs noded runs, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// The orchestrator's tool request for `name` in work item `work`: the hub's decision.
fn tool(hub: &Hub, work: u64, name: &str, input: Value) -> Value {
    let message = json!({ "type": "tool_request", "work": work, "call": "toolu_1", "name": name, "input": input });
    let (status, body) = hub.guest(ORCHESTRATOR, "message", message);
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

/// The work the hub pushed to the orchestrator on `thread`, once it has been pushed: (work, prompt).
fn pushed_on(gateway: &FakeDaemon, thread: &str, about: &str) -> (u64, String) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found = pushed(gateway, ORCHESTRATOR)
            .into_iter()
            .rfind(|(_, t, prompt)| t == thread && prompt.contains(about));
        if let Some((work, _, prompt)) = found {
            return (work, prompt);
        }
        assert!(
            Instant::now() < deadline,
            "no work on {thread} about {about:?}: {:?}",
            pushed(gateway, ORCHESTRATOR)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// noded and the agent gateway, sharing the VMs noded runs: a new run's VM gets the next CID from [`FIRST_SESSION_VM`].
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
            let cid = FIRST_SESSION_VM
                + vms
                    .iter()
                    .filter(|(cid, _)| *cid >= FIRST_SESSION_VM)
                    .count() as u64;
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
fn a_draft_in_the_main_chat_becomes_a_project_the_owner_starts() {
    let vms: Vms = Arc::new(Mutex::new(vec![
        (ORCHESTRATOR, "orchestrator".to_string()),
        (BUILDER, "builder".to_string()),
    ]));
    let (noded, gateway) = daemons(&vms);
    let role =
        |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "read"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "builder": role("builder", "Builder"),
        "planner": role("planner", "Planner"),
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

    // The owner asks the main chat; the orchestrator's turn on it is pushed to it.
    let (status, said) = hub.call(
        "POST",
        "/v1/chats/main/messages",
        Some(json!({ "text": "The scheduler tests are flaky; get 0.9 out." })),
    );
    assert_eq!(status, 201, "{said}");
    let (asked, _) = pushed_on(&gateway, "chat:main", "scheduler tests");
    let (_, main_chat) = hub.call("GET", "/v1/chats/main?limit=10", None);
    assert_eq!(main_chat["works"], json!([asked]), "{main_chat}");

    // The orchestrator drafts the project: its tasks, budget, GitHub mode and disk quota.
    let draft = json!({
        "action": "write",
        "title": "Release 0.9",
        "goal": "Make the scheduler tests deterministic, then tag 0.9.",
        "repo": "agentplaza, branch release-0.9",
        "tasks": [
            { "key": "1", "goal": "Survey the flaky scheduler tests", "role": "qa" },
            { "key": "2", "goal": "Make the scheduler tests deterministic", "role": "builder", "after": ["1"] },
        ],
        "budget": { "unit": "usd", "cap": 9000000 },
        "github": "mirror",
        "disk_quota_bytes": 2_147_483_648u64,
    });
    let written = tool(&hub, asked, "plaza_project_draft", draft);
    assert!(
        text(&written["output"]).contains("revision 1 saved"),
        "{written}"
    );
    assert!(
        text(&written["output"]).contains("Nothing stops Start"),
        "{written}"
    );
    hub.work_done(
        ORCHESTRATOR,
        asked,
        "done",
        "Here is the draft; press Start when it looks right.",
    );

    // The draft is a project of its own, at brief, in its own chat; the main chat says where that chat is.
    let listed = tool(
        &hub,
        asked,
        "plaza_project_draft",
        json!({ "action": "read" }),
    );
    let listed: Value = serde_json::from_str(&text(&listed["output"])).unwrap_or_default();
    let pid = text(&listed["drafts"][0]["project"]);
    let chat = text(&listed["drafts"][0]["chat"]);
    assert!(pid.starts_with("pr") && chat.starts_with("ch"), "{listed}");
    let (_, project) = hub.call("GET", &format!("/v1/projects/{pid}"), None);
    assert_eq!(project["stage"], "brief", "a draft runs nothing: {project}");
    assert_eq!(project["github"], "mirror", "{project}");
    let (_, main) = hub.call("GET", "/v1/chats/main/messages?after=0&limit=50", None);
    assert!(
        main.as_array().is_some_and(|m| m
            .iter()
            .any(|m| text(&m["text"]).contains(&format!("chat {chat}")))),
        "the main chat names the draft's chat: {main}"
    );
    let (_, chat_view) = hub.call("GET", &format!("/v1/chats/{chat}"), None);
    assert_eq!(chat_view["kind"], "project", "{chat_view}");
    assert_eq!(
        chat_view["draft"]["disk_quota_bytes"], 2_147_483_648u64,
        "the quota waits on the draft: {chat_view}"
    );
    assert_eq!(chat_view["project"], pid.as_str(), "{chat_view}");
    assert_eq!(chat_view["draft"]["revision"], 1, "{chat_view}");
    let (_, draft) = hub.call("GET", &format!("/v1/chats/{chat}/draft"), None);
    assert_eq!(draft["problems"], json!([]), "{draft}");
    let (_, cards) = hub.call("GET", &format!("/v1/cards?chat={chat}&open=true"), None);
    assert_eq!(cards.as_array().map(Vec::len), Some(1), "{cards}");
    let old_card = text(&cards[0]["id"]);
    assert_eq!(cards[0]["kind"], "start", "{cards}");
    assert_eq!(cards[0]["subject"], format!("draft:{pid}"), "{cards}");
    assert_eq!(cards[0]["choices"][0]["id"], "start", "{cards}");
    assert_eq!(cards[0]["choices"][1]["id"], "edit-later", "{cards}");

    // The owner raises the budget and changes the GitHub mode: revision 2, and the card asks again.
    let (status, edited) = hub.call(
        "PATCH",
        &format!("/v1/chats/{chat}/draft"),
        Some(json!({ "revision": 1, "budget": { "unit": "usd", "cap": 8000000 }, "github": "ci" })),
    );
    assert_eq!(status, 200, "{edited}");
    assert_eq!(edited["revision"], 2, "{edited}");
    assert_eq!(edited["budget"]["cap"], 8000000, "{edited}");
    assert_eq!(edited["github"], "ci", "{edited}");
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/cards/{old_card}"),
        Some(json!({ "choice": "edit-later" })),
    );
    assert_eq!(
        status, 409,
        "the card of revision 1 is withdrawn: {refused}"
    );
    let (_, cards) = hub.call("GET", &format!("/v1/cards?chat={chat}&open=true"), None);
    assert_eq!(cards.as_array().map(Vec::len), Some(1), "{cards}");
    let card = text(&cards[0]["id"]);
    assert_ne!(card, old_card, "{cards}");

    // The owner says something in the chat: the orchestrator is told of the edit it has not read.
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/chats/{chat}/messages"),
        Some(json!({ "text": "Looks better." })),
    );
    assert_eq!(status, 201);
    let (noted, prompt) = pushed_on(
        &gateway,
        &format!("chat:{chat}"),
        "edited the project draft",
    );
    assert!(prompt.contains("revision 2"), "{prompt}");
    assert!(prompt.contains("Looks better."), "{prompt}");
    hub.work_done(
        ORCHESTRATOR,
        noted,
        "done",
        "Read it: revision 2 is current.",
    );

    // The orchestrator, on revision 1, is told to read again; a read makes revision 2 the one it has seen.
    let stale = tool(
        &hub,
        noted,
        "plaza_project_draft",
        json!({ "action": "write", "project": pid, "revision": 1, "title": "Release 0.9.1" }),
    );
    assert!(text(&stale["output"]).contains("revision 2"), "{stale}");
    let read = tool(
        &hub,
        noted,
        "plaza_project_draft",
        json!({ "action": "read", "project": pid }),
    );
    let read: Value = serde_json::from_str(&text(&read["output"])).unwrap_or_default();
    assert_eq!(read["draft"]["title"], "Release 0.9", "{read}");

    // Start takes the revision the owner looked at: a stale one is refused, the card's start opens the project.
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/chats/{chat}/start"),
        Some(json!({ "revision": 1 })),
    );
    assert_eq!(status, 409, "a stale revision does not start: {refused}");
    let (status, answered) = hub.call(
        "POST",
        &format!("/v1/cards/{card}"),
        Some(json!({ "choice": "start" })),
    );
    assert_eq!(status, 200, "{answered}\n{}", hub.log());
    assert_eq!(answered["card"]["state"], "answered", "{answered}");
    assert_eq!(
        answered["done"],
        format!("project {pid} started"),
        "{answered}"
    );

    // The project: the owner's plan, the orchestrator its lead, its mode and quota, its two tasks.
    let (_, project) = hub.call("GET", &format!("/v1/projects/{pid}"), None);
    assert_eq!(project["stage"], "implementation", "{project}");
    assert_eq!(project["lead_cid"], ORCHESTRATOR, "{project}");
    assert_eq!(project["approved_by"], "owner", "{project}");
    assert_eq!(project["github"], "ci", "{project}");
    assert!(text(&project["goal"]).contains("release-0.9"), "{project}");
    assert_eq!(
        project["tasks"].as_array().map(Vec::len),
        Some(2),
        "{project}"
    );
    let (_, builder) = hub.call("GET", &format!("/v1/agents/{BUILDER}"), None);
    assert!(builder["project"].is_null(), "no agent joined: {builder}");
    // Its budget is the draft's, in its unit.
    let (_, budget) = hub.call("GET", &format!("/v1/projects/{pid}/budget"), None);
    assert_eq!(
        budget["disk"]["quota_bytes"], 2_147_483_648u64,
        "the draft's quota is the budget's: {budget}"
    );
    assert_eq!(
        (budget["unit"].clone(), budget["cap"].clone()),
        (json!("usd"), json!(8000000)),
        "{budget}"
    );

    // The started draft is not edited, nor started again.
    let (status, _) = hub.call(
        "PATCH",
        &format!("/v1/chats/{chat}/draft"),
        Some(json!({ "revision": 2, "title": "Late" })),
    );
    assert_eq!(status, 409);
    let (status, again) = hub.call(
        "POST",
        &format!("/v1/chats/{chat}/start"),
        Some(json!({ "revision": 2 })),
    );
    assert_eq!(status, 409, "a draft starts one project: {again}");

    // The planner's session starts on the next tick, and the chat says the project started.
    hub.wait_for(
        &format!("/v1/sessions?project={pid}"),
        Duration::from_secs(20),
        |s| {
            s.as_array()
                .is_some_and(|s| s.iter().any(|s| s["role"] == "planner"))
        },
    );
    let (_, chat_view) = hub.call("GET", &format!("/v1/chats/{chat}?limit=50"), None);
    let messages = chat_view["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        messages
            .iter()
            .any(|m| text(&m["text"]).contains(&format!("Project {pid} \"Release 0.9\" started"))),
        "{chat_view}"
    );

    // The orchestrator is told of the start in the project's chat, and a task it posts there goes to the project.
    let (started, prompt) = pushed_on(&gateway, &format!("chat:{chat}"), "started project");
    assert!(prompt.contains(&pid), "{prompt}");
    let posted = tool(
        &hub,
        started,
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

    // Nothing serves the owner's conversations any more; the main chat's turns and context are its own.
    let (status, _) = hub.call("GET", "/v1/owner/conversations", None);
    assert_eq!(status, 404);
    let (status, turns) = hub.call("GET", "/v1/chats/main/transcript", None);
    assert_eq!(status, 200, "{turns}");
    assert!(turns.is_array(), "{turns}");
    let (status, context) = hub.call("GET", "/v1/chats/main/context", None);
    assert_eq!(status, 200, "{context}");
    // The main chat's model goes back to the orchestrator's own (null).
    let (status, cleared) = hub.call("PATCH", "/v1/chats/main", Some(json!({ "model": null })));
    assert_eq!(status, 200, "{cleared}");
    assert!(cleared.get("model").is_none_or(Value::is_null), "{cleared}");
}
