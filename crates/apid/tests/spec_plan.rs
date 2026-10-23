//! Specs and plans through a booted hub (PLAN 21.2, 14.5; Engine 5.4), against a fake Linear GraphQL server that records
//! its calls. A Linear ticket above its project's threshold (its words, title and text together) starts a spec-and-plan
//! task on the `plan` recipe: its planner's session writes `.plaza/specs/<id>.md` on the task's lane in Weft. A spec's
//! question the project memory answers is answered from it; one it cannot answer is an owner thread, which the owner
//! answers through POST /v1/threads/{id}/answer. The plan step waits for that answer, reads it in its prompt, and the
//! planner posts the task list
//! with plan_post. The plan's tasks are created with their links and recipes, and the two that share a file are one
//! task. A ticket below the threshold becomes its task directly, on its rule's recipe.
//!
//! The sessions' VMs are played by this test as in `weft_review`: each takes the CID noded gives it, hellos, reads the
//! work the hub pushes, runs its `weft` commands through the agent gateway and makes its tool calls.

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use common::{FakeDaemon, Hub, Scratch, text};

/// The CID noded gives the first VM it starts; each next one gets the next.
const FIRST_CID: u64 = 500;
/// How long a step waits for the hub (scaled by the slack of a slow run).
const WAIT: Duration = Duration::from_secs(40);
/// The models the seats run on: the planner's, the builder's and the reviewer's.
const PLANNER_MODEL: &str = "mock/agent";
const BUILDER_MODEL: &str = "mock/agent";
const REVIEWER_MODEL: &str = "mock/echo";

const LINEAR_SIGNING: &str = "lin_hook_TESTSIGN_5401";
/// The Linear tickets: a long one (above the threshold), a short one (below it).
const BIG_ISSUE: &str = "c0ffee00-0000-4000-8000-000000000051";
const SMALL_ISSUE: &str = "c0ffee00-0000-4000-8000-000000000052";

/// A spec the planner writes: its four sections, with two open questions the project memory does not answer.
const SPEC: &str = "# Export the usage table as CSV\n\n## Behaviour\n\nThe usage table exports as CSV with the screen's filters.\n\n\
## Acceptance tests\n\n- The export matches the filters.\n- An empty table exports a header row.\n\n\
## Areas touched\n\n- crates/apid/src/export.rs\n- apps/web/src/usage.ts\n- docs/usage.md\n\n\
## Open questions\n\n- Which currency does the exported total use?\n- Does the export stream its rows?\n";

/// The VMs noded started, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// The roles the seats run: a planner, a builder and a reviewer.
fn roles() -> Value {
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["run", "run_tests", "read"] });
    json!({ "roles": {
        "planner": role("planner", "Planner"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } })
}

/// noded, which starts each session's VM (the next CID) and reports the VMs it started, and the agent gateway, which
/// takes the work the hub pushes to them.
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

/// The work items the hub pushed to `cid`'s VM, oldest first.
fn pushed(gateway: &FakeDaemon, cid: u64) -> Vec<Value> {
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, _)| {
            method == "POST" && path.ends_with(&format!("/agents/{cid}/send"))
        })
        .filter_map(|(_, _, body)| serde_json::from_str::<Value>(&body).ok())
        .map(|sent| sent["message"].clone())
        .filter(|message| message["type"] == "work")
        .collect()
}

