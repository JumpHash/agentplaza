//! A project's own recipe copy, read by the work loop on a scratch hub (PLAN 4.1): a project that keeps
//! `.plaza/recipes/change.toml` on main runs its tasks on that copy (the run and the seats say so), and a copy that does
//! not check is named in the log, and the built-in change recipe runs instead.
//!
//! noded and the agent gateway are stood in for, as in the other work-engine tests: the VMs are not played here, only
//! the task's run and seats are read.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(30);

/// The project's own copy of the change recipe: a named recipe, version 4, with a review that must also find nothing.
const OWN: &str = r#"
id = "change"
name = "Our change"
version = 4
scope = "project"

[[steps]]
id = "build"
seat = "builder"
role = "builder"
model = "any"

[[steps]]
id = "review"
seat = "reviewer"
role = "reviewer"
model = "differ"
gate = ["passed", "no findings"]

[[edges]]
from = "build"
to = "review"
when = "passed"

[[edges]]
from = "review"
to = "build"
when = "failed"
max = 1
"#;

/// A copy that does not check: a step with no role or engine.
const BROKEN: &str = r#"
id = "change"
version = 4
scope = "project"

[[steps]]
id = "build"
"#;

fn roles() -> Value {
    let role =
        |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "read"] });
    json!({ "roles": {
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
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

/// Lands an owner's edit of `files` (path, text) on the repo's main.
fn land(hub: &Hub, repo: &str, title: &str, files: &[(&str, &str)]) {
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

/// A code project with its repo, and a task of it created after `recipe` (if any) landed on the repo's main. Returns
/// the task's id.
fn task_after(hub: &Hub, recipe: Option<(&str, &str)>) -> String {
    let weft = hub.wait_for("/v1/weft", WAIT, |w| w["running"] == true);
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Recipes", "goal": "Its own recipe", "kind": "code" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let project = hub.wait_for(&format!("/v1/projects/{pid}"), WAIT, |p| {
        p["repo"].is_string()
    });
    let repo = text(&project["repo"]);
    eprintln!("recipes_project: repo {repo}");
    if let Some((path, body)) = recipe {
        land(hub, &repo, "The project's change recipe", &[(path, body)]);
    }
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Run on the project's recipe", "acceptance": "its recipe is the one run", "project": pid, "role": "builder" })),
    );
    assert_eq!(status, 201, "{task}");
    text(&task["id"])
}

#[test]
fn a_task_runs_the_recipe_its_project_keeps_on_main_with_its_own_seats() {
    let (noded, gateway) = daemons(900);
    let scratch = Scratch::new("recipes-project-own");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let tid = task_after(&hub, Some((".plaza/recipes/change.toml", OWN)));

    let runs = hub.wait_for(&format!("/v1/recipe-runs?task={tid}"), WAIT, |r| {
        r["runs"].as_array().is_some_and(|runs| !runs.is_empty())
    });
    let run = &runs["runs"][0];
    assert_eq!(run["recipe"], "change", "{runs}");
    assert_eq!(run["source"], "project", "{runs}");
    assert_eq!(run["version"], 4, "{runs}");
    let (status, detail) = hub.call(
        "GET",
        &format!("/v1/recipe-runs/{}", text(&run["run"])),
        None,
    );
    assert_eq!(status, 200, "{detail}");
    assert!(
        text(&detail["recipe_toml"]).contains("name = \"Our change\""),
        "the run keeps the copy it started on: {detail}"
    );

    let (status, seats) = hub.call("GET", &format!("/v1/tasks/{tid}/seats"), None);
    assert_eq!(status, 200, "{seats}");
    let names: Vec<&str> = seats
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"builder") && names.contains(&"reviewer"),
        "{seats}"
    );
    let reviewer = seats
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "reviewer")
        .unwrap();
    assert_eq!(reviewer["recipe"], "change", "{seats}");
    assert_eq!(reviewer["model_policy"], "differ", "{seats}");
}

#[test]
fn a_project_copy_that_does_not_check_is_named_and_the_built_in_recipe_runs() {
    let (noded, gateway) = daemons(950);
    let scratch = Scratch::new("recipes-project-broken");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let tid = task_after(&hub, Some((".plaza/recipes/change.toml", BROKEN)));

    let runs = hub.wait_for(&format!("/v1/recipe-runs?task={tid}"), WAIT, |r| {
        r["runs"].as_array().is_some_and(|runs| !runs.is_empty())
    });
    let run = &runs["runs"][0];
    assert_eq!(run["source"], "built_in", "{runs}");
    assert_eq!(run["version"], 2, "{runs}");
}
