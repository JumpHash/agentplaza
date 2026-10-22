//! The hub's internal API on a scratch hub, as the agent gateway (vsockd) calls it for each agent VM: hello, the guest
//! messages, gone, the egress report, and the egress proxy's questions. Each test starts its own hub; the VM's calls
//! carry the internal token, as the gateway sends them, and the owner's calls are plain.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// The CID noded gives the planner VM a project's run starts.
const PLANNER: u64 = 600;
const WAIT: Duration = Duration::from_secs(20);

/// Agent `cid`'s VM says hello as the builder on VM 0, and the hub registers it.
fn hello(hub: &Hub, cid: u64) {
    let (status, body) = hub.guest(cid, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200, "{body}");
    assert!(body["profile"].is_object(), "{body}");
}

/// Agent `cid`'s VM sends one guest message; the hub answers 200 with its reply (null when it has none).
fn message(hub: &Hub, cid: u64, body: Value) -> Value {
    let (status, reply) = hub.guest(cid, "message", body);
    assert_eq!(status, 200, "{reply}");
    reply
}

/// noded and the agent gateway, sharing the VMs noded started: each run's VM is [`PLANNER`], and the gateway answers
/// the status poll with the VMs started so far.
fn daemons() -> (FakeDaemon, FakeDaemon) {
    let started: Arc<Mutex<Vec<Value>>> = Arc::default();
    let spawned = Arc::clone(&started);
    let noded = FakeDaemon::serve(move |method, path, body| {
        let mut vms = spawned.lock().unwrap_or_else(|p| p.into_inner());
        if method == "GET" && path.starts_with("/v1/runs") {
            let runs = json!({ "runs": [{ "id": "r1", "ended_at": null, "vms": vms.clone() }] });
            (200, runs.to_string())
        } else if method == "POST" && path == "/v1/runs" {
            let role = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|b| b["role"].as_str().map(str::to_string))
                .unwrap_or_default();
            let id = vms.len();
            vms.push(json!({ "id": id, "guest_cid": PLANNER, "role": role }));
            let vm = json!({ "id": id, "guest_cid": PLANNER, "state": "running" });
            (200, json!({ "id": "r1", "vms": [vm] }).to_string())
        } else {
            (200, "{}".into())
        }
    });
    let reported = Arc::clone(&started);
    let gateway = FakeDaemon::serve(move |_, path, _| {
        if path.starts_with("/v1/status") {
            let vms = reported.lock().unwrap_or_else(|p| p.into_inner());
            let agents: Vec<Value> = vms
                .iter()
                .map(|vm| json!({ "cid": vm["guest_cid"], "role": vm["role"] }))
                .collect();
            (200, json!({ "agents": agents }).to_string())
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    });
    (noded, gateway)
}

#[test]
fn hello_then_gone() {
    let scratch = Scratch::new("internal-hello-gone");
    let hub = Hub::start(&scratch, None, None);
    let cid = 410;

    hello(&hub, cid);
    let (status, agent) = hub.call("GET", &format!("/v1/agents/{cid}"), None);
    assert_eq!(status, 200, "{agent}");
    assert_eq!(agent["vm"], 0, "{agent}");
    assert_eq!(agent["status"], "idle", "{agent}");

    // The VM ends its session: the agent is offline, and the hub says so in its view.
    let (status, body) = hub.guest(cid, "gone", json!({ "reason": "session closed" }));
    assert_eq!(status, 200, "{body}");
    let (status, agent) = hub.call("GET", &format!("/v1/agents/{cid}"), None);
    assert_eq!(status, 200, "{agent}");
    assert_eq!(agent["status"], "offline", "{agent}");
}

