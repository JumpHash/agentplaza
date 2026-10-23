//! The work engine's frames on the event stream (Engine 0.5, 0.10): a budget's change reaches the consoles as a `budget`
//! frame carrying the budget's view, and a chat's edit as a `chat` frame. Read off the owner's socket on a scratch hub
//! (`/v1/ws`, the `apz.events.v1` protocol), as the consoles read them.

mod common;

use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Socket};
use serde_json::{Value, json};

use common::{Hub, Scratch};

const WAIT: Duration = Duration::from_secs(10);

/// The owner's event socket on `hub`, open (its hello is read by [`frame`] with the rest).
fn events(hub: &Hub) -> Socket {
    let target = Target {
        base: format!("http://127.0.0.1:{}", hub.port),
        ..Default::default()
    };
    ws::connect(
        &target,
        "/v1/ws",
        Some("apz.events.v1"),
        WAIT,
        Limits::default(),
    )
    .expect("the event socket opens")
}

/// The data of the next `kind` frame that `ready` accepts, within [`WAIT`]; the other frames are passed over.
fn frame(socket: &mut Socket, kind: &str, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if let Ok(Some(Message::Text(text))) = socket.recv(Duration::from_millis(200)) {
            let event: Value = serde_json::from_str(&text).expect("a frame is JSON");
            if event["event"] == kind && ready(&event["data"]) {
                return event["data"].clone();
            }
        }
    }
    panic!("no {kind} frame came within {WAIT:?}");
}

#[test]
fn an_edit_of_a_chat_publishes_a_chat_frame() {
    let scratch = Scratch::new("chat-frame");
    let hub = Hub::start(&scratch, None, None);
    let mut socket = events(&hub);
    let (status, chat) = hub.call(
        "PATCH",
        "/v1/chats/main",
        Some(json!({ "title": "Renamed" })),
    );
    assert_eq!(status, 200, "{chat}");
    let data = frame(&mut socket, "chat", |d| {
        d["chat"] == "main" && d["updated"] == true
    });
    assert_eq!(data["chat"], "main");
}

#[test]
fn a_budget_set_by_the_owner_publishes_its_view_as_a_budget_frame() {
    let scratch = Scratch::new("budget-frame");
    let hub = Hub::start(&scratch, None, None);
    let mut socket = events(&hub);
    // The plaza's own budget needs no project.
    let (status, budget) = hub.call(
        "PUT",
        "/v1/projects/plaza/budget",
        Some(json!({ "unit": "usd", "cap": 5_000_000 })),
    );
    assert_eq!(status, 200, "{budget}");
    let data = frame(&mut socket, "budget", |d| {
        d["owner"] == "plaza" && d["cap"] == 5_000_000
    });
    assert_eq!(data["unit"], "usd");
    assert_eq!(data["left"], 5_000_000, "{data}");
    assert!(data.get("spent_pct").is_some(), "{data}");
}
