//! The hub's internal API on a scratch hub, as the agent gateway (vsockd) calls it for each agent VM: hello, the guest
//! messages, gone, the egress report, and the egress proxy's questions. Each test starts its own hub; the VM's calls
//! carry the internal token, as the gateway sends them, and the owner's calls are plain.

mod common;

use std::time::Duration;

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

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
        json!({ "type": "tool_request", "work": 1, "call": "toolu_0", "name": "bash", "input": {} }),
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
    let scratch = Scratch::new("internal-messages");
    let hub = Hub::start(&scratch, None, None);
    let cid = 430;
    hello(&hub, cid);

    // On the bench (no project), a model call and a tool request are refused before any provider or tool runs, so
    // nothing leaves the hub. Each reply still comes back in its own type.
    let reply = message(
        &hub,
        cid,
        json!({ "type": "model_call", "req": 7, "work": 1, "request": { "model": "", "messages": [] } }),
    );
    assert_eq!(reply["reply"]["type"], "model_result", "{reply}");
    assert_eq!(reply["reply"]["req"], 7, "{reply}");
    assert_eq!(reply["reply"]["ok"], false, "{reply}");
    assert!(
        text(&reply["reply"]["error"]).contains("on the bench"),
        "{reply}"
    );

    let reply = message(
        &hub,
        cid,
        json!({ "type": "tool_request", "work": 1, "call": "toolu_2", "name": "bash", "input": {} }),
    );
    assert_eq!(reply["reply"]["type"], "tool_decision", "{reply}");
    assert_eq!(reply["reply"]["call"], "toolu_2", "{reply}");
    assert_eq!(reply["reply"]["decision"], "deny", "{reply}");
    assert!(
        text(&reply["reply"]["output"]).contains("on the bench"),
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
        let reply = message(&hub, cid, body);
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

    // With the token, the check answers with its allow field. The agent is on the bench, so it reaches no host.
    let (status, answer) = hub.internal("/internal/egress/check", check);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["allow"], false, "{answer}");
    assert!(text(&answer["reason"]).contains("on the bench"), "{answer}");

    // With the token, the VM's report of its connection is recorded.
    let (status, body) = hub.guest(cid, "egress", report);
    assert_eq!(status, 200, "{body}");
}