/// The first work item pushed to `cid`'s VM: its session's step.
fn first_work(hub: &Hub, gateway: &FakeDaemon, cid: u64) -> Value {
    let deadline = Instant::now() + WAIT.mul_f64(common::slack());
    loop {
        if let Some(work) = pushed(gateway, cid).into_iter().next() {
            return work;
        }
        assert!(
            Instant::now() < deadline,
            "no work reached agent {cid}\n{}",
            hub.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The session of `seat` on `task` that runs on a VM `known` does not have.
fn next_session(hub: &Hub, task: &str, seat: &str, known: &HashSet<u64>) -> Value {
    let fresh = |s: &Value| {
        s["seat"] == seat
            && matches!(s["state"].as_str(), Some("forking" | "running"))
            && s["cid"].as_u64().is_some_and(|cid| !known.contains(&cid))
    };
    let list = hub.wait_for(&format!("/v1/sessions?task={task}"), WAIT, |list| {
        list.as_array().is_some_and(|l| l.iter().any(&fresh))
    });
    list.as_array()
        .and_then(|l| l.iter().find(|s| fresh(s)).cloned())
        .unwrap_or_else(|| {
            panic!(
                "no new {seat} session for task {task}: {list}\n{}",
                hub.log()
            )
        })
}

/// The VM says hello: its session runs, and its step's work goes out.
fn hello(hub: &Hub, cid: u64, role: &str) {
    let (status, body) = hub.guest(cid, "hello", json!({ "role": role, "vm": 0 }));
    assert_eq!(status, 200, "{body}");
}

/// A session's VM: its workspace and the `weft` commands its harness runs, through the agent gateway.
struct Vm<'a> {
    hub: &'a Hub,
    cid: u64,
    home: PathBuf,
    token: PathBuf,
}

fn vm_of<'a>(hub: &'a Hub, cid: u64, home: &Path, token: &Path) -> Vm<'a> {
    Vm {
        hub,
        cid,
        home: home.join(format!("vm{cid}")),
        token: token.to_path_buf(),
    }
}

impl Vm<'_> {
    fn weft(&self, cwd: &Path, args: &[&str]) {
        let env = std::collections::BTreeMap::from([
            (
                "WEFT_HUB".to_string(),
                format!(
                    "http://127.0.0.1:{}/internal/v1/agents/{}/gateway/weft",
                    self.hub.port, self.cid
                ),
            ),
            (
                "WEFT_TOKEN_FILE".to_string(),
                self.token.display().to_string(),
            ),
            (
                "WEFT_CACHE".to_string(),
                self.home.join(".cache").display().to_string(),
            ),
        ]);
        weft::cli::run_in(
            args.iter().map(|s| s.to_string()).collect(),
            cwd.to_path_buf(),
            env,
        )
        .unwrap_or_else(|e| {
            panic!(
                "agent {}: weft {}: {e}\n{}",
                self.cid,
                args.join(" "),
                self.hub.log()
            )
        });
    }

    /// Checks `change` out in this VM's own workspace, `repo` under its home.
    fn checkout(&self, change: &str, repo: &str) -> PathBuf {
        let ws = self.home.join(repo);
        self.weft(
            &self.home,
            &["checkout", change, "--dir", ws.to_str().unwrap()],
        );
        ws
    }

    /// A tool call the model makes in work item `work`, which must succeed: the hub's decision.
    fn tool(&self, work: u64, name: &str, input: Value) -> Value {
        let message = json!({ "type": "tool_request", "work": work, "call": format!("toolu_{name}_{work}"), "name": name, "input": input });
        let (status, body) = self.hub.guest(self.cid, "message", message);
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body["reply"]["is_error"], false,
            "{name} for agent {}: {body}",
            self.cid
        );
        body["reply"].clone()
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

/// The path of the spec a work prompt names (`.plaza/specs/SP-0001.md`).
fn spec_path_in(prompt: &str) -> String {
    let start = prompt
        .find(".plaza/specs/")
        .expect("the prompt names the spec's file");
    let rest = &prompt[start..];
    let end = rest.find(".md").expect("the spec's file is a .md") + 3;
    rest[..end].to_string()
}

/// A hex HMAC-SHA256 of `body` under `secret`, as the outside trackers sign their webhooks.
fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A signed POST to a public webhook route, with its headers.
fn post_signed(hub: &Hub, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, Value) {
    let reply = apz_http::client::send_with(
        hub.port,
        "POST",
        path,
        headers,
        Some(body),
        Duration::from_secs(60),
    )
    .unwrap_or_else(|e| panic!("POST {path}: {e}\n{}", hub.log()));
    let value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    (reply.status, value)
}

