//! Subscriptions on a scratch hub (Engine 13.3; PLAN 10.4, 11.2): a model the policy marks `subscription` draws on its
//! vendor's allowance. A fake CLIProxyAPI usage endpoint reports the window used up, with a reset a few seconds ahead:
//! the builder's call waits, its session pauses with an allowance card, and at the reset the ticker resumes the session
//! and the next call goes through. The login bridge's switch is off by default.
//!
//! The model is the harness's mock (`mock/agent`), and its vendor is the mock provider's name, so the fake usage
//! reports its window under `mock`. The reset is the one moment that matters here: the fake answers it from its first
//! reading on, so the test is about the wall clock of a few seconds, not a fake clock (the unit tests in
//! `allowances.rs` run the rules on a fake clock).
//!
//! noded and the agent gateway are stood in for, as in `work_engine.rs`. No real account, no sign-in.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const FIRST_CID: u64 = 500;
const WAIT: Duration = Duration::from_secs(20);

/// The default orchestrator list, as the policy starts with it, and the worker list with the mock agent added. The mock
/// echo comes first: it is the policy's fallback for a subscription model (`allowances::fallback`).
const ORCHESTRATOR: [&str; 4] = [
    "anthropic/claude-opus-5-5",
    "anthropic/claude-fable-5-1",
    "openai/gpt-6-astra",
    "xai/grok-4.7",
];
const WORKER: [&str; 8] = [
    "mock/echo",
    "deepseek/deepseek-flash",
    "anthropic/claude-haiku-5-5",
    "anthropic/claude-opus-5-5@low",
    "openai/gpt-6-luna",
    "openai/gpt-5.6-terra",
    "xai/grok-4.7@low",
    "mock/agent",
];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The fake CLIProxyAPI. Its usage answer carries the allowance windows inside the `usage` object, the object
/// `usage_providers.rs` reads the totals from (the shape `allowances::parse` reads, assumed: see its doc). The one
/// window is used up, and it resets `reset_in_ms` after the first reading (scaled by the slack of a slow run).
fn usage_hub(reset_in_ms: u64) -> FakeDaemon {
    let reset: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
    let reset_in = (reset_in_ms as f64 * common::slack()) as u64;
    FakeDaemon::serve(move |method, path, _| {
        if method != "GET" {
            return (405, "{}".into());
        }
        match path {
            "/v0/management/usage" => {
                let mut reset = reset.lock().unwrap_or_else(|p| p.into_inner());
                let resets_at = *reset.get_or_insert_with(|| now_ms() + reset_in);
                (
                    200,
                    json!({
                        "usage": {
                            "total_requests": 40, "success_count": 40, "failure_count": 0,
                            "total_tokens": 9000, "apis": {},
                            "allowances": [{
                                "provider": "mock", "account": "owner@example.com", "window": "5h",
                                "used": 40, "limit": 40, "resets_at": resets_at
                            }]
                        }
                    })
                    .to_string(),
                )
            }
            "/v0/management/auth-files" => (200, json!({ "files": [] }).to_string()),
            _ => (404, "{}".into()),
        }
    })
}

/// noded and the agent gateway, sharing the VMs noded started (as `work_engine.rs` stands them in).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

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

/// The work items pushed to `cid`, as (work, prompt).
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
                .then(|| (work["work"].as_u64().unwrap_or(0), text(&work["prompt"])))
        })
        .collect()
}

/// `cid`'s model call for `work`: the hub's answer.
fn model_call(hub: &Hub, cid: u64, work: u64, req: u64) -> Value {
    let request = json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Do the task." }] }] });
    let message = json!({ "type": "model_call", "req": req, "work": work, "request": request });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// A project with its builder on the mock agent and a budget the calls fit in.
fn project(hub: &Hub) -> String {
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Changelog", "goal": "Keep the changelog.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, budget) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/budget"),
        Some(json!({ "unit": "tokens", "cap": 10_000_000 })),
    );
    assert_eq!(status, 200, "{budget}");
    let (status, settings) = hub.call(
        "PATCH",
        &format!("/v1/projects/{pid}/settings"),
        Some(json!({ "role_models": { "builder": "mock/agent", "reviewer": "mock/echo" } })),
    );
    assert_eq!(status, 200, "{settings}");
    pid
}

