// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The node's event bus: the live half of the console's data layer.
//!
//! The registry publishes run and VM state transitions and each VM's serial
//! deltas as they happen, and the task ledger publishes every task transition,
//! to the hub's `GET /v1/ws` WebSocket, which sends every event to a connected
//! console as one text message (`{"event": <kind>, "data": <data>}`), so a
//! console sees a run's or a task's life between its polls. Polling stays the source of truth for a full snapshot (the register,
//! the ledger, and a run's whole serial capture); the stream is the deltas.
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
//! event: task
//! data: {"id":"t1","state":"claimed","assignee":"builder","agent_cid":100,"project":"pr1","ready":false}
//!
//! event: task_link
//! data: {"task":"t3","after":["t1","t2"]}
//!
//! event: treasury
//! data: {"entry":1,"kind":"mint","amount":500,"task":"t1"}
//!
//! event: agora
//! data: {"task":"t1","kind":"bid","agent_cid":100,"price":300}
//!
//! event: board
//! data: {"kind":"post","post":{"id":"p1",…},"agent_cid":100,"role":"builder","read_up_to":null}
//!
//! event: conversation
//! data: {"id":"c1","from_cid":100,"to_cid":101,"delivered":true}
//!
//! event: motion
//! data: {"id":"m1","kind":"priority","state":"open"}
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

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// One event, as the console's `GET /v1/ws` socket carries it (its kind and its data).
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
    /// A task changed state (or the task was created, as `open`), or a task it
    /// runs after ended, which makes it ready or blocked: `assignee` is the
    /// schema's `assignee_id` once a claimed agent has one, and `null` while
    /// nobody does; `agent_cid` is the claiming agent's guest CID; `project`
    /// the project the task is for; `ready` whether it can be claimed now.
    Task {
        id: String,
        state: &'static str,
        assignee: Option<String>,
        agent_cid: Option<u64>,
        project: Option<String>,
        ready: bool,
    },
    /// An open task's links changed: `after` is the whole new list of the
    /// tasks it runs after.
    TaskLink { task: String, after: Vec<String> },
    /// A treasury entry was posted: the double-entry journal grew. `entry` is
    /// the entry's id, `kind` the schema's entry kind, `amount` the entry's
    /// size (its credited total, which equals its debited total), and `task`
    /// the task the entry belongs to, or `null` when it belongs to none (a
    /// `fine` or a `fee`).
    Treasury {
        entry: u64,
        kind: &'static str,
        amount: i128,
        task: Option<String>,
    },
    /// A call moved: an agent bid on a roled open task's call (`kind` `bid`,
    /// at `price`), or the call was awarded to a bidder (`kind` `award`, at
    /// the price that bidder offered). `task` is the call — the id of the task
    /// the offer is for — and `agent_cid` the agent that bid or won.
    Agora {
        task: String,
        kind: &'static str,
        agent_cid: u64,
        price: u64,
    },
    /// The board moved: an agent posted (`kind` `post`, with `post` the post's
    /// own JSON object, exactly as `GET /v1/board` writes it) or advanced
    /// its read cursor (`kind` `read`, with `post` `null` and `read_up_to` the
    /// post it moved to). `agent_cid` and `role` are the agent that acted.
    ///
    /// The frame carries more than `docs/agents/social.md`'s `{post,
    /// agent_cid, role}` — a superset by `kind` and `read_up_to`, so a console
    /// can patch a cursor's advance live instead of re-polling the board for
    /// it. The contract's three fields are all here, unchanged.
    Board {
        /// `post` or `read`.
        kind: &'static str,
        /// The post's JSON object as the board endpoint writes it, or `null`
        /// for a read.
        post: Option<String>,
        agent_cid: u64,
        role: String,
        /// The post a read advanced to, or `null` for a post.
        read_up_to: Option<String>,
    },
    /// Two agents spoke: an exchange was recorded. `delivered` says whether the
    /// peer's session took the `conversation` call.
    ///
    /// `social.md` pins `{from_cid, to_cid}`; the frame carries the exchange's
    /// id and its delivery verdict beside them, so a console can patch the
    /// thread without re-reading it.
    Conversation {
        id: String,
        from_cid: u64,
        to_cid: u64,
        delivered: bool,
    },
    /// A council motion moved: it was proposed, or a seat voted on it. The
    /// frame carries the motion's id, its kind and the state it is now in —
    /// **no text**: the motion's text and its votes' reasons are read from the
    /// motions endpoint, which is the only source for them, exactly as the
    /// exchange record is for a conversation's words (`docs/agents/council.md`).
    Motion {
        /// The motion's id (`m<seq>`).
        id: String,
        /// The kind, as the wire spells it.
        kind: &'static str,
        /// The state the motion is now in.
        state: &'static str,
    },
}