/// The Linear delivery for `issue` (an issue created) to the connector's hook at `path`, signed with the signing secret.
fn linear_hook(hub: &Hub, path: &str, issue: &str) -> (u16, Value) {
    let body = json!({
        "action": "create", "type": "Issue",
        "data": { "id": issue, "identifier": linear_issue(issue)["identifier"] },
        "webhookTimestamp": now_ms(), "createdAt": "2026-10-23T09:00:00.000Z"
    })
    .to_string();
    let signature = sign(LINEAR_SIGNING, body.as_bytes());
    post_signed(
        hub,
        path,
        &[("linear-signature", &signature)],
        body.as_bytes(),
    )
}

/// A Linear issue as its GraphQL API answers it. The long ticket's text is well above a project's threshold.
fn linear_issue(id: &str) -> Value {
    let (identifier, title, body) = match id {
        BIG_ISSUE => (
            "ENG-30",
            "Export the usage table as CSV",
            "The usage console has a table of runs, with filters for the team, the model and the date range. People \
             want to take the same table out of the console. Export it as CSV with exactly the filters the screen \
             shows: an empty table still exports a header row, and a large table streams its rows rather than \
             building the whole file in memory. Add a button to the usage console next to the filters, a route that \
             serves the file, a test that checks the filters carry over, and a short note in the usage guide that \
             says what each column means and how the totals are computed for each currency. The finance team \
             takes the export every month, so the column order stays the same from one release to the next, the \
             dates are written in ISO form whatever the locale, and a row a filter removes does not appear even in \
             part. The route checks the same permission the screen checks before it writes a single row, and a \
             refused request writes nothing at all.",
        ),
        _ => (
            "ENG-31",
            "Fix the empty export header",
            "The export has no header row when the report is empty.",
        ),
    };
    json!({
        "id": id, "identifier": identifier, "title": title, "description": body,
        "url": format!("https://linear.app/acme/issue/{identifier}"),
        "creator": { "name": "Ada Reporter" },
        "team": { "key": "ENG", "states": { "nodes": [
            { "id": "st-todo", "name": "Todo", "type": "unstarted" },
            { "id": "st-prog", "name": "In Progress", "type": "started" },
            { "id": "st-done", "name": "Done", "type": "completed" }
        ] } },
        "labels": { "nodes": [{ "name": "plaza" }] }
    })
}

/// The fake Linear GraphQL API: the issue reads, the state moves and the comments it is sent.
fn linear_answer(method: &str, path: &str, body: &str) -> (u16, String) {
    if method != "POST" || path != "/graphql" {
        return (404, json!({ "message": "not found" }).to_string());
    }
    let request: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let query = request["query"].as_str().unwrap_or("");
    let data = if query.contains("PlazaIssue") {
        let id = request["variables"]["id"].as_str().unwrap_or("");
        json!({ "issue": linear_issue(id) })
    } else if query.contains("PlazaState") {
        json!({ "issueUpdate": { "success": true } })
    } else if query.contains("PlazaComment") {
        json!({ "commentCreate": { "success": true } })
    } else {
        return (
            400,
            json!({ "errors": [{ "message": "unknown operation" }] }).to_string(),
        );
    };
    (200, json!({ "data": data }).to_string())
}

/// The intake item a source named, from the owner's list.
fn item_of(hub: &Hub, external: &str) -> Value {
    let (_, list) = hub.call("GET", "/v1/intake", None);
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["source"] == "linear" && i["external_id"] == external)
        .cloned()
        .unwrap_or_else(|| panic!("no intake item {external}: {list}"))
}

/// A memory entry as the project memory reads it: its front matter and its title.
fn memory_entry(id: &str, kind: &str, title: &str) -> String {
    format!("---\nid: {id}\ntype: {kind}\nstatus: active\n---\n{title}\n")
}

/// Lands `files` on the project's main as one change, through the hub's weft route.
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