fn open_cards(hub: &Hub) -> Vec<Value> {
    let (_, cards) = hub.call("GET", "/v1/cards?open=true", None);
    cards.as_array().cloned().unwrap_or_default()
}

/// A hub with the roles a builder, a reviewer and a verifier need, and the work engine's stand-ins.
struct Rig {
    hub: Hub,
    gateway: FakeDaemon,
    usage: FakeDaemon,
    _noded: FakeDaemon,
    _scratch: Scratch,
}

fn rig(name: &str, reset_in_ms: u64) -> Rig {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let roles = json!({ "roles": {
        "orchestrator": { "role": "orchestrator", "title": "Orchestrator", "tools": ["run", "read"] },
        "builder": { "role": "builder", "title": "Builder", "tools": ["run", "run_tests", "read"] },
        "reviewer": { "role": "reviewer", "title": "Reviewer", "tools": ["run", "read"] },
        "verifier": { "role": "verifier", "title": "Verifier", "tools": ["run", "read"] },
    } });
    let scratch = Scratch::new(name);
    let no_weft = scratch.0.join("no-weft");
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[("WEFT_BIN", no_weft.to_str().expect("a path"))],
    );
    Rig {
        hub,
        gateway,
        usage: usage_hub(reset_in_ms),
        _noded: noded,
        _scratch: scratch,
    }
}

/// A builder whose first call waited on its subscription's allowance.
struct Waiting {
    pid: String,
    tid: String,
    session_id: String,
    cid: u64,
    work: u64,
}

