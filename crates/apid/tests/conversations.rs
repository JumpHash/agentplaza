//! The owner's conversations with the orchestrator, end to end on a scratch hub: two conversations are two threads
//! that never mix, each answer lands in the conversation it answers, and a conversation's transcript holds only its
//! own turns. The agent gateway and noded are stood in for: noded reports the orchestrator's VM, so the hub trusts its
//! role, and the gateway takes the work the hub pushes, which the test answers as the orchestrator's harness would.

mod common;

use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message};
use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// The orchestrator's guest CID: slot 9 over the fleet's CID base.
const ORCHESTRATOR: u64 = 109;

/// The work items the hub pushed to the orchestrator for the conversations, through the gateway, as (work, thread,
/// prompt). The hub also sends the orchestrator work of its own on start, on threads that are not conversations; those
/// are not this test's.
fn pushed(gateway: &FakeDaemon) -> Vec<(u64, String, String)> {
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, _)| {
            method == "POST" && path.ends_with(&format!("/agents/{ORCHESTRATOR}/send"))
        })
        .filter_map(|(_, _, body)| {
            let body: Value = serde_json::from_str(&body).ok()?;
            let work = &body["message"];
            (work["type"] == "work").then(|| {
                (
                    work["work"].as_u64().unwrap_or(0),
                    text(&work["thread"]),
                    text(&work["prompt"]),
                )
            })
        })
        .filter(|(_, thread, _)| thread.starts_with("conv-"))
        .collect()
}

/// The conversations' pushes once there are at least `count` of them, or the last look after 20 s. The pushes come
/// through the gateway asynchronously, so a read straight after the hub answers can miss one.
fn wait_for_pushes(gateway: &FakeDaemon, count: usize) -> Vec<(u64, String, String)> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let sent = pushed(gateway);
        if sent.len() >= count || Instant::now() >= deadline {
            return sent;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits on the hub's event socket for an `owner_conversation` event that `wanted` takes; its data, if one comes in time.
fn conversation_event(socket: &mut ws::Socket, wanted: impl Fn(&Value) -> bool) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(Message::Text(frame))) = socket.recv(Duration::from_millis(200)) {
            let frame: Value = serde_json::from_str(&frame).unwrap_or_default();
            if frame["event"] == "owner_conversation" && wanted(&frame["data"]) {
                return Some(frame["data"].clone());
            }
        }
    }
    None
}

/// The messages of a conversation, as (from, text).
fn messages(conversation: &Value) -> Vec<(String, String)> {
    conversation["messages"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|m| (text(&m["from"]), text(&m["text"])))
                .collect()
        })
        .unwrap_or_default()
}

/// The work items of transcript entries.
fn works(entries: &Value) -> Vec<u64> {
    let mut works: Vec<u64> = entries
        .as_array()
        .map(|rows| rows.iter().filter_map(|e| e["work"].as_u64()).collect())
        .unwrap_or_default();
    works.dedup();
    works
}

