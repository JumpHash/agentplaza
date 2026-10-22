//! The prompt layout (PLAN 13.3) on a scratch hub: a session's system prompt is parts, most stable first, and the sessions
//! of one project share the first three byte for byte. Two seats of project A and the planner of project B are the
//! sessions: A's seats agree on parts 1 to 3, and B's planner differs from them only from part 3 on. The VMs are noded's
//! and the gateway's stand-ins, as in `project_gate`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(20);

/// The roles the projects need: a project's planner and a task's builder.
fn roles() -> Value {
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["bash", "read_file"] });
    json!({ "roles": {
        "builder": role("builder", "Builder"),
        "planner": role("planner", "Planner"),
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

/// The session of `role` on project `pid`, once noded has started its VM: its CID.
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

/// The system prompt parts agent `cid` is welcomed with, as its harness says hello.
fn parts_of(hub: &Hub, cid: u64, role: &str) -> Vec<Value> {
    let (status, body) = hub.guest(cid, "hello", json!({ "role": role, "vm": 0 }));
    assert_eq!(status, 200, "{body}");
    body["profile"]["system_parts"]
        .as_array()
        .unwrap_or_else(|| panic!("system parts: {body}"))
        .clone()
}

/// A project with a planner, and a task whose builder seat starts on it: returns the planner's and the seat's CIDs.
fn project_with_seat(hub: &Hub, title: &str) -> (String, u64, u64) {
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": title, "goal": format!("Work on {title}") })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let planner = session_of(hub, &pid, "planner");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Build it", "project": pid, "role": "builder" })),
    );
    assert_eq!(status, 201, "{task}");
    let seat = session_of(hub, &pid, "builder");
    (pid, planner, seat)
}

#[test]
fn the_sessions_of_one_project_share_parts_one_to_three_and_another_project_differs_from_part_three()
 {
    let (noded, gateway) = daemons(300);
    let scratch = Scratch::new("prompt-layout");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let (project_a, planner_a, seat_a) = project_with_seat(&hub, "Alpha");
    let (project_b, planner_b, _) = project_with_seat(&hub, "Beta");
    assert_ne!(project_a, project_b);

    let a_planner = parts_of(&hub, planner_a, "planner");
    let a_seat = parts_of(&hub, seat_a, "builder");
    let b_planner = parts_of(&hub, planner_b, "planner");
    assert_eq!(a_planner.len(), 4, "{a_planner:?}");

    // Two sessions of one project: parts 1 to 3 are the same bytes, and the seat's own part 4 is not.
    assert_eq!(a_planner[..3], a_seat[..3]);
    assert_ne!(a_planner[3], a_seat[3]);
    // A session of another project differs only from part 3 on.
    assert_eq!(a_planner[..2], b_planner[..2]);
    assert_ne!(a_planner[2], b_planner[2]);
    assert!(
        text(&b_planner[2]["text"]).contains("Beta"),
        "{b_planner:?}"
    );
    // Only parts 1 to 3 are cached, and part 4 is the seat's.
    assert_eq!(
        a_planner
            .iter()
            .map(|p| p["cache"] == true)
            .collect::<Vec<_>>(),
        [true, true, true, false]
    );
    // Nothing in the shared parts names the seat.
    for part in &a_planner[..3] {
        assert!(
            !text(&part["text"]).contains(&format!("CID {planner_a}")),
            "{part}"
        );
    }
}
