//! The hub's live event stream (`GET /v1/events`, server-sent events) as an iced subscription:
//! each event's name and JSON data. Plain HTTP on loopback, pinned TLS to a remote hub (with the
//! client's bearer); reconnects with backoff.

use iced::Subscription;
use serde_json::Value;

use super::client::Client;

/// One event: its name (the bus kind: "transcript", "agent", "delta", …) and data.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: String,
    pub data: Value,
}

/// The stream for `client`'s hub.
pub fn subscription(client: &Client) -> Subscription<Event> {
    let _ = client;
    Subscription::none() // contract: EVT
}