#[test]
fn a_message_from_an_agent_that_never_said_hello_is_refused() {
    let scratch = Scratch::new("internal-no-hello");
    let hub = Hub::start(&scratch, None, None);
    let cid = 420;

    // The hub answers 200 and refuses in the reply, not with an HTTP error: a model call gets a failed model result
    // and a tool request a denied decision, both saying "unknown agent".
    let reply = message(
        &hub,
        cid,
        json!({ "type": "model_call", "req": 1, "work": 1, "request": { "model": "", "messages": [] } }),
    );
    assert_eq!(reply["reply"]["type"], "model_result", "{reply}");
    assert_eq!(reply["reply"]["ok"], false, "{reply}");
    assert_eq!(reply["reply"]["error"], "unknown agent", "{reply}");

    let reply = message(
        &hub,
        cid,
        json!({ "type": "tool_request", "work": 1, "call": "toolu_0", "name": "run", "input": {} }),
    );
    assert_eq!(reply["reply"]["type"], "tool_decision", "{reply}");
    assert_eq!(reply["reply"]["decision"], "deny", "{reply}");
    assert_eq!(reply["reply"]["output"], "unknown agent", "{reply}");

    // A message is not a hello: the VM never became an agent.
    let (status, body) = hub.call("GET", &format!("/v1/agents/{cid}"), None);
    assert_eq!(status, 404, "{body}");
}

#[test]
fn each_guest_message_type_is_answered() {
    let (noded, gateway) = daemons();
    let roles = json!({ "roles": {
        "planner": { "role": "planner", "title": "Planner", "tools": ["run", "read"] },
    } });
    let scratch = Scratch::new("internal-messages");
    let hub = Hub::start_with_roles(&scratch, Some(gateway.port), Some(noded.port), Some(&roles));
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Messages", "goal": "Answer each guest message" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    // The project's planner: a long-lived session on the VM noded started for it, which the work loop starts on a tick.
    let planners = hub.wait_for(&format!("/v1/sessions?project={pid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["role"] == "planner" && s["cid"] == PLANNER)
        })
    });
    assert!(
        planners
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["role"] == "planner")),
        "a planner session: {planners}\n{}",
        hub.log()
    );
    let (status, body) = hub.guest(PLANNER, "hello", json!({ "role": "planner", "vm": 0 }));
    assert_eq!(status, 200, "{body}");

    // Its project paused, the session's model call and tool request are held: each reply comes back in its own type.
    let (status, paused) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200, "{paused}");
    let reply = message(
        &hub,
        PLANNER,
        json!({ "type": "model_call", "req": 7, "work": 1, "request": { "model": "", "messages": [] } }),
    );
    assert_eq!(reply["reply"]["type"], "model_result", "{reply}");
    assert_eq!(reply["reply"]["req"], 7, "{reply}");
    assert_eq!(reply["reply"]["ok"], false, "{reply}");
    assert!(text(&reply["reply"]["error"]).contains("paused"), "{reply}");

    let reply = message(
        &hub,
        PLANNER,
        json!({ "type": "tool_request", "work": 1, "call": "toolu_2", "name": "run", "input": {} }),
    );
    assert_eq!(reply["reply"]["type"], "tool_decision", "{reply}");
    assert_eq!(reply["reply"]["call"], "toolu_2", "{reply}");
    assert_eq!(reply["reply"]["decision"], "deny", "{reply}");
    assert!(
        text(&reply["reply"]["output"]).contains("paused"),
        "{reply}"
    );

    // The rest have no reply for the VM: each is accepted (200) and its reply is null.
    let others = [
        json!({ "type": "tool_report", "work": 1, "call": "toolu_2", "ok": true, "output": "done" }),
        json!({
            "type": "engine_update",
            "work": 1,
            "call": "toolu_3",
            "engine": "acp:test",
            "session": "s1",
            "seq": 1,
            "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "hi" } },
        }),
        json!({ "type": "compressed", "work": 1, "before": 4000, "after": 1000, "folded": 3, "reason": "auto" }),
        json!({ "type": "stats", "workspace_used_mib": 10, "workspace_size_mib": 100, "top": [["cargo", 12.5, 300.0]] }),
        json!({ "type": "work_done", "work": 1, "status": "done", "summary": "Checked the gate" }),
    ];
    for body in others {
        let kind = body["type"].clone();
        let reply = message(&hub, PLANNER, body);
        assert!(reply["reply"].is_null(), "{kind}: {reply}");
    }
}

