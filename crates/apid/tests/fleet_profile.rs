//! A guest can hello before noded identifies its trusted role. Observe the real apid's gateway
//! messages: its new model and tools must reach the guest before queued owner work does, including
//! when the first profile delivery fails. No provider or VM is contacted by these tests.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch};

const CID: u64 = 109;
const ORCHESTRATOR_MODEL: &str = "codex-local/gpt-6-astra@ultra";
const WORKER_MODEL: &str = "local-deepseek/deepseek/deepseek-flash";
const PROMPT: &str = "Create the furniture shop after receiving your trusted profile.";
const WAIT: Duration = Duration::from_secs(45);

#[derive(Default)]
struct Wire {
    /// Every attempted delivery, in arrival order, with whether the gateway accepted it.
    messages: Vec<(Value, bool)>,
}

fn wait_for_wire(wire: &Mutex<Wire>, ready: impl Fn(&Wire) -> bool) {
    let until = Instant::now() + WAIT.mul_f64(common::slack());
    loop {
        if ready(&wire.lock().unwrap()) {
            return;
        }
        assert!(
            Instant::now() < until,
            "gateway never received the expected message"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn has_tool(profile: &Value, name: &str) -> bool {
    profile["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == name))
}

fn reconcile_profile(reject_first: bool) {
    let scratch = Scratch::new(if reject_first {
        "fleet-profile-retry"
    } else {
        "fleet-profile"
    });
    let reported = Arc::new(AtomicBool::new(false));
    let report = Arc::clone(&reported);
    let noded = FakeDaemon::serve(move |method, path, _| {
        if method == "GET" && path.starts_with("/v1/runs") {
            let runs = if report.load(Ordering::SeqCst) {
                json!([{"id":"fleet-run", "ended_at":null,
                    "vms":[{"id":0,"guest_cid":CID,"role":"orchestrator"}]}])
            } else {
                json!([])
            };
            (200, json!({"runs":runs}).to_string())
        } else {
            (200, "{}".into())
        }
    });
    let connected = Arc::new(AtomicBool::new(false));
    let connection = Arc::clone(&connected);
    let accept_profile = Arc::new(AtomicBool::new(!reject_first));
    let accept = Arc::clone(&accept_profile);
    let wire = Arc::new(Mutex::new(Wire::default()));
    let observed = Arc::clone(&wire);
    let gateway = FakeDaemon::serve(move |method, path, body| {
        if method == "GET" && path.starts_with("/v1/status") {
            let agents = if connection.load(Ordering::SeqCst) {
                // This is the guest's claimed role, not noded's trusted inventory.
                json!([{"cid":CID,"role":"orchestrator"}])
            } else {
                json!([])
            };
            return (200, json!({"agents":agents}).to_string());
        }
        if method == "POST" && path.ends_with(&format!("/agents/{CID}/send")) {
            let body: Value = serde_json::from_str(body).expect("gateway message is JSON");
            let message = body["message"].clone();
            let trusted_profile =
                message["type"] == "profile" && message["profile"]["role"] == "orchestrator";
            let accepted = !trusted_profile || accept.load(Ordering::SeqCst);
            observed.lock().unwrap().messages.push((message, accepted));
            if !accepted {
                return (
                    503,
                    json!({"error":"temporary profile delivery failure"}).to_string(),
                );
            }
        }
        (200, json!({"ok":true}).to_string())
    });
    let hub = Hub::start(&scratch, Some(gateway.port), Some(noded.port));
    let (status, mut settings) = hub.call("GET", "/v1/settings", None);
    assert_eq!(status, 200);
    settings["default_model"] = json!(WORKER_MODEL);
    settings["orchestrator_model"] = json!(ORCHESTRATOR_MODEL);
    settings["role_models"] = json!({});
    settings["model_policy"]["orchestrator"] = json!(["codex-local/gpt-6-astra"]);
    settings["model_policy"]["worker"] = json!([WORKER_MODEL]);
    let (status, reply) = hub.call("PUT", "/v1/settings", Some(settings));
    assert_eq!(status, 200, "{reply}");

    let (status, welcome) = hub.guest(CID, "hello", json!({"role":"orchestrator","vm":0}));
    assert_eq!(status, 200, "{welcome}");
    assert_ne!(welcome["profile"]["role"], "orchestrator");
    assert_eq!(welcome["profile"]["model"], WORKER_MODEL);
    assert!(!has_tool(&welcome["profile"], "plaza_repository"));
    connected.store(true, Ordering::SeqCst);
    let (status, reply) = hub.call(
        "POST",
        "/v1/chats/main/messages",
        Some(json!({"text":PROMPT,"attachments":[]})),
    );
    assert_eq!(status, 201, "{reply}");
    assert!(
        !wire
            .lock()
            .unwrap()
            .messages
            .iter()
            .any(|(m, _)| m["type"] == "work")
    );

    reported.store(true, Ordering::SeqCst);
    if reject_first {
        wait_for_wire(&wire, |w| {
            w.messages
                .iter()
                .any(|(m, accepted)| m["type"] == "profile" && !accepted)
        });
        // The failed promotion must not become a terminal, apparently-ready roster entry.
        let agent = hub.wait_for(&format!("/v1/agents/{CID}"), WAIT, |a| {
            a["role"] != "orchestrator"
        });
        assert_ne!(agent["role"], "orchestrator", "{agent}");
        assert!(
            !wire
                .lock()
                .unwrap()
                .messages
                .iter()
                .any(|(m, _)| m["type"] == "work")
        );
        let chat = hub.call("GET", "/v1/chats/main", None).1;
        assert!(
            chat["works"].as_array().is_none_or(Vec::is_empty),
            "work admitted before the profile: {chat}"
        );
        accept_profile.store(true, Ordering::SeqCst);
    }

    wait_for_wire(&wire, |w| {
        w.messages.iter().any(|(m, _)| m["type"] == "work")
    });
    let messages = &wire.lock().unwrap().messages;
    let work_at = messages
        .iter()
        .position(|(m, _)| m["type"] == "work")
        .unwrap();
    let profile_at = messages
        .iter()
        .position(|(m, accepted)| {
            *accepted && m["type"] == "profile" && m["profile"]["role"] == "orchestrator"
        })
        .expect("an accepted trusted profile preceded work");
    assert!(
        profile_at < work_at,
        "work reached the guest before its trusted profile: {messages:?}"
    );
    let profile = &messages[profile_at].0["profile"];
    assert_eq!(profile["model"], ORCHESTRATOR_MODEL);
    assert!(
        has_tool(profile, "plaza_repository"),
        "orchestrator provisioning tool missing: {profile}"
    );
    assert!(has_tool(profile, "tool_search"));
    assert!(
        messages[work_at].0["prompt"]
            .as_str()
            .is_some_and(|p| p.contains(PROMPT))
    );
    assert_eq!(
        messages.iter().filter(|(m, _)| m["type"] == "work").count(),
        1
    );
    if reject_first {
        assert!(
            messages[..profile_at]
                .iter()
                .any(|(m, accepted)| !accepted && m["type"] == "profile")
        );
    }
    let agent = hub.call("GET", &format!("/v1/agents/{CID}"), None).1;
    assert_eq!(agent["role"], "orchestrator");
}

#[test]
fn trusted_fleet_profile_arrives_before_queued_owner_work() {
    reconcile_profile(false);
}

#[test]
fn failed_trusted_profile_is_retried_before_admitting_owner_work() {
    reconcile_profile(true);
}