#[test]
fn two_conversations_are_two_threads_and_each_gets_its_own_answers() {
    let noded = FakeDaemon::serve(|_, path, _| {
        if path.starts_with("/v1/runs") {
            let runs = json!({ "runs": [{
                "id": "r1",
                "ended_at": null,
                "vms": [{ "id": 0, "guest_cid": ORCHESTRATOR, "role": "orchestrator" }],
            }] });
            (200, runs.to_string())
        } else {
            (200, "{}".into())
        }
    });
    let gateway = FakeDaemon::serve(|_, path, _| {
        if path.starts_with("/v1/status") {
            let status = json!({ "agents": [{ "cid": ORCHESTRATOR, "role": "orchestrator" }] });
            (200, status.to_string())
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    });
    let scratch = Scratch::new("conversations");
    scratch.name_agents(&[(ORCHESTRATOR, "Ada")]);
    let hub = Hub::start(&scratch, Some(gateway.port), Some(noded.port));

    // No conversation before the orchestrator is there, and none with an empty message.
    let agent = hub.wait_for(
        &format!("/v1/agents/{ORCHESTRATOR}"),
        Duration::from_secs(20),
        |agent| agent["role"] == "orchestrator" && agent["status"] == "idle",
    );
    assert_eq!(agent["role"], "orchestrator", "{agent}\n{}", hub.log());
    let (status, refused) = hub.call(
        "POST",
        "/v1/owner/conversations",
        Some(json!({ "text": "" })),
    );
    assert_eq!(status, 400, "{refused}");

    // Two conversations, each its own thread on the orchestrator.
    let (status, first) = hub.call(
        "POST",
        "/v1/owner/conversations",
        Some(json!({ "text": "The scheduler tests are flaky and I want 0.9 out this week." })),
    );
    assert_eq!(status, 201, "{first}");
    assert_eq!(first["id"], "conv-1");
    assert_eq!(first["thread"], "conv-1");
    assert_eq!(
        first["title"],
        "The scheduler tests are flaky and I want 0.9 out this week."
    );
    let (status, second) = hub.call(
        "POST",
        "/v1/owner/conversations",
        Some(json!({ "text": "What did the auditor find last week?" })),
    );
    assert_eq!(status, 201, "{second}");
    assert_eq!(second["id"], "conv-2");
    let first_work = first["works"][0]
        .as_u64()
        .expect("the first message's work item");
    let second_work = second["works"][0]
        .as_u64()
        .expect("the second message's work item");
    assert_eq!(first["pending"], json!([first_work]));

    // The first one is pushed at once; the second waits behind it on the orchestrator's lane.
    let sent = wait_for_pushes(&gateway, 1);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!((sent[0].0, sent[0].1.as_str()), (first_work, "conv-1"));

    // The orchestrator answers the first, and the answer arrives on the event socket; the second goes out next, on
    // its own thread.
    let target = Target {
        base: format!("http://127.0.0.1:{}", hub.port),
        ..Default::default()
    };
    let mut events = ws::connect(
        &target,
        "/v1/ws",
        Some("apz.events.v1"),
        Duration::from_secs(5),
        Limits::default(),
    )
    .expect("the hub's event socket");
    hub.work_done(
        ORCHESTRATOR,
        first_work,
        "done",
        "Which repo, and is the sanction double-charge part of 0.9?",
    );
    let event = conversation_event(&mut events, |data| {
        data["id"] == "conv-1" && data["pending"] == json!([])
    })
    .expect("the answer arrives as an owner_conversation event");
    assert_eq!(messages(&event).len(), 2, "{event}");
    let sent = wait_for_pushes(&gateway, 2);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!((sent[1].0, sent[1].1.as_str()), (second_work, "conv-2"));
    hub.work_done(
        ORCHESTRATOR,
        second_work,
        "done",
        "Two findings, both fixed.",
    );

    let (_, first) = hub.call("GET", "/v1/owner/conversations/conv-1", None);
    assert_eq!(
        messages(&first),
        [
            (
                "owner".to_string(),
                "The scheduler tests are flaky and I want 0.9 out this week.".to_string()
            ),
            (
                "orchestrator".to_string(),
                "Which repo, and is the sanction double-charge part of 0.9?".to_string()
            ),
        ]
    );
    assert_eq!(first["pending"], json!([]));
    let (_, second) = hub.call("GET", "/v1/owner/conversations/conv-2", None);
    assert_eq!(
        messages(&second),
        [
            (
                "owner".to_string(),
                "What did the auditor find last week?".to_string()
            ),
            (
                "orchestrator".to_string(),
                "Two findings, both fixed.".to_string()
            ),
        ]
    );

    // The owner answers in the first conversation: it goes out on that conversation's thread.
    let (status, first) = hub.call(
        "POST",
        "/v1/owner/conversations/conv-1/messages",
        Some(json!({ "text": "agentplaza; yes, the double-charge too." })),
    );
    assert_eq!(status, 200, "{first}");
    let third_work = first["works"][1].as_u64().expect("the reply's work item");
    let sent = wait_for_pushes(&gateway, 3);
    assert_eq!((sent[2].0, sent[2].1.as_str()), (third_work, "conv-1"));
    assert_eq!(sent[2].2, "agentplaza; yes, the double-charge too.");
    // A failed turn says why in the conversation, as the orchestrator's message.
    hub.work_done(ORCHESTRATOR, third_work, "failed", "the model declined");
    let (_, first) = hub.call("GET", "/v1/owner/conversations/conv-1", None);
    let last = first["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        (last["from"].clone(), last["status"].clone()),
        (json!("orchestrator"), json!("failed"))
    );

    // Each conversation's transcript is its own turns only, through either route.
    let (status, entries) = hub.call("GET", "/v1/owner/conversations/conv-1/transcript", None);
    assert_eq!(status, 200, "{entries}");
    assert_eq!(works(&entries), [first_work, third_work]);
    let (_, entries) = hub.call(
        "GET",
        &format!("/v1/agents/{ORCHESTRATOR}/transcript?thread=conv-2"),
        None,
    );
    assert_eq!(works(&entries), [second_work]);

    // The list is newest first, without the messages; renaming and archiving keep a conversation.
    let (_, list) = hub.call("GET", "/v1/owner/conversations", None);
    let ids: Vec<String> = list
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| text(&c["id"]))
        .collect();
    assert_eq!(ids, ["conv-1", "conv-2"]);
    assert_eq!(list[0]["messages"], 4);
    let (status, renamed) = hub.call(
        "PATCH",
        "/v1/owner/conversations/conv-2",
        Some(json!({ "title": "Auditor findings", "archived": true })),
    );
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["title"], "Auditor findings");
    let (_, list) = hub.call("GET", "/v1/owner/conversations", None);
    assert_eq!(
        list.as_array().map(Vec::len),
        Some(1),
        "an archived one is listed only on request"
    );
    let (_, all) = hub.call("GET", "/v1/owner/conversations?archived=true", None);
    assert_eq!(all.as_array().map(Vec::len), Some(2));
    let (status, refused) = hub.call(
        "POST",
        "/v1/owner/conversations/conv-2/messages",
        Some(json!({ "text": "and?" })),
    );
    assert_eq!(
        status, 409,
        "an archived conversation is unarchived first: {refused}"
    );
    let (status, _) = hub.call("GET", "/v1/owner/conversations/conv-9", None);
    assert_eq!(status, 404);
}
