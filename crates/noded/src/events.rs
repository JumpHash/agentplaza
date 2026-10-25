// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The node's event bus: the live half of the console's data layer.
//!
//! The registry publishes run and VM state transitions and each VM's serial
//! deltas as they happen, to `GET /v1/ws`, a WebSocket that sends every event
//! to a connected client as one text message. The socket is read-only and no
//! browser may open it (its origin policy refuses any `Origin` header). Polling
//! stays the source of truth for a full snapshot (a run's whole serial
//! capture); the stream is the deltas.
//!
//! The messages:
//!
//! ```text
//! {"event":"run","data":{"id":"r1","state":"running"}}
//! {"event":"vm","data":{"run":"r1","vm":0,"state":"exited"}}
//! {"event":"serial","data":{"run":"r1","vm":0,"offset":120,"text":"[    0.000000] Linux version…"}}
//! ```
//!
//! `serial`'s `offset` is the byte offset the `text` starts at in the VM's
//! serial capture, so a client that already has `offset` bytes appends, a
//! client that missed a stretch re-polls, and one that sees a stale offset
//! (it already has more than the event carries) ignores the event. The
//! capture is bounded (its head only), and so are the events: once the
//! capture stops growing, no further `serial` events fire for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use apz_http::ws::{Message, Socket};

/// The events a socket may have queued before it is cut off (closed 1013):
/// a client that cannot keep up loses its stream rather than growing the node.
pub const QUEUE_BOUND: usize = 256;

/// How long one wait for client data lasts before the queue is drained again,
/// so an event reaches its socket within this.
const TICK: Duration = Duration::from_millis(50);

/// One event, as `GET /v1/ws` sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A run's state changed (or the run was registered, as `starting`).
    Run { id: String, state: &'static str },
    /// A run's VM changed state.
    Vm {
        run: String,
        vm: u32,
        state: &'static str,
    },
    /// A run's VM's serial capture grew: `offset` is the capture byte the
    /// `text` starts at.
    Serial {
        run: String,
        vm: u32,
        offset: u64,
        text: String,
    },
}

impl Event {
    /// The message's `data`: one line of JSON.
    pub fn data(&self) -> String {
        match self {
            Event::Run { id, state } => {
                format!("{{\"id\":{},\"state\":{}}}", quote(id), quote(state))
            }
            Event::Vm { run, vm, state } => format!(
                "{{\"run\":{},\"vm\":{},\"state\":{}}}",
                quote(run),
                vm,
                quote(state)
            ),
            Event::Serial {
                run,
                vm,
                offset,
                text,
            } => format!(
                "{{\"run\":{},\"vm\":{},\"offset\":{},\"text\":{}}}",
                quote(run),
                vm,
                offset,
                quote(text)
            ),
        }
    }

    /// The message's `event` name.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Run { .. } => "run",
            Event::Vm { .. } => "vm",
            Event::Serial { .. } => "serial",
        }
    }

    /// The whole WebSocket text message: `{"event": kind, "data": data}`.
    pub fn message(&self) -> String {
        format!("{{\"event\":\"{}\",\"data\":{}}}", self.kind(), self.data())
    }
}

/// A JSON string literal for `data`: the value is escaped and wrapped in
/// double quotes. The serial text is untrusted guest output — escaping it
/// is what keeps it a JSON string and nothing else.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Every other control character takes the short escape so no
            // control byte ever reaches a message raw.
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One subscriber's end of the bus: its bounded queue, and the flag the bus
/// raises when it cut the subscriber off for a full queue.
#[derive(Debug)]
pub struct Subscription {
    events: Receiver<Arc<Event>>,
    overflowed: Arc<AtomicBool>,
}

impl Subscription {
    /// Whether the bus cut this subscriber off because its queue was full.
    pub fn overflowed(&self) -> bool {
        self.overflowed.load(Ordering::Acquire)
    }
}

/// The bus's side of one subscriber.
#[derive(Debug)]
struct Subscriber {
    sender: SyncSender<Arc<Event>>,
    overflowed: Arc<AtomicBool>,
}

/// The bus the registry publishes to and the socket endpoint subscribes to.
///
/// Publishing to a subscriber that went away fails silently and drops it, so
/// a socket that closed never holds a slot; a subscriber whose queue is full
/// is cut off (dropped, and flagged). A bus with no subscribers publishes to
/// nobody, which is a no-op.
#[derive(Debug, Default)]
pub struct EventBus {
    subscribers: Mutex<Vec<Subscriber>>,
}

