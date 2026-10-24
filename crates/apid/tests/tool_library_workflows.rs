//! The ephemeral environments and the workflows as tool library entries on a booted hub: each entry's route is the one the
//! library answers a `tools_call` with (its `exec` names the route), the route answers for a project, the search finds the
//! entries, and the seats see an operator entry as the orchestrator's only. The calls themselves go through the VM's path:
//! a `tool_request` for `tools_call` is decided by the hub (the laws, the approvals), run as the host tool, and checked by
//! the entry's policy (`visible`) before its route answers, so a refusal's text tells which of the three decided it.

mod common;

use std::time::Duration;

use common::{FakeDaemon, Hub, Scratch, text};
use serde_json::{Value, json};

fn project(hub: &Hub) -> String {
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Shop", "goal": "Sell things" })),
    );
    assert_eq!(status, 201, "{project}");
    project["id"].as_str().expect("an id").to_string()
}

/// The route an entry answers, as `(method, pattern)`, from its `exec` label (`route GET /v1/...`).
fn route_of(entry: &Value) -> (String, String) {
    let exec = entry["exec"].as_str().expect("an exec label");
    let rest = exec.strip_prefix("route ").expect("a route entry");
    let (method, pattern) = rest.split_once(' ').expect("a method and a pattern");
    (method.to_string(), pattern.to_string())
}

fn seat<'a>(entry: &'a Value, role: &str) -> &'a Value {
    entry["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|seat| seat["seat"] == role)
        .unwrap_or_else(|| panic!("no {role} seat: {}", entry["seats"]))
}