impl Event {
    /// The frame's one-line JSON `data`.
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
            Event::Task {
                id,
                state,
                assignee,
                agent_cid,
                project,
                ready,
            } => format!(
                "{{\"id\":{},\"state\":{},\"assignee\":{},\"agent_cid\":{},\"project\":{},\"ready\":{}}}",
                quote(id),
                quote(state),
                match assignee {
                    Some(assignee) => quote(assignee),
                    None => "null".to_string(),
                },
                match agent_cid {
                    Some(cid) => cid.to_string(),
                    None => "null".to_string(),
                },
                match project {
                    Some(project) => quote(project),
                    None => "null".to_string(),
                },
                ready
            ),
            Event::TaskLink { task, after } => format!(
                "{{\"task\":{},\"after\":[{}]}}",
                quote(task),
                after
                    .iter()
                    .map(|id| quote(id))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Event::Treasury {
                entry,
                kind,
                amount,
                task,
            } => format!(
                "{{\"entry\":{},\"kind\":{},\"amount\":{},\"task\":{}}}",
                entry,
                quote(kind),
                amount,
                match task {
                    Some(task) => quote(task),
                    None => "null".to_string(),
                }
            ),
            Event::Agora {
                task,
                kind,
                agent_cid,
                price,
            } => format!(
                "{{\"task\":{},\"kind\":{},\"agent_cid\":{},\"price\":{}}}",
                quote(task),
                quote(kind),
                agent_cid,
                price
            ),
            // The post is embedded as the JSON object it already is — built by
            // the board endpoint's own serializer, so a frame and a poll can
            // never disagree about a post's shape.
            Event::Board {
                kind,
                post,
                agent_cid,
                role,
                read_up_to,
            } => format!(
                "{{\"kind\":{},\"post\":{},\"agent_cid\":{},\"role\":{},\"read_up_to\":{}}}",
                quote(kind),
                post.as_deref().unwrap_or("null"),
                agent_cid,
                quote(role),
                match read_up_to {
                    Some(id) => quote(id),
                    None => "null".to_string(),
                }
            ),
            Event::Conversation {
                id,
                from_cid,
                to_cid,
                delivered,
            } => format!(
                "{{\"id\":{},\"from_cid\":{},\"to_cid\":{},\"delivered\":{}}}",
                quote(id),
                from_cid,
                to_cid,
                delivered
            ),
            Event::Motion { id, kind, state } => format!(
                "{{\"id\":{},\"kind\":{},\"state\":{}}}",
                quote(id),
                quote(kind),
                quote(state)
            ),
        }
    }

    /// The frame's `event` name.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Run { .. } => "run",
            Event::Vm { .. } => "vm",
            Event::Serial { .. } => "serial",
            Event::Task { .. } => "task",
            Event::TaskLink { .. } => "task_link",
            Event::Treasury { .. } => "treasury",
            Event::Agora { .. } => "agora",
            Event::Board { .. } => "board",
            Event::Conversation { .. } => "conversation",
            Event::Motion { .. } => "motion",
        }
    }

    /// The event as one server-sent block, for the text streams that still frame events so (the
    /// hub's `/v1/ws` sends [`Event::kind`] and [`Event::data`] as one JSON message instead).
    pub fn frame(&self) -> String {
        format!("event: {}\ndata: {}\n\n", self.kind(), self.data())
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

// How long a stream waits for an event before it writes a heartbeat
// comment, so a dead connection is noticed (the write fails) instead of
// sitting in the channel's wait forever.

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
        assert_eq!(
            Event::Task {
                id: "t1".to_string(),
                state: "open",
                assignee: None,
                agent_cid: None,
                project: None,
                ready: true,
            }
            .frame(),
            "event: task\ndata: {\"id\":\"t1\",\"state\":\"open\",\"assignee\":null,\"agent_cid\":null,\"project\":null,\"ready\":true}\n\n"
        );
        assert_eq!(
            Event::Task {
                id: "t1".to_string(),
                state: "claimed",
                assignee: Some("builder".to_string()),
                agent_cid: Some(100),
                project: Some("pr1".to_string()),
                ready: false,
            }
            .frame(),
            "event: task\ndata: {\"id\":\"t1\",\"state\":\"claimed\",\"assignee\":\"builder\",\"agent_cid\":100,\"project\":\"pr1\",\"ready\":false}\n\n"
        );
        assert_eq!(
            Event::TaskLink {
                task: "t3".to_string(),
                after: vec!["t1".to_string(), "t2".to_string()],
            }
            .frame(),
            "event: task_link\ndata: {\"task\":\"t3\",\"after\":[\"t1\",\"t2\"]}\n\n"
        );
        assert_eq!(
            Event::TaskLink {
                task: "t3".to_string(),
                after: Vec::new(),
            }
            .data(),
            "{\"task\":\"t3\",\"after\":[]}"
        );
        assert_eq!(
            Event::Treasury {
                entry: 1,
                kind: "mint",
                amount: 500,
                task: Some("t1".to_string()),
            }
            .frame(),
            "event: treasury\ndata: {\"entry\":1,\"kind\":\"mint\",\"amount\":500,\"task\":\"t1\"}\n\n"
        );
        assert_eq!(
            Event::Treasury {
                entry: 7,
                kind: "fine",
                amount: 5,
                task: None,
            }
            .frame(),
            "event: treasury\ndata: {\"entry\":7,\"kind\":\"fine\",\"amount\":5,\"task\":null}\n\n"
        );
        assert_eq!(
            Event::Agora {
                task: "t1".to_string(),
                kind: "bid",
                agent_cid: 100,
                price: 300,
            }
            .frame(),
            "event: agora\ndata: {\"task\":\"t1\",\"kind\":\"bid\",\"agent_cid\":100,\"price\":300}\n\n"
        );
        // An award frame carries the price the award was made at.
        assert_eq!(
            Event::Agora {
                task: "t1".to_string(),
                kind: "award",
                agent_cid: 101,
                price: 0,
            }
            .frame(),
            "event: agora\ndata: {\"task\":\"t1\",\"kind\":\"award\",\"agent_cid\":101,\"price\":0}\n\n"
        );
        // A board frame carries the post's own JSON object, the agent that
        // acted and its role; a read carries the cursor instead of a post.
        assert_eq!(
            Event::Board {
                kind: "post",
                post: Some("{\"id\":\"p1\"}".to_string()),
                agent_cid: 100,
                role: "builder".to_string(),
                read_up_to: None,
            }
            .frame(),
            "event: board\ndata: {\"kind\":\"post\",\"post\":{\"id\":\"p1\"},\"agent_cid\":100,\"role\":\"builder\",\"read_up_to\":null}\n\n"
        );
        assert_eq!(
            Event::Board {
                kind: "read",
                post: None,
                agent_cid: 101,
                role: "librarian".to_string(),
                read_up_to: Some("p3".to_string()),
            }
            .frame(),
            "event: board\ndata: {\"kind\":\"read\",\"post\":null,\"agent_cid\":101,\"role\":\"librarian\",\"read_up_to\":\"p3\"}\n\n"
        );
        assert_eq!(
            Event::Conversation {
                id: "c1".to_string(),
                from_cid: 100,
                to_cid: 101,
                delivered: false,
            }
            .frame(),
            "event: conversation\ndata: {\"id\":\"c1\",\"from_cid\":100,\"to_cid\":101,\"delivered\":false}\n\n"
        );
        // A motion frame carries the motion's id, its kind and its state, and
        // no text at all.
        assert_eq!(
            Event::Motion {
                id: "m1".to_string(),
                kind: "budget_move",
                state: "open",
            }
            .frame(),
            "event: motion\ndata: {\"id\":\"m1\",\"kind\":\"budget_move\",\"state\":\"open\"}\n\n"
        );
        assert_eq!(
            Event::Motion {
                id: "m2".to_string(),
                kind: "law_proposal",
                state: "awaiting_owner",
            }
            .frame(),
            "event: motion\ndata: {\"id\":\"m2\",\"kind\":\"law_proposal\",\"state\":\"awaiting_owner\"}\n\n"
        );
    }

    #[test]
    fn a_frame_escapes_every_string_it_carries_into_the_json() {
        // Was a_board_frame_escapes_the_role_it_names: a board frame escapes the role it names.
        {
            // The role and the cursor are the store's strings, but a frame is built
            // from whatever they carry: nothing here may break out of the frame.
            let frame = Event::Board {
                kind: "read",
                post: None,
                agent_cid: 100,
                role: "a\"b\\c\nd".to_string(),
                read_up_to: Some("p\"1".to_string()),
            }
            .frame();
            assert!(
                frame.contains("\"role\":\"a\\\"b\\\\c\\nd\""),
                "the frame escapes the role: {frame}"
            );
            assert!(frame.contains("\"read_up_to\":\"p\\\"1\""), "{frame}");
            assert_eq!(frame.matches("data: ").count(), 1);
        }
        // Was a_role_name_is_escaped_into_the_json_string: a task frame escapes its assignee.
        {
            // A role is the manifest's, but a frame is built from whatever string
            // the identity carries: nothing here may break out of the frame.
            let frame = Event::Task {
                id: "t1".to_string(),
                state: "claimed",
                assignee: Some("a\"b\\c\nd".to_string()),
                agent_cid: Some(7),
                project: Some("p\"r\n1".to_string()),
                ready: false,
            }
            .frame();
            assert!(
                frame.contains("\"assignee\":\"a\\\"b\\\\c\\nd\""),
                "the frame escapes the assignee: {frame}"
            );
            assert!(
                frame.contains("\"project\":\"p\\\"r\\n1\""),
                "the frame escapes the project: {frame}"
            );
            assert_eq!(frame.matches("data: ").count(), 1);
        }
        // Was guest_text_is_escaped_into_the_json_string: serial guest text is escaped.
        {
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
            live.recv_timeout(std::time::Duration::from_secs(1)),
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
            live.recv_timeout(std::time::Duration::from_secs(1)),
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
}