/// The messages of the task's chat, as one text.
fn task_chat_says(hub: &Hub, task: &str) -> String {
    let (_, chats) = hub.call("GET", "/v1/chats", None);
    let Some(chat) = chats
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["task"] == task)
    else {
        return String::new();
    };
    let id = text(&chat["id"]);
    let (_, messages) = hub.call("GET", &format!("/v1/chats/{id}/messages"), None);
    messages.to_string()
}

#[test]
fn a_ticket_above_the_threshold_becomes_a_spec_and_a_plan_and_one_below_becomes_a_task() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let scratch = Scratch::new("spec-plan");
    let hub = Hub::start_with_roles(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles()),
    );
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    let token = scratch.0.join("internal.token");
    std::fs::write(&token, hub.token()).unwrap();
    let home = scratch.0.join("vms");

    // The stand-in for Linear, the connector's hook (its signing secret), and its connection with the key in the vault.
    let linear = FakeDaemon::serve(linear_answer);
    let linear_url = format!("http://127.0.0.1:{}/graphql", linear.port);
    let (status, hook) = hub.call(
        "POST",
        "/v1/connectors/linear/webhook",
        Some(json!({ "secret": LINEAR_SIGNING })),
    );
    assert_eq!(status, 201, "{hook}\n{}", hub.log());
    let linear_path = text(&hook["path"]);
    let (status, view) = hub.call(
        "PUT",
        "/v1/intake/connections/linear",
        Some(json!({ "enabled": true, "base_url": linear_url, "secrets": { "LINEAR_API_KEY": "lin_api_TESTKEY_5401" } })),
    );
    assert_eq!(status, 200, "{view}\n{}", hub.log());

    // A code project: its planner, builder and reviewer on models of their own, and its Linear rule (team ENG).
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Usage", "goal": "The usage console", "kind": "code" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({
            "role_models": { "planner": PLANNER_MODEL, "builder": BUILDER_MODEL, "reviewer": REVIEWER_MODEL },
            "intake": [{ "source": "linear", "field": "team", "value": "ENG", "recipe": "change" }]
        })),
    );
    assert_eq!(status, 200, "{settings}\n{}", hub.log());

    // The project's memory on main: an active decision that answers the spec's second question.
    let project = hub.wait_for(&format!("/v1/projects/{pid}"), WAIT, |p| {
        p["repo"].is_string()
    });
    let repo = text(&project["repo"]);
    land(
        &hub,
        &repo,
        "Record how the export streams",
        &[(
            ".plaza/memory/decisions/D-0001-export-streams-rows.md".to_string(),
            memory_entry("D-0001", "decision", "Export streams its rows."),
        )],
    );
    hub.wait_for(&format!("/v1/projects/{pid}/memory"), WAIT, |m| {
        m.to_string().contains("D-0001")
    });

    // ── The long ticket: above the threshold, so a spec-and-plan task on the plan recipe, not a task of its own.
    let (status, started) = linear_hook(&hub, &linear_path, BIG_ISSUE);
    assert_eq!(status, 202, "{started}\n{}", hub.log());
    assert_eq!(started["intake"]["status"], "started", "{started}");
    let big = item_of(&hub, BIG_ISSUE);
    assert_eq!(big["recipe"], "plan", "{big}");
    let spec_task = text(&big["task"]);
    // The work loop starts the task's run on its next tick (its seats open then).
    let runs = hub.wait_for(&format!("/v1/recipe-runs?task={spec_task}"), WAIT, |r| {
        r.as_array().is_some_and(|l| !l.is_empty())
    });
    assert!(
        runs.to_string().contains("\"plan\""),
        "the spec-and-plan task runs the plan recipe: {runs}"
    );
    let (_, intake_runs) = hub.call("GET", &format!("/v1/recipe-runs?project={pid}"), None);
    assert!(
        intake_runs.to_string().contains("\"intake\""),
        "the intake recipe classed the item: {intake_runs}"
    );

    // The planner writes the spec on the task's lane (its checkout, snapshotted as a draft), then the step passes.
    let mut known = HashSet::new();
    let spec_session = next_session(&hub, &spec_task, "spec", &known);
    let spec_cid = spec_session["cid"].as_u64().expect("its VM");
    known.insert(spec_cid);
    assert_eq!(spec_session["model"], PLANNER_MODEL, "{spec_session}");
    hello(&hub, spec_cid, "planner");
    let work = first_work(&hub, &gateway, spec_cid);
    let work_id = work["work"].as_u64().expect("a work id");
    let change = text(&work["lane"]["change"]);
    let repo = text(&work["lane"]["repo"]);
    let spec_path = spec_path_in(&text(&work["prompt"]));
    let spec_vm = vm_of(&hub, spec_cid, &home, &token);
    let ws = spec_vm.checkout(&change, &repo);
    spec_vm.write(&format!("{repo}/{spec_path}"), SPEC);
    spec_vm.weft(
        &ws,
        &[
            "snapshot",
            "--turn",
            "1",
            "--tool-call",
            "toolu_spec",
            "--model",
            PLANNER_MODEL,
        ],
    );
    hub.work_done(spec_cid, work_id, "done", "The spec is written.");

    // The spec's questions: the memory answers the second; the first went to the owner as a thread.
    let spec = hub.wait_for(&format!("/v1/tasks/{spec_task}/spec"), WAIT, |s| {
        s["state"] == "ready"
    });
    let questions = spec["questions"].as_array().cloned().unwrap_or_default();
    assert_eq!(questions.len(), 2, "{spec}");
    assert_eq!(questions[1]["state"], "memory", "{spec}");
    assert_eq!(questions[1]["answer"], "D-0001", "{spec}");
    assert_eq!(questions[0]["state"], "owner", "{spec}");
    let thread = text(&questions[0]["thread"]);
    assert!(!thread.is_empty(), "{spec}");
    let (_, threads) = hub.call("GET", &format!("/v1/tasks/{spec_task}/threads"), None);
    let asked = threads
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["id"] == thread.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("no thread {thread}: {threads}"));
    assert_eq!(asked["messages"][0]["to_seat"], "owner", "{asked}");
    assert!(
        text(&asked["messages"][0]["text"]).contains("Which currency does the exported total use?"),
        "{asked}"
    );
    let chat = task_chat_says(&hub, &spec_task);
    assert!(
        chat.contains(&format!("asks the owner in thread {thread}")),
        "the owner was asked: {chat}\n{}",
        hub.log()
    );

    // The plan step waits for the owner: no session of it starts before the answer.
    std::thread::sleep(Duration::from_secs(2));
    let (_, sessions) = hub.call("GET", &format!("/v1/sessions?task={spec_task}"), None);
    assert!(
        !sessions.to_string().contains("\"seat\":\"tasks\""),
        "the plan step waits for the answer: {sessions}"
    );

    // The owner answers through the route: the spec's question is answered, a second answer is refused.
    let answer = "Dollars: the total is in the account's currency.";
    let (status, reply) = hub.call(
        "POST",
        &format!("/v1/threads/{thread}/answer"),
        Some(json!({ "text": answer })),
    );
    assert_eq!(status, 200, "{reply}\n{}", hub.log());
    assert_eq!(reply["spec"], spec["id"], "{reply}");
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/threads/{thread}/answer"),
        Some(json!({ "text": "Again." })),
    );
    assert_eq!(status, 409, "a question is answered once");
    let spec = hub.wait_for(&format!("/v1/tasks/{spec_task}/spec"), WAIT, |s| {
        s["questions"][0]["state"] == "answered"
    });
    assert_eq!(spec["questions"][0]["answer"], answer, "{spec}");

    // The planner's second step posts the plan, and its prompt carries the owner's answer.
    let plan_session = next_session(&hub, &spec_task, "tasks", &known);
    let plan_cid = plan_session["cid"].as_u64().expect("its VM");
    known.insert(plan_cid);
    hello(&hub, plan_cid, "planner");
    let plan_work = first_work(&hub, &gateway, plan_cid);
    assert!(
        text(&plan_work["prompt"]).contains(answer),
        "the plan step's prompt carries the answer: {plan_work}"
    );
    let plan_work_id = plan_work["work"].as_u64().expect("a work id");
    let plan_vm = vm_of(&hub, plan_cid, &home, &token);
    let reply = plan_vm.tool(
        plan_work_id,
        "plan_post",
        json!({ "tasks": [
            { "key": "1.1", "goal": "Add the CSV export route", "files": ["crates/apid/src/export.rs"], "budget": 3000000 },
            { "key": "1.2", "goal": "Add the export button to the usage console", "files": ["apps/web/src/usage.ts"] },
            { "key": "1.3", "goal": "Document the export", "files": ["docs/usage.md"], "after": ["1.1", "1.2"] },
            { "key": "1.4", "goal": "Test the export route", "files": ["crates/apid/src/export.rs"], "after": ["1.1"] }
        ] }),
    );
    assert!(
        reply.to_string().contains("Posted 3 tasks"),
        "three tasks after the merge: {reply}"
    );
    hub.work_done(plan_cid, plan_work_id, "done", "The plan is posted.");

    // The spec-and-plan task is done, and its three planned tasks exist with their links.
    let done = hub.wait_for(&format!("/v1/tasks/{spec_task}"), WAIT, |t| {
        t["state"] == "done"
    });
    assert_eq!(done["state"], "done", "{done}\n{}", hub.log());
    let (_, list) = hub.call("GET", "/v1/tasks", None);
    let planned: Vec<Value> = list["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["project"] == pid.as_str() && t["id"] != spec_task.as_str())
        .cloned()
        .collect();
    assert_eq!(planned.len(), 3, "{list}\n{}", hub.log());
    let goal_of = |needle: &str| {
        planned
            .iter()
            .find(|t| text(&t["goal"]).contains(needle))
            .cloned()
            .unwrap_or_else(|| panic!("no planned task about {needle}: {list}"))
    };
    let csv = goal_of("Add the CSV export route");
    assert!(
        text(&csv["goal"]).contains("Test the export route"),
        "1.4 is merged into 1.1: {csv}"
    );
    let button = goal_of("Add the export button");
    let docs = goal_of("Document the export");
    let after_docs = docs["after"].as_array().cloned().unwrap_or_default();
    assert!(
        after_docs.contains(&csv["id"]) && after_docs.contains(&button["id"]),
        "1.3 runs after 1.1 and 1.2: {docs}"
    );
    for planned_task in [&csv, &button] {
        let id = text(&planned_task["id"]);
        let run = hub.wait_for(&format!("/v1/recipe-runs?task={id}"), WAIT, |r| {
            r.as_array().is_some_and(|l| !l.is_empty())
        });
        assert!(
            run.to_string().contains("\"change\""),
            "a planned task runs the change recipe: {run}"
        );
    }

    // ── The short ticket: below the threshold, so a task of its own on the rule's recipe.
    let (status, started) = linear_hook(&hub, &linear_path, SMALL_ISSUE);
    assert_eq!(status, 202, "{started}\n{}", hub.log());
    assert_eq!(started["intake"]["status"], "started", "{started}");
    let small = item_of(&hub, SMALL_ISSUE);
    assert_eq!(small["recipe"], "change", "{small}");
    let small_task = text(&small["task"]);
    let task = hub.wait_for(&format!("/v1/tasks/{small_task}"), WAIT, |t| {
        t["id"] == small_task.as_str()
    });
    assert!(
        text(&task["goal"]).contains("ENG-31"),
        "the short ticket is its task: {task}"
    );
    let run = hub.wait_for(&format!("/v1/recipe-runs?task={small_task}"), WAIT, |r| {
        r.as_array().is_some_and(|l| !l.is_empty())
    });
    assert!(
        run.to_string().contains("\"change\""),
        "the short ticket runs the change recipe: {run}"
    );
}