#[test]
fn an_ephemeral_deploy_is_the_orchestrator_s_and_the_builder_is_told_why() {
    let scratch = Scratch::new("tool-library-ephemeral");
    let roles = json!({ "roles": {
        "builder": { "role": "builder", "title": "Builder", "tools": ["run", "read"] },
    } });
    let hub = Hub::start_with_roles(&scratch, None, None, Some(&roles));
    let (status, entry) = hub.call(
        "GET",
        "/v1/tool-library/entry?path=environments.ephemeral.up",
        None,
    );
    assert_eq!(status, 200, "{entry}");
    assert_eq!(entry["access"], "operator");
    assert_eq!(entry["runs"], "hub");
    assert_eq!(seat(&entry, "orchestrator")["allowed"], true, "{entry}");
    let builder = seat(&entry, "builder");
    assert_eq!(builder["allowed"], false, "{entry}");
    assert!(
        builder["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("orchestrator"),
        "{entry}"
    );
}

#[test]
fn the_ephemeral_and_workflow_entries_answer_on_a_project_through_their_routes() {
    let scratch = Scratch::new("tool-library-routes");
    let hub = Hub::start(&scratch, None, None);
    let pid = project(&hub);
    // The ephemeral list answers for any project. The workflow routes read the project's repo, which this scratch project
    // does not have, so they answer the route's own refusal: the entry reached the route, and the route decided.
    let (status, entry) = hub.call(
        "GET",
        "/v1/tool-library/entry?path=environments.ephemeral.list",
        None,
    );
    assert_eq!(status, 200, "{entry}");
    assert_eq!(entry["access"], "any");
    let (method, pattern) = route_of(&entry);
    assert_eq!(method, "GET");
    let (status, listed) = hub.call(&method, &pattern.replace("{id}", &pid), None);
    assert_eq!(status, 200, "{listed}");
    assert!(listed.is_array() || listed.is_object(), "{listed}");
    for path in ["workflows.list", "workflows.import.preview"] {
        let (status, entry) = hub.call("GET", &format!("/v1/tool-library/entry?path={path}"), None);
        assert_eq!(status, 200, "{entry}");
        assert_eq!(entry["access"], "any", "{path}");
        let (method, pattern) = route_of(&entry);
        assert_eq!(method, "GET", "{path}");
        let (status, answer) = hub.call(&method, &pattern.replace("{id}", &pid), None);
        assert_eq!(status, 409, "{path} answered {answer}");
        assert!(
            answer["error"]
                .as_str()
                .unwrap_or_default()
                .contains("repo"),
            "{path} answered {answer}"
        );
    }
}

#[test]
fn an_operator_workflow_run_is_refused_to_a_builder_in_the_entry_and_the_search_finds_both() {
    let scratch = Scratch::new("tool-library-workflow-run");
    let roles = json!({ "roles": {
        "builder": { "role": "builder", "title": "Builder", "tools": ["run", "read"] },
    } });
    let hub = Hub::start_with_roles(&scratch, None, None, Some(&roles));
    let (status, entry) = hub.call("GET", "/v1/tool-library/entry?path=workflows.run", None);
    assert_eq!(status, 200, "{entry}");
    assert_eq!(entry["approval"], true);
    assert_eq!(seat(&entry, "builder")["allowed"], false, "{entry}");
    assert_eq!(seat(&entry, "orchestrator")["allowed"], true, "{entry}");

    let (status, found) = hub.call("GET", "/v1/tool-library/search?q=ephemeral", None);
    assert_eq!(status, 200, "{found}");
    let paths: Vec<&str> = found["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["path"].as_str().unwrap_or_default())
        .collect();
    assert!(paths.contains(&"environments.ephemeral.up"), "{paths:?}");
    let (status, found) = hub.call("GET", "/v1/tool-library/search?q=workflow", None);
    assert_eq!(status, 200, "{found}");
    let paths: Vec<&str> = found["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["path"].as_str().unwrap_or_default())
        .collect();
    assert!(
        paths.iter().any(|p| p.starts_with("workflows.")),
        "{paths:?}"
    );
}

/// noded, reporting one run whose VMs are `agents` (cid and role): the hub learns each agent's role from these reports.
fn noded_with(agents: &[(u64, &str)]) -> FakeDaemon {
    let vms: Vec<Value> = agents
        .iter()
        .enumerate()
        .map(|(id, (cid, role))| json!({ "id": id, "guest_cid": cid, "role": role }))
        .collect();
    let runs = json!({ "runs": [{ "id": "r1", "ended_at": null, "vms": vms }] });
    FakeDaemon::serve(move |method, path, _| {
        if method == "GET" && path.starts_with("/v1/runs") {
            (200, runs.to_string())
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    })
}

/// Agent `cid`'s VM says hello, and the hub lists the agent with `role` (the report's role, adopted on a later pass).
fn hello_as(hub: &Hub, cid: u64, role: &str) {
    let (status, body) = hub.guest(cid, "hello", json!({ "role": role, "vm": 0 }));
    assert_eq!(status, 200, "{body}");
    assert!(body["profile"].is_object(), "{body}");
    let listed = hub.wait_for("/v1/agents", Duration::from_secs(30), |list| {
        list.as_array()
            .is_some_and(|list| list.iter().any(|a| a["cid"] == cid && a["role"] == role))
    });
    assert!(
        listed
            .as_array()
            .is_some_and(|list| list.iter().any(|a| a["cid"] == cid && a["role"] == role)),
        "agent {cid} is not {role}: {listed}"
    );
}

/// Agent `cid`'s VM asks the hub to run tool `name` with `input`: the decision, as the hub answers the VM's
/// `tool_request`.
fn request(hub: &Hub, cid: u64, call: &str, name: &str, input: Value) -> Value {
    let (status, body) = hub.guest(
        cid,
        "message",
        json!({ "type": "tool_request", "work": 1, "call": call, "name": name, "input": input }),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["reply"]["type"], "tool_decision", "{body}");
    body["reply"].clone()
}

/// Agent `cid`'s VM asks the hub to run the tool library's `tools_call` for `path` with `args`.
fn tools_call(hub: &Hub, cid: u64, call: &str, path: &str, args: Value) -> Value {
    request(
        hub,
        cid,
        call,
        "tools_call",
        json!({ "path": path, "args": args }),
    )
}

#[test]
fn tools_call_runs_a_library_entry_through_the_policy_path() {
    let scratch = Scratch::new("tool-library-call");
    let (orchestrator, builder) = (510, 511);
    let noded = noded_with(&[(orchestrator, "orchestrator"), (builder, "builder")]);
    let hub = Hub::start(&scratch, None, Some(noded.port));
    let pid = project(&hub);
    hello_as(&hub, orchestrator, "orchestrator");
    hello_as(&hub, builder, "builder");

    // The browse tools run for both seats (T-002b): the tree answers, and the search and describe follow the seat's policy.
    for (cid, seat) in [(orchestrator, "orchestrator"), (builder, "builder")] {
        let listed = request(
            &hub,
            cid,
            "toolu_browse1",
            "tools_list",
            json!({ "path": "" }),
        );
        assert_eq!(listed["decision"], "done", "{seat}: {listed}");
        assert_eq!(listed["is_error"], false, "{seat}: {listed}");
        assert!(
            text(&listed["output"]).contains("environments"),
            "{seat}: {listed}"
        );

        let found = request(
            &hub,
            cid,
            "toolu_browse2",
            "tools_search",
            json!({ "query": "ephemeral" }),
        );
        assert_eq!(found["decision"], "done", "{seat}: {found}");
        assert_eq!(found["is_error"], false, "{seat}: {found}");
        assert!(
            text(&found["output"]).contains("environments.ephemeral.list"),
            "{seat}: {found}"
        );

        let described = request(
            &hub,
            cid,
            "toolu_browse3",
            "tools_describe",
            json!({ "path": "environments.ephemeral.up" }),
        );
        assert_eq!(described["decision"], "done", "{seat}: {described}");
        assert_eq!(described["is_error"], false, "{seat}: {described}");
        let entry: Value =
            serde_json::from_str(&text(&described["output"])).expect("the entry's JSON");
        // The orchestrator may use the entry; a builder is told it is the orchestrator's.
        assert_eq!(
            entry["available"],
            seat == "orchestrator",
            "{seat}: {entry}"
        );
    }

    // An open entry runs for the orchestrator: its route answers the project's ephemeral instances (possibly none).
    let listed = tools_call(
        &hub,
        orchestrator,
        "toolu_call1",
        "environments.ephemeral.list",
        json!({ "id": pid }),
    );
    assert_eq!(listed["decision"], "done", "{listed}");
    assert_eq!(listed["is_error"], false, "{listed}");
    let instances: Value =
        serde_json::from_str(&text(&listed["output"])).expect("the route's JSON");
    assert!(instances.is_array(), "{listed}");

    // An open entry runs for a builder too: the policy lets every seat see it.
    let builder_listed = tools_call(
        &hub,
        builder,
        "toolu_call2",
        "environments.ephemeral.list",
        json!({ "id": pid }),
    );
    assert_eq!(builder_listed["is_error"], false, "{builder_listed}");

    // The workflows route answers the project's refusal for a project with no repo: the entry ran and the route decided.
    let workflows = tools_call(
        &hub,
        orchestrator,
        "toolu_call3",
        "workflows.list",
        json!({ "id": pid }),
    );
    assert_eq!(workflows["is_error"], true, "{workflows}");
    let why = text(&workflows["output"]);
    assert!(why.starts_with("409:") && why.contains("repo"), "{why}");

    // An operator entry is refused to a builder by `visible`, before its route is reached: the text is the policy's.
    let refused = tools_call(
        &hub,
        builder,
        "toolu_call4",
        "environments.ephemeral.up",
        json!({ "id": pid }),
    );
    assert_eq!(refused["decision"], "done", "{refused}");
    assert_eq!(refused["is_error"], true, "{refused}");
    assert_eq!(
        text(&refused["output"]),
        "environments.ephemeral.up is the orchestrator's: the builder seat may not use it",
        "{refused}"
    );
}