impl EventBus {
    /// A bus with no subscribers.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registers a subscriber with a queue of [`QUEUE_BOUND`] events.
    pub fn subscribe(&self) -> Subscription {
        let (sender, events) = sync_channel(QUEUE_BOUND);
        let overflowed = Arc::new(AtomicBool::new(false));
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Subscriber {
                sender,
                overflowed: Arc::clone(&overflowed),
            });
        Subscription { events, overflowed }
    }

    /// Publishes one event to every subscriber, dropping the ones that went
    /// away and the ones whose queue is full.
    pub fn publish(&self, event: Event) {
        // One allocation shared by every subscriber: each send clones the
        // `Arc`, so the serial text is never copied per subscriber.
        let event = Arc::new(event);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(
                |subscriber| match subscriber.sender.try_send(Arc::clone(&event)) {
                    Ok(()) => true,
                    // The flag is raised before the sender is dropped (the drop
                    // is what ends the subscription's queue).
                    Err(TrySendError::Full(_)) => {
                        subscriber.overflowed.store(true, Ordering::Release);
                        false
                    }
                    Err(TrySendError::Disconnected(_)) => false,
                },
            );
    }
}

/// Streams `subscription`'s events to `socket`, one text message each, until
/// the bus is dropped (the node is shutting down: closed 1001), the subscriber
/// is cut off for a full queue (closed 1013), or the socket ends. The stream
/// is read-only: a data message from the client closes it (1008). Liveness is
/// the socket's own: its pings, and the close of a peer that stopped answering.
pub fn ws_stream(subscription: &Subscription, socket: &mut Socket) -> std::io::Result<()> {
    loop {
        if socket.recv(TICK)?.is_some() {
            socket.close(1008, "this socket takes no messages");
            return Ok(());
        }
        loop {
            if subscription.overflowed() {
                socket.close(1013, "the event queue is full");
                return Ok(());
            }
            match subscription.events.try_recv() {
                Ok(event) => socket.send(&Message::Text(event.message()))?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if subscription.overflowed() {
                        socket.close(1013, "the event queue is full");
                    } else {
                        socket.close(1001, "the node is shutting down");
                    }
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_carry_their_events() {
        assert_eq!(
            Event::Run {
                id: "r1".to_string(),
                state: "running"
            }
            .message(),
            "{\"event\":\"run\",\"data\":{\"id\":\"r1\",\"state\":\"running\"}}"
        );
        assert_eq!(
            Event::Vm {
                run: "r1".to_string(),
                vm: 2,
                state: "exited"
            }
            .message(),
            "{\"event\":\"vm\",\"data\":{\"run\":\"r1\",\"vm\":2,\"state\":\"exited\"}}"
        );
        assert_eq!(
            Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 120,
                text: "hello".to_string(),
            }
            .message(),
            "{\"event\":\"serial\",\"data\":{\"run\":\"r1\",\"vm\":0,\"offset\":120,\"text\":\"hello\"}}"
        );
    }

    #[test]
    fn guest_text_is_escaped_into_the_json_string() {
        // A quote, a backslash, a newline and a control byte: none of these
        // may break out of the string or the message.
        let message = Event::Serial {
            run: "r1".to_string(),
            vm: 0,
            offset: 0,
            text: "a\"b\\c\nd\u{1}e".to_string(),
        }
        .message();
        assert!(
            message.chars().all(|c| c as u32 >= 0x20),
            "no control byte reaches the message raw: {message:?}"
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&message).expect("the message is one JSON document");
        assert_eq!(parsed["event"], "serial");
        assert_eq!(parsed["data"]["text"], "a\"b\\c\nd\u{1}e");
    }

    #[test]
    fn a_subscriber_receives_and_a_dead_one_is_dropped() {
        let bus = EventBus::new();
        let live = bus.subscribe();
        let dead = bus.subscribe();
        drop(dead);

        bus.publish(Event::Run {
            id: "r1".to_string(),
            state: "starting",
        });
        assert_eq!(
            live.events.recv_timeout(Duration::from_secs(1)),
            Ok(Arc::new(Event::Run {
                id: "r1".to_string(),
                state: "starting"
            }))
        );
        // The dead subscriber was dropped on publish: the bus holds one sender.
        assert_eq!(
            bus.subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1
        );
        assert!(!live.overflowed());
    }

    #[test]
    fn a_full_queue_cuts_its_subscriber_off() {
        let bus = EventBus::new();
        let slow = bus.subscribe();
        let event = || Event::Run {
            id: "r1".to_string(),
            state: "running",
        };
        for _ in 0..QUEUE_BOUND {
            bus.publish(event());
        }
        assert!(!slow.overflowed(), "a queue that fits is not cut off");

        bus.publish(event());
        assert!(slow.overflowed(), "one event past the bound cuts it off");
        assert_eq!(
            bus.subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            0
        );
    }

    #[test]
    fn a_closed_bus_is_a_disconnect_not_an_overflow() {
        let bus = EventBus::new();
        let subscription = bus.subscribe();
        // Dropping the bus drops every sender: how a daemon's shutdown ends
        // its streams. That is not an overflow.
        drop(bus);
        assert_eq!(
            subscription.events.try_recv(),
            Err(TryRecvError::Disconnected)
        );
        assert!(!subscription.overflowed());
    }
}