/// The policy names the mock agent as a subscription, a fake CLIProxyAPI is the usage provider, and a project and a
/// task run its builder on the mock agent. The builder's first call waits (paused by its allowance, card asked).
fn waiting_builder(rig: &Rig) -> Waiting {
    let hub = &rig.hub;
    // A subscription entry the policy does not name is refused; the named one is saved.
    let (status, refused) = hub.call(
        "PUT",
        "/v1/settings",
        Some(json!({ "model_policy": { "orchestrator": ORCHESTRATOR, "worker": WORKER[..6], "subscription": ["mock/agent"] } })),
    );
    assert_eq!(status, 400, "{refused}");
    assert!(
        refused.to_string().contains("not in the model policy"),
        "{refused}"
    );
    let (status, saved) = hub.call(
        "PUT",
        "/v1/settings",
        Some(json!({ "model_policy": { "orchestrator": ORCHESTRATOR, "worker": WORKER, "subscription": ["mock/agent"] } })),
    );
    assert_eq!(status, 200, "{saved}\n{}", hub.log());
    assert_eq!(
        saved["model_policy"]["subscription"],
        json!(["mock/agent"]),
        "{saved}"
    );

    let (status, provider) = hub.call(
        "POST",
        "/v1/usage-providers",
        Some(json!({ "id": "fake-hub", "url": format!("http://127.0.0.1:{}", rig.usage.port), "key": "test-management-key", "label": "Fake hub" })),
    );
    assert_eq!(status, 201, "{provider}");
    assert_eq!(provider["last"]["status"], "ok", "{provider}");
    let (status, windows) = hub.call("GET", "/v1/allowances", None);
    assert_eq!(status, 200, "{windows}");
    assert_eq!(windows.as_array().map(Vec::len), Some(1), "{windows}");
    assert_eq!(windows[0]["vendor"], "mock", "{windows}");
    assert_eq!(windows[0]["blocking"], true, "{windows}");
    assert_eq!(windows[0]["models"], json!(["mock/agent"]), "{windows}");

    let pid = project(hub);
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "Update the changelog", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);
    let session = hub.wait_for(&format!("/v1/sessions?task={tid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["seat"] == "builder" && s["cid"].as_u64().is_some())
        })
    });
    let builder = session
        .as_array()
        .and_then(|s| s.iter().find(|s| s["seat"] == "builder").cloned())
        .unwrap_or_else(|| panic!("a builder session: {session}\n{}", hub.log()));
    let cid = builder["cid"].as_u64().expect("its VM");
    let session_id = text(&builder["id"]);
    let (status, hello) = hub.guest(cid, "hello", json!({ "role": "builder", "vm": 0 }));
    assert_eq!(status, 200, "{hello}");
    let (work, prompt) = {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            if let Some(found) = pushed(&rig.gateway, cid).into_iter().next() {
                break found;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no work reached {cid}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    assert!(prompt.contains("Update the changelog"), "{prompt}");

    let reply = model_call(hub, cid, work, 1);
    assert_eq!(reply["ok"], false, "{reply}");
    let error = text(&reply["error"]);
    assert!(error.starts_with("paused: "), "{reply}");
    assert!(error.contains("allowance"), "{reply}");
    let (_, paused) = hub.call("GET", &format!("/v1/sessions/{session_id}"), None);
    assert_eq!(paused["state"], "paused", "{paused}\n{}", hub.log());
    assert_eq!(paused["paused_by"], "allowance", "{paused}");
    let card = open_cards(hub)
        .into_iter()
        .find(|c| c["subject"] == format!("allowance:mock:{pid}"))
        .unwrap_or_else(|| panic!("an allowance card: {}", hub.log()));
    assert_eq!(card["kind"], "allowance", "{card}");
    assert_eq!(card["task"], tid, "{card}");
    assert!(
        card["body"].as_str().unwrap_or("").contains("resets in"),
        "{card}"
    );
    Waiting {
        pid,
        tid,
        session_id,
        cid,
        work,
    }
}

#[test]
fn a_subscription_call_waits_while_its_allowance_is_used_up_and_runs_again_at_the_reset() {
    let rig = rig("subscriptions", 8_000);
    let hub = &rig.hub;
    let waiting = waiting_builder(&rig);

    // The window resets: the ticker resumes the session without anyone asking, and the card is withdrawn.
    let resumed = hub.wait_for(&format!("/v1/sessions/{}", waiting.session_id), WAIT, |s| {
        s["state"] == "running"
    });
    assert_eq!(resumed["paused_by"], Value::Null, "{resumed}");
    let deadline = std::time::Instant::now() + WAIT;
    while open_cards(hub)
        .iter()
        .any(|c| c["subject"] == format!("allowance:mock:{}", waiting.pid))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the allowance card is withdrawn at the reset: {}",
            hub.log()
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    // Its next call goes through to the model, and its usage is charged to the task.
    let reply = model_call(hub, waiting.cid, waiting.work, 2);
    assert_eq!(reply["ok"], true, "{reply}");
    let (_, budget) = hub.call("GET", &format!("/v1/tasks/{}/budget", waiting.tid), None);
    assert!(
        budget["spent"].as_u64().unwrap_or(0) > 0,
        "charged: {budget}"
    );
}

#[test]
fn the_api_key_choice_moves_the_project_to_the_policys_fallback_and_resumes_its_session_at_once() {
    // The window stays used up for ten minutes: only the API-key route can get the call through.
    let rig = rig("subscriptions-api-key", 600_000);
    let hub = &rig.hub;
    let waiting = waiting_builder(&rig);

    let (_, cards) = hub.call("GET", "/v1/cards?open=true", None);
    let card = cards
        .as_array()
        .and_then(|c| {
            c.iter()
                .find(|c| c["subject"] == format!("allowance:mock:{}", waiting.pid))
                .cloned()
        })
        .unwrap_or_else(|| panic!("the allowance card: {cards}"));
    let choices: Vec<&str> = card["choices"]
        .as_array()
        .map(|c| c.iter().filter_map(|c| c["id"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(choices, ["wait", "api_key", "stop"], "{card}");

    let (status, answered) = hub.call(
        "POST",
        &format!("/v1/cards/{}", text(&card["id"])),
        Some(json!({ "choice": "api_key" })),
    );
    assert_eq!(status, 200, "{answered}\n{}", hub.log());
    assert_eq!(answered["card"]["answer"], "api_key", "{answered}");

    // The session runs again at once (the window has not reset), and its call goes to the fallback model.
    let resumed = hub.wait_for(
        &format!("/v1/sessions/{}", waiting.session_id),
        Duration::from_secs(5),
        |s| s["state"] == "running",
    );
    assert_eq!(resumed["paused_by"], Value::Null, "{resumed}");
    let reply = model_call(hub, waiting.cid, waiting.work, 3);
    assert_eq!(
        reply["ok"],
        true,
        "the fallback answered: {reply}\n{}",
        hub.log()
    );
    let (_, budget) = hub.call("GET", &format!("/v1/tasks/{}/budget", waiting.tid), None);
    assert!(
        budget["spent"].as_u64().unwrap_or(0) > 0,
        "charged: {budget}"
    );
    let (_, open) = hub.call("GET", "/v1/cards?open=true", None);
    assert!(
        !open
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["subject"] == format!("allowance:mock:{}", waiting.pid)),
        "the card is answered: {open}"
    );
}

#[test]
fn the_login_bridge_is_off_by_default_offered_per_vendor_and_owner_only_over_http() {
    // Switching a vendor on writes the `login_bridge` table, which 0001_init.sql does not create yet (the lead merges
    // `p133-schema.sql` after review); until then the save is refused with 500. The switch's on path and its refusals
    // are unit-tested in `login_bridge.rs`; this test covers what the hub answers without a save.
    let scratch = Scratch::new("subscriptions-bridge");
    let hub = Hub::start(&scratch, None, None);

    let (status, bridges) = hub.call("GET", "/v1/login-bridges", None);
    assert_eq!(status, 200, "{bridges}");
    let vendors: Vec<&str> = bridges
        .as_array()
        .map(|b| b.iter().filter_map(|b| b["vendor"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(vendors, ["anthropic", "openai"], "{bridges}");
    for bridge in bridges.as_array().into_iter().flatten() {
        assert_eq!(bridge["enabled"], false, "off by default: {bridge}");
        assert_eq!(bridge["usable"], false, "{bridge}");
        assert_eq!(bridge["refusal"], "Off", "{bridge}");
        assert!(
            bridge["terms"]
                .as_str()
                .unwrap_or("")
                .contains("PLAN 28.1 point 2"),
            "labelled with the terms: {bridge}"
        );
    }

    // Not the owner: a page of another origin reaching the hub on loopback gets no loopback trust, and the hub's
    // agent token is a credential of the machine, not of its owner. The hub's `/v1` gate answers it 401 before the
    // route runs; the route's own 403 (`refused`) is the second line. Both the read and the switch are refused, and the
    // switch stays off.
    let agent = format!("Bearer {}", hub.token());
    let cross_site = [
        ("authorization", agent.as_str()),
        ("origin", "https://elsewhere.example"),
        ("sec-fetch-site", "cross-site"),
    ];
    let (status, refused) = hub.send_with_headers("GET", "/v1/login-bridges", &cross_site, &[]);
    assert_eq!(status, 401, "{refused}");
    let (status, refused) = hub.send_with_headers(
        "PUT",
        "/v1/login-bridges/anthropic",
        &[
            ("authorization", agent.as_str()),
            ("origin", "https://elsewhere.example"),
            ("sec-fetch-site", "cross-site"),
            ("content-type", "application/json"),
        ],
        br#"{"enabled":true}"#,
    );
    assert_eq!(status, 401, "{refused}");
    let (_, bridges) = hub.call("GET", "/v1/login-bridges", None);
    assert!(
        bridges
            .as_array()
            .into_iter()
            .flatten()
            .all(|b| b["enabled"] == false),
        "a refused switch changes nothing: {bridges}"
    );

    let (status, unknown) = hub.call(
        "PUT",
        "/v1/login-bridges/example",
        Some(json!({ "enabled": true })),
    );
    assert_eq!(status, 404, "{unknown}");
    let (status, bad) = hub.call(
        "PUT",
        "/v1/login-bridges/openai",
        Some(json!({ "enabled": "yes" })),
    );
    assert_eq!(status, 400, "{bad}");
}
