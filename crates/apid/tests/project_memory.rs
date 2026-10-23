//! Project memory (PLAN 9.3 to 9.5) on a scratch hub: the entries on a project's main are the index in part 3 of its
//! sessions' prompts, the same bytes in each session of the project, and a landing that changes main renews the index.
//! The sessions' VMs are noded's and the gateway's stand-ins, as in `prompt_layout`.

mod common;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(20);

/// The roles the projects need: a project's planner and a task's builder.
fn roles() -> Value {
    let role =
        |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "read"] });
    json!({ "roles": {
        "builder": role("builder", "Builder"),
        "planner": role("planner", "Planner"),
        "verifier": role("verifier", "Verifier"),
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

/// Part 3 of agent `cid`'s prompt, once it holds `want` (or when the wait runs out: the caller asserts on it).
fn part_three_with(hub: &Hub, cid: u64, role: &str, want: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let part = text(&parts_of(hub, cid, role)[2]["text"]);
        if part.contains(want) || Instant::now() > deadline {
            return part;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// A project with a planner, and a task whose builder seat starts on it: returns the project, the planner's and the
/// seat's CIDs.
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

/// An entry's file, as the repo holds it.
fn entry(id: &str, kind: &str, title: &str) -> String {
    format!("---\nid: {id}\ntype: {kind}\nstatus: active\n---\n{title}\n")
}

/// Lands an owner's edit of `files` (path, text) on the repo's main.
fn land(hub: &Hub, repo: &str, title: &str, files: &[(String, String)]) {
    let files: Vec<Value> = files
        .iter()
        .map(|(path, text)| json!({ "path": path, "text": text }))
        .collect();
    let (status, body) = hub.call(
        "POST",
        &format!("/v1/weft/repos/{repo}/edits"),
        Some(json!({ "title": title, "files": files })),
    );
    assert!(
        status == 200 || status == 201,
        "{status} {body}\n{}",
        hub.log()
    );
}

#[test]
fn the_index_of_main_is_in_part_three_of_every_session_of_the_project_and_a_landing_renews_it() {
    let (noded, gateway) = daemons(400);
    let scratch = Scratch::new("project-memory");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let (pid, planner, seat) = project_with_seat(&hub, "Memory");
    let project = hub.wait_for(&format!("/v1/projects/{pid}"), WAIT, |p| {
        p["repo"].is_string()
    });
    let repo = text(&project["repo"]);

    // Three entries on main: a decision, a convention and a pitfall (9.3's folders), each with its own type.
    let dir = ".plaza/memory";
    land(
        &hub,
        &repo,
        "Seed the memory",
        &[
            (
                format!("{dir}/decisions/D-0001-sessions-not-agents.md"),
                entry("D-0001", "decision", "Sessions, not named agents."),
            ),
            (
                format!("{dir}/conventions/C-0003-merge-never-rebase.md"),
                entry(
                    "C-0003",
                    "convention",
                    "Merge main into a branch; never rebase.",
                ),
            ),
            (
                format!("{dir}/pitfalls/P-0002-move-claims-first.md"),
                entry(
                    "P-0002",
                    "pitfall",
                    "A move releases its claim before it wakes the target.",
                ),
            ),
        ],
    );

    // The index lists them by type, and the planner's and the seat's prompts carry the same bytes.
    let want = "## Project memory\n\n- D-0001 decision: Sessions, not named agents.\n\
                - C-0003 convention: Merge main into a branch; never rebase.\n\
                - P-0002 pitfall: A move releases its claim before it wakes the target.";
    let planner_part = part_three_with(&hub, planner, "planner", want);
    assert!(planner_part.contains(want), "{planner_part}\n{}", hub.log());
    let seat_part = part_three_with(&hub, seat, "builder", want);
    assert_eq!(planner_part, seat_part);

    // A landing that changes an entry renews the index: the pitfall now says more.
    land(
        &hub,
        &repo,
        "Say more about the move",
        &[(
            format!("{dir}/pitfalls/P-0002-move-claims-first.md"),
            entry(
                "P-0002",
                "pitfall",
                "A move releases its claim, then wakes the target, or the wake never starts.",
            ),
        )],
    );
    let renewed = part_three_with(
        &hub,
        planner,
        "planner",
        "- P-0002 pitfall: A move releases its claim, then wakes the target, or the wake never starts.",
    );
    assert!(
        renewed.contains("P-0002 pitfall: A move releases its claim, then wakes the target"),
        "{renewed}\n{}",
        hub.log()
    );
}

/// The work item first pushed to `cid` through the gateway, with its prompt, once one is pushed.
fn first_work(gateway: &FakeDaemon, cid: u64) -> (u64, String) {
    let deadline = Instant::now() + WAIT;
    loop {
        let found = gateway.seen().into_iter().find_map(|(method, path, body)| {
            if method != "POST" || !path.ends_with(&format!("/agents/{cid}/send")) {
                return None;
            }
            let body: Value = serde_json::from_str(&body).ok()?;
            (body["message"]["type"] == "work").then(|| {
                (
                    body["message"]["work"].as_u64().unwrap_or(0),
                    text(&body["message"]["prompt"]),
                )
            })
        });
        if let Some(found) = found {
            return found;
        }
        assert!(Instant::now() < deadline, "no work reached {cid}");
        thread::sleep(Duration::from_millis(100));
    }
}

/// `cid`'s tool request in `work`: the hub's output for it.
fn tool(hub: &Hub, cid: u64, work: u64, name: &str, input: Value) -> String {
    let message = json!({ "type": "tool_request", "work": work, "call": "toolu_1", "name": name, "input": input });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    text(&body["reply"]["output"])
}

/// The entry `id` in the project's memory view, once `want` holds for the view.
fn memory_entry(hub: &Hub, pid: &str, id: &str, want: impl Fn(&Value) -> bool) -> Value {
    let view = hub.wait_for(&format!("/v1/projects/{pid}/memory"), WAIT, |m| {
        m["entries"]
            .as_array()
            .is_some_and(|es| es.iter().any(|e| e["id"] == id && want(e)))
    });
    view["entries"]
        .as_array()
        .and_then(|es| es.iter().find(|e| e["id"] == id).cloned())
        .unwrap_or(Value::Null)
}

#[test]
fn an_entry_anchored_to_a_file_is_shown_to_the_session_that_reads_it_and_a_landing_that_touches_it_flags_it_for_the_verifier()
 {
    let (noded, gateway) = daemons(400);
    let scratch = Scratch::new("anchors");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let (pid, _planner, seat) = project_with_seat(&hub, "Anchors");
    let project = hub.wait_for(&format!("/v1/projects/{pid}"), WAIT, |p| {
        p["repo"].is_string()
    });
    let repo = text(&project["repo"]);
    let file = "crates/apid/src/placement.rs";

    // The code exists first, then an entry anchored to it lands, so the entry's own landing flags nothing.
    land(
        &hub,
        &repo,
        "Add the placement",
        &[(file.to_string(), "pub fn move_claimed() {}\n".to_string())],
    );
    land(
        &hub,
        &repo,
        "Remember the move's claim",
        &[(
            ".plaza/memory/pitfalls/P-0002-move-claims-first.md".to_string(),
            format!(
                "---\nid: P-0002\ntype: pitfall\nstatus: active\nanchors:\n  - {file}\n---\nA move releases its claim before it wakes the target.\n"
            ),
        )],
    );

    // The builder reads the file: the entry anchored to it is answered, and a file nothing is anchored to is not.
    let (status, _) = hub.guest(seat, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200);
    let (work, _) = first_work(&gateway, seat);
    let deadline = Instant::now() + WAIT;
    let answer = loop {
        let found: Value = serde_json::from_str(&tool(
            &hub,
            seat,
            work,
            "memory_anchored",
            json!({ "paths": [file] }),
        ))
        .expect("the answer is JSON");
        if found["entries"].as_array().is_some_and(|e| !e.is_empty()) || Instant::now() > deadline {
            break found;
        }
        thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        answer["entries"][0]["id"],
        "P-0002",
        "{answer}\n{}",
        hub.log()
    );
    let other = tool(
        &hub,
        seat,
        work,
        "memory_anchored",
        json!({ "paths": ["crates/apid/src/other.rs"] }),
    );
    assert!(other.contains("\"entries\":[]"), "{other}");

    // A landing that changes the file flags the entry; the verifier's session starts on the flag.
    land(
        &hub,
        &repo,
        "Release the claim first",
        &[(
            file.to_string(),
            "pub fn move_claimed() { release(); }\n".to_string(),
        )],
    );
    let flagged = memory_entry(&hub, &pid, "P-0002", |e| e["status"] == "flagged");
    assert_eq!(flagged["status"], "flagged", "{}", hub.log());

    let verifier = session_of(&hub, &pid, "verifier");
    let (status, _) = hub.guest(verifier, "hello", json!({ "role": "verifier", "vm": 1 }));
    assert_eq!(status, 200);
    let (verify_work, prompt) = first_work(&gateway, verifier);
    assert!(
        prompt.contains(&format!("- P-0002 pitfall (anchors: {file}):")),
        "{prompt}"
    );

    // Its answer confirms the entry: it is active again, checked against the code as it now is.
    let answered = tool(
        &hub,
        verifier,
        verify_work,
        "memory_verify",
        json!({ "verdicts": [{ "id": "P-0002", "action": "confirm" }] }),
    );
    assert!(answered.starts_with("Answered: 1 confirmed"), "{answered}");
    let confirmed = memory_entry(&hub, &pid, "P-0002", |e| e["status"] == "active");
    assert_eq!(confirmed["status"], "active", "{}", hub.log());
}