#[test]
fn stats_are_accepted_on_a_running_vm() {
    // noded reports one builder VM, 300 on run r1, and the gateway reports the same agent; the hub gives the agent
    // its run from them, and a stats report needs a run to land on.
    let noded = FakeDaemon::serve(|method, path, _| {
        if method == "GET" && path.starts_with("/v1/runs") {
            let runs = json!({ "runs": [{ "id": "r1", "ended_at": null, "vms": [{ "id": 0, "guest_cid": 300, "role": "builder" }] }] });
            (200, runs.to_string())
        } else {
            (200, "{}".into())
        }
    });
    let gateway = FakeDaemon::serve(|_, path, _| {
        if path.starts_with("/v1/status") {
            (
                200,
                json!({ "agents": [{ "cid": 300, "role": "builder" }] }).to_string(),
            )
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    });
    let scratch = Scratch::new("internal-stats");
    let hub = Hub::start(&scratch, Some(gateway.port), Some(noded.port));
    let agent = hub.wait_for("/v1/agents/300", Duration::from_secs(20), |a| {
        a["run"] == "r1"
    });
    assert_eq!(agent["run"], "r1", "{agent}\n{}", hub.log());
    hello(&hub, 300);

    // The hub accepts the VM's stats (200, no reply). The gauges it records (`vm.workspace_used_mib` under `r1/0`)
    // are not read here: the metrics overview this test can reach shows no VM series right after the report, so
    // that check is left open (see the report).
    let reply = message(
        &hub,
        300,
        json!({ "type": "stats", "workspace_used_mib": 42, "workspace_size_mib": 512, "top": [["cargo", 80.0, 900.0]] }),
    );
    assert!(reply["reply"].is_null(), "{reply}");
}

#[test]
fn the_egress_report_and_check_need_the_token() {
    let scratch = Scratch::new("internal-egress");
    let hub = Hub::start(&scratch, None, None);
    let cid = 440;
    hello(&hub, cid);

    // The proxy's question and the VM's report are refused without the internal token: the owner's plain call has none.
    let check = json!({ "cid": cid, "host": "example.com", "port": 443 });
    let (status, body) = hub.call("POST", "/internal/egress/check", Some(check.clone()));
    assert_eq!(status, 403, "{body}");
    assert!(body.get("allow").is_none(), "{body}");
    let report = json!({ "host": "example.com", "port": 443, "allowed": true, "bytes_up": 10, "bytes_down": 20, "ms": 5 });
    let (status, body) = hub.call(
        "POST",
        &format!("/internal/v1/agents/{cid}/egress"),
        Some(report.clone()),
    );
    assert_eq!(status, 403, "{body}");

    // With the token, the check answers with its allow field. The agent has no session, so no project holds it: the
    // laws decide for its host, and the answer is a decision either way.
    let (status, answer) = hub.internal("/internal/egress/check", check);
    assert_eq!(status, 200, "{answer}");
    assert!(answer["allow"].is_boolean(), "{answer}");
    assert!(!text(&answer["reason"]).contains("paused"), "{answer}");

    // With the token, the VM's report of its connection is recorded.
    let (status, body) = hub.guest(cid, "egress", report);
    assert_eq!(status, 200, "{body}");
}

#[test]
fn a_turn_is_one_batch_decided_as_each_of_its_calls_would_be() {
    let scratch = Scratch::new("internal-batch");
    let hub = Hub::start(&scratch, None, None);
    let cid = 440;
    hello(&hub, cid);

    // The five calls of one turn: decided one request at a time, then as one batch. The batch answers once, with the
    // same decision for each call, in the calls' order.
    let calls = [
        ("toolu_b1", "run", json!({"command": "ls"})),
        ("toolu_b2", "read", json!({"path": "a.txt"})),
        ("toolu_b3", "search", json!({"pattern": "x"})),
        ("toolu_b4", "plaza_tasks", json!({})),
        ("toolu_b5", "no_such_tool", json!({})),
    ];
    let one_by_one: Vec<Value> = calls
        .iter()
        .map(|(call, name, input)| {
            let reply = message(
                &hub,
                cid,
                json!({ "type": "tool_request", "work": 1, "call": call, "name": name, "input": input }),
            );
            reply["reply"].clone()
        })
        .collect();
    let batch: Vec<Value> = calls
        .iter()
        .map(|(call, name, input)| json!({ "call": call, "name": name, "input": input }))
        .collect();
    let reply = message(
        &hub,
        cid,
        json!({ "type": "tool_batch", "work": 1, "calls": batch }),
    );
    assert_eq!(reply["reply"]["type"], "tool_batch_decision", "{reply}");
    assert_eq!(reply["reply"]["work"], 1, "{reply}");
    let decisions = reply["reply"]["decisions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(decisions.len(), 5, "{reply}");
    for (decision, single) in decisions.iter().zip(&one_by_one) {
        assert_eq!(
            (
                decision["call"].clone(),
                decision["decision"].clone(),
                decision["output"].clone(),
                decision["is_error"].clone()
            ),
            (
                single["call"].clone(),
                single["decision"].clone(),
                single["output"].clone(),
                single["is_error"].clone()
            ),
            "{decision} is not {single}"
        );
    }
}

#[test]
fn a_session_starts_with_its_policy_and_a_report_outside_it_is_a_violation() {
    let scratch = Scratch::new("internal-policy");
    let hub = Hub::start(&scratch, None, None);
    let cid = 450;

    // The hello carries the agent's policy, signed by the hub: the guest tools its grant covers that read no input the
    // laws test run in the VM on their own; a shell command does not.
    let (status, body) = hub.guest(cid, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200, "{body}");
    let policy = body["policy"].clone();
    let allowed = |tool: &str| {
        policy["allow"]
            .as_array()
            .is_some_and(|allow| allow.iter().any(|t| t == tool))
    };
    assert!(allowed("read") && !allowed("run"), "{body}");
    assert_eq!(text(&policy["id"]).len(), 64, "{body}");
    assert!(!text(&policy["signature"]).is_empty(), "{body}");
    let id = text(&policy["id"]);

    // A call the VM reports under that policy, for a tool the policy does not allow, is recorded with the policy it
    // was decided under and as a violation. The call stands: the report is answered with nothing.
    let reply = message(
        &hub,
        cid,
        json!({
            "type": "tool_report", "work": 1, "call": "toolu_p1", "ok": true, "output": "ran",
            "policy": id, "name": "run", "input": {"command": "ls"},
        }),
    );
    assert!(reply["reply"].is_null(), "{reply}");
    let (status, transcript) = hub.call("GET", &format!("/v1/agents/{cid}/transcript"), None);
    assert_eq!(status, 200, "{transcript}");
    let mentions = |transcript: &Value, needle: &str| {
        transcript
            .as_array()
            .is_some_and(|entries| entries.iter().any(|e| e.to_string().contains(needle)))
    };
    assert!(
        mentions(&transcript, &format!("allowed by policy {id}")),
        "{transcript}"
    );
    assert!(
        mentions(&transcript, "policy violation: run ran under policy"),
        "{transcript}"
    );

    // A report under a policy the hub never issued to this agent is a violation as well.
    message(
        &hub,
        cid,
        json!({
            "type": "tool_report", "work": 1, "call": "toolu_p2", "ok": true, "output": "ran",
            "policy": "not-issued", "name": "glob", "input": {},
        }),
    );
    let (_, transcript) = hub.call("GET", &format!("/v1/agents/{cid}/transcript"), None);
    assert!(
        mentions(
            &transcript,
            "glob was decided under policy not-issued, which the hub never issued"
        ),
        "{transcript}"
    );
}
