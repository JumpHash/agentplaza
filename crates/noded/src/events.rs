// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The node's event bus: the live half of the console's data layer.
//!
//! The registry publishes run and VM state transitions and each VM's serial
//! deltas as they happen; `GET /api/v0/events` streams every event to a
//! connected console as server-sent events, so a console sees a run's life
//! between its polls. Polling stays the source of truth for a full snapshot
//! (the register and a run's whole serial capture); the stream is the deltas.
//!
//! The frames:
//!
//! ```text
//! event: run
//! data: {"id":"r1","state":"running"}
//!
//! event: vm
//! data: {"run":"r1","vm":0,"state":"exited"}
//!
//! event: serial
//! data: {"run":"r1","vm":0,"offset":120,"text":"[    0.000000] Linux version…"}
//!
//! : heartbeat
//!
//! ```
//!
//! `serial`'s `offset` is the byte offset the `text` starts at in the VM's
//! serial capture, so a client that already has `offset` bytes appends, a
//! client that missed a stretch re-polls, and one that sees a stale offset
//! (it already has more than the event carries) ignores the event. The
//! capture is bounded (its head only), and so are the events: once the
//! capture stops growing, no further `serial` events fire for it.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One event, as `GET /api/v0/events` frames it.
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
    /// The event's server-sent frame: an `event:` line, a `data:` line of
    /// JSON, and the blank line that ends the frame.
    pub fn frame(&self) -> String {
        let data = match self {
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
        };
        let name = match self {
            Event::Run { .. } => "run",
            Event::Vm { .. } => "vm",
            Event::Serial { .. } => "serial",
        };
        format!("event: {name}\ndata: {data}\n\n")
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
            // control byte ever reaches a frame line raw.
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// How long a stream waits for an event before it writes a heartbeat
/// comment, so a dead connection is noticed (the write fails) instead of
/// sitting in the channel's wait forever.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// The bus the registry publishes to and the events endpoint subscribes to.
///
/// Publishing to a subscriber that went away fails silently and drops it, so
/// a console that closed its stream never holds a slot; a bus with no
/// subscribers publishes to nobody, which is a no-op.
#[derive(Debug, Default)]
pub struct EventBus {
    subscribers: Mutex<Vec<Sender<Event>>>,
}

impl EventBus {
    /// A bus with no subscribers.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registers a subscriber and returns its receiving end.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(sender);
        receiver
    }

    /// Publishes one event to every subscriber, dropping the ones that went
    /// away.
    pub fn publish(&self, event: Event) {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|sender| sender.send(event.clone()).is_ok());
    }



}

/// Streams `receiver`'s events as server-sent frames to `writer`: the
/// stream head first, then one frame per event, and a heartbeat comment
/// whenever [`HEARTBEAT_INTERVAL`]] passes without one. Returns when the
/// stream's channel ends (the daemon dropped it) or a write fails (the
/// console closed the stream) — the two ways a stream honestly ends.
pub fn stream(
    receiver: &Receiver<Event>,
    writer: &mut impl std::io::Write,
) -> std::io::Result<()> {
    writer.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
          Cache-Control: no-store\r\nConnection: close\r\n\r\n",
    )?;
    writer.flush()?;
    loop {
        match receiver.recv_timeout(HEARTBEAT_INTERVAL) {
            Ok(event) => writer.write_all(event.frame().as_bytes())?,
            Err(RecvTimeoutError::Timeout) => writer.write_all(b": heartbeat\n\n")?,
            // Every sender was dropped: the bus is gone, so the stream ends.
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        writer.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_carry_their_events() {
        assert_eq!(
            Event::Run {
                id: "r1".to_string(),
                state: "running"
            }
            .frame(),
            "event: run\ndata: {\"id\":\"r1\",\"state\":\"running\"}\n\n"
        );
        assert_eq!(
            Event::Vm {
                run: "r1".to_string(),
                vm: 2,
                state: "exited"
            }
            .frame(),
            "event: vm\ndata: {\"run\":\"r1\",\"vm\":2,\"state\":\"exited\"}\n\n"
        );
        assert_eq!(
            Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 120,
                text: "hello".to_string(),
            }
            .frame(),
            "event: serial\ndata: {\"run\":\"r1\",\"vm\":0,\"offset\":120,\"text\":\"hello\"}\n\n"
        );
    }

    #[test]
    fn guest_text_is_escaped_into_the_json_string() {
        // A quote, a backslash, a newline and a control byte: none of these
        // may break out of the string or the frame.
        let frame = Event::Serial {
            run: "r1".to_string(),
            vm: 0,
            offset: 0,
            text: "a\"b\\c\nd\u{1}e".to_string(),
        }
        .frame();
        assert!(
            frame.contains("\"text\":\"a\\\"b\\\\c\\nd\\u0001e\""),
            "the frame escapes the guest text: {frame}"
        );
        // Exactly one data line: no raw newline ever breaks the frame.
        assert_eq!(frame.matches("data: ").count(), 1);
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
            live.recv_timeout(Duration::from_secs(1)),
            Ok(Event::Run {
                id: "r1".to_string(),
                state: "starting"
            })
        );

        // The dead subscriber was dropped on publish: a second event
        // reaches the live one and the bus holds one sender.
        bus.publish(Event::Run {
            id: "r1".to_string(),
            state: "running",
        });
        assert_eq!(
            live.recv_timeout(Duration::from_secs(1)),
            Ok(Event::Run {
                id: "r1".to_string(),
                state: "running"
            })
        );
        assert_eq!(
            bus.subscribers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );
    }

    #[test]
    fn a_stream_ends_when_its_channel_ends() {
        let bus = EventBus::new();
        let receiver = bus.subscribe();
        // Dropping the bus drops every sender, which is how a daemon's
        // shutdown ends its streams.
        drop(bus);
        let mut out = Vec::new();
        assert!(stream(&receiver, &mut out).is_ok());
        assert!(
            out.starts_with(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream"),
            "the stream head is written: {out:?}"
        );
    }
}
