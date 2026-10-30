// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The orchestrator's VM can say hello before noded's run report names its role. The hello then registers the agent
// with no role, the owner's first message to the main chat gets the "not running" notice, and the report that names
// the orchestrator (the reconcile's next pass) must adopt it and deliver that message, not drop it.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;

use common::{FakeDaemon, Hub, Scratch};

/// The orchestrator VM's guest CID in this test.
const CID: u64 = 109;

#[test]
fn a_message_said_before_the_orchestrators_role_is_known_is_delivered_when_it_is() {
    let scratch = Scratch::new("orchestrator-role");
    // noded reports the VM's role only in its runs, and only once `reported` is set: until then the runs are empty,
    // so the hello comes before any report names the role (the race this test is about).
    let reported = Arc::new(AtomicBool::new(false));
    let report = Arc::clone(&reported);
    let noded = FakeDaemon::serve(move |method, path, _| {
        if method == "GET" && path.starts_with("/v1/runs") && report.load(Ordering::SeqCst) {
            let runs = json!({
                "runs": [{
                    "id": "r1",
                    "ended_at": null,
                    "vms": [{ "id": 0, "guest_cid": CID, "role": "orchestrator" }],
                }]
            });
            (200, runs.to_string())
        } else if method == "GET" && path.starts_with("/v1/runs") {
            (200, json!({ "runs": [] }).to_string())
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    });
    // Promotion must deliver the trusted profile to a live gateway before queued work is admitted.
    let gateway = FakeDaemon::serve(|_, _, _| (200, json!({"ok":true}).to_string()));
    let hub = Hub::start(&scratch, Some(gateway.port), Some(noded.port));

    let (status, _) = hub.guest(CID, "hello", json!({ "role": "orchestrator", "vm": 0 }));
    assert_eq!(status, 200);
    let (status, _) = hub.call(
        "POST",
        "/v1/chats/main/messages",
        Some(json!({ "text": "hello orchestrator", "attachments": [] })),
    );
    assert_eq!(status, 201);
    // Before the report the role is unknown: the message gets the "not running" notice.
    let before = hub.call("GET", "/v1/chats/main", None).1;
    assert!(
        before["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["kind"] == "notice"),
        "the message should wait for the role: {before}"
    );

    // The report now names the VM's role: it arrives on the reconcile's next pass, the VM takes the orchestrator's
    // role...
    reported.store(true, Ordering::SeqCst);
    let agent = hub.wait_for("/v1/agents/109", Duration::from_secs(40), |agent| {
        agent["role"] == "orchestrator"
    });
    assert_eq!(agent["role"], "orchestrator");

    // ...and the owner's message goes to it: it is work of the main chat, and the chat says so.
    let chat = hub.wait_for("/v1/chats/main", Duration::from_secs(20), |chat| {
        chat["works"]
            .as_array()
            .is_some_and(|works| !works.is_empty())
    });
    let delivered = chat["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|message| message["text"] == "The orchestrator has your message.");
    assert!(delivered, "the chat does not say the message went: {chat}");
}
