//! An engine's `session/update` stream: forwarded to the hub as `engine_update`s, and remembered
//! for the turn's summary.
//!
//! Text chunks (`agent_message_chunk`, `agent_thought_chunk`) are coalesced per kind and message
//! for at least [`COALESCE`] before they go out, so a fast engine sends a few lines a second rather
//! than one per token. Every other update goes out at once, after any buffered text, so the order
//! the hub sees is the engine's. Each update is capped to [`UPDATE_BYTES`]
//! (`apz_acp::map::cap_update`) and numbered from 1 (`seq`).

use std::time::{Duration, Instant};

use serde_json::Value;

use super::link::EngineLink;

/// How long text chunks are gathered before they go out.
pub const COALESCE: Duration = Duration::from_millis(100);
/// The most one forwarded update may hold, in bytes.
pub const UPDATE_BYTES: usize = 64 * 1024;

/// One tool call the engine made this turn, as its last update left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallSeen {
    pub id: String,
    pub title: String,
    /// The ACP kind (`read`, `edit`, `execute`, …), `other` when the engine gave none.
    pub kind: String,
    /// `pending`, `in_progress`, `completed` or `failed`.
    pub status: String,
}

/// What the summary needs from one prompt turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    /// The agent's text this turn (every `agent_message_chunk`, in order); thoughts are not kept.
    pub message: String,
    pub tool_calls: Vec<ToolCallSeen>,
    /// The latest `plan` update's entries.
    pub plan: Option<Value>,
    /// The latest `usage_update`.
    pub usage: Option<Value>,
    /// The latest `current_mode_update`'s mode id.
    pub mode: Option<String>,
}

/// The text chunks gathered so far. `update` is the first chunk as the engine sent it: its other
/// fields (kind, messageId) are the ones the merged update keeps.
struct Buffer {
    update: Value,
    text: String,
    started: Instant,
}

/// One engine session's stream.
pub struct Stream<'a> {
    link: &'a dyn EngineLink,
    engine: String,
    session: String,
    turn: Turn,
    buffer: Option<Buffer>,
}

impl<'a> Stream<'a> {
    pub fn new(link: &'a dyn EngineLink, engine: &str, session: &str) -> Self {
        Self {
            link,
            engine: engine.to_string(),
            session: session.to_string(),
            turn: Turn::default(),
            buffer: None,
        }
    }

    /// Takes the `update` of one `session/update` notification: buffers or forwards it, and keeps
    /// what [`Turn`] needs.
    pub fn absorb(&mut self, update: Value) {
        self.record(&update);
        let text = match session_update(&update) {
            "agent_message_chunk" | "agent_thought_chunk" => text_of(&update).map(str::to_string),
            _ => None,
        };
        match text {
            Some(text) => self.buffer_text(update, text),
            None => {
                self.flush();
                self.send(update);
            }
        }
    }

    /// True when buffered text has waited [`COALESCE`] or longer as of `now`.
    pub fn due(&self, now: Instant) -> bool {
        self.buffer
            .as_ref()
            .is_some_and(|buffer| now.saturating_duration_since(buffer.started) >= COALESCE)
    }

    /// Sends the buffered text, if any.
    pub fn flush(&mut self) {
        if let Some(Buffer {
            mut update, text, ..
        }) = self.buffer.take()
        {
            if let Some(slot) = update.pointer_mut("/content/text") {
                *slot = Value::String(text);
            }
            self.send(update);
        }
    }

    /// The turn so far.
    pub fn turn(&self) -> &Turn {
        &self.turn
    }

    /// Appends a text chunk to the buffer when it continues the buffered stream (same kind and
    /// message); otherwise the buffer goes out and this chunk starts a new one.
    fn buffer_text(&mut self, update: Value, text: String) {
        if let Some(buffer) = self.buffer.as_mut()
            && same_stream(&buffer.update, &update)
        {
            buffer.text.push_str(&text);
            return;
        }
        self.flush();
        self.buffer = Some(Buffer {
            update,
            text,
            started: Instant::now(),
        });
    }

    /// Keeps what the turn's summary needs from one update.
    fn record(&mut self, update: &Value) {
        match session_update(update) {
            "agent_message_chunk" => {
                if let Some(text) = text_of(update) {
                    self.turn.message.push_str(text);
                }
            }
            "tool_call" | "tool_call_update" => {
                if let Some(id) = update.get("toolCallId").and_then(Value::as_str) {
                    apply_tool_fields(self.tool_call(id), update);
                }
            }
            "plan" => {
                if let Some(entries) = update.get("entries") {
                    self.turn.plan = Some(entries.clone());
                }
            }
            "usage_update" => self.turn.usage = Some(update.clone()),
            "current_mode_update" => {
                if let Some(mode) = update.get("currentModeId").and_then(Value::as_str) {
                    self.turn.mode = Some(mode.to_string());
                }
            }
            _ => {}
        }
    }

    /// The turn's entry for tool call `id`, added as `other`/`pending` when the engine has not
    /// announced it (an update may come first).
    fn tool_call(&mut self, id: &str) -> &mut ToolCallSeen {
        let position = match self.turn.tool_calls.iter().position(|call| call.id == id) {
            Some(position) => position,
            None => {
                self.turn.tool_calls.push(ToolCallSeen {
                    id: id.to_string(),
                    title: String::new(),
                    kind: "other".to_string(),
                    status: "pending".to_string(),
                });
                self.turn.tool_calls.len() - 1
            }
        };
        &mut self.turn.tool_calls[position]
    }

    /// Sends one update to the hub, capped.
    fn send(&self, update: Value) {
        let capped = apz_acp::map::cap_update(update, UPDATE_BYTES);
        self.link.update(&self.engine, &self.session, &capped);
    }
}

/// The `sessionUpdate` discriminator, empty when the update has none.
fn session_update(update: &Value) -> &str {
    update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The text of a `content` block; `None` for images, resources and anything else without text.
fn text_of(update: &Value) -> Option<&str> {
    update.pointer("/content/text").and_then(Value::as_str)
}

/// Whether `next` continues the chunk stream `buffered` is gathering: same kind and same message.
fn same_stream(buffered: &Value, next: &Value) -> bool {
    session_update(buffered) == session_update(next) && message_id(buffered) == message_id(next)
}

fn message_id(update: &Value) -> Option<&str> {
    update.get("messageId").and_then(Value::as_str)
}

/// Overwrites the fields the update carries (`title`, `kind`, `status`); absent ones keep their
/// value.
fn apply_tool_fields(call: &mut ToolCallSeen, update: &Value) {
    if let Some(title) = update.get("title").and_then(Value::as_str) {
        call.title = title.to_string();
    }
    if let Some(kind) = update.get("kind").and_then(Value::as_str) {
        call.kind = kind.to_string();
    }
    if let Some(status) = update.get("status").and_then(Value::as_str) {
        call.status = status.to_string();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::acp::link::Verdict;

    /// Records every update the stream sends, with the engine and session it names.
    #[derive(Default)]
    struct Recorder {
        sent: Mutex<Vec<(String, String, Value)>>,
    }

    impl Recorder {
        fn updates(&self) -> Vec<Value> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .map(|(_, _, update)| update.clone())
                .collect()
        }
    }

    impl EngineLink for Recorder {
        fn decide(&self, _name: &str, _input: &Value, _deadline: Instant) -> Verdict {
            Verdict::Allow
        }

        fn update(&self, engine: &str, session: &str, update: &Value) {
            self.sent.lock().unwrap().push((
                engine.to_string(),
                session.to_string(),
                update.clone(),
            ));
        }

        fn cancelled(&self) -> bool {
            false
        }
    }

    fn chunk(kind: &str, message_id: &str, text: &str) -> Value {
        json!({
            "sessionUpdate": kind,
            "messageId": message_id,
            "content": {"type": "text", "text": text},
        })
    }

    #[test]
    fn five_chunks_of_one_message_go_out_as_one_update_on_flush() {
        let link = Recorder::default();
        let mut stream = Stream::new(&link, "claude", "s1");
        for part in ["Hel", "lo ", "wor", "ld", "!"] {
            stream.absorb(chunk("agent_message_chunk", "m1", part));
        }
        assert!(link.updates().is_empty());
        assert_eq!(stream.turn().message, "Hello world!");

        stream.flush();
        let sent = link.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "claude");
        assert_eq!(sent[0].1, "s1");
        assert_eq!(
            sent[0].2,
            chunk("agent_message_chunk", "m1", "Hello world!")
        );
    }

    #[test]
    fn a_tool_call_flushes_the_text_before_it() {
        let link = Recorder::default();
        let mut stream = Stream::new(&link, "claude", "s1");
        stream.absorb(chunk("agent_message_chunk", "m1", "Let me look."));
        stream.absorb(json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read"}));
        stream.absorb(chunk("agent_message_chunk", "m1", " Done."));
        stream.flush();

        let sent = link.updates();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0]["content"]["text"], "Let me look.");
        assert_eq!(sent[1]["sessionUpdate"], "tool_call");
        assert_eq!(sent[2]["content"]["text"], " Done.");
    }

    #[test]
    fn thoughts_and_messages_and_other_messages_are_not_merged() {
        let link = Recorder::default();
        let mut stream = Stream::new(&link, "claude", "s1");
        stream.absorb(chunk("agent_message_chunk", "m1", "a"));
        stream.absorb(chunk("agent_thought_chunk", "m1", "b"));
        stream.absorb(chunk("agent_message_chunk", "m1", "c"));
        stream.absorb(chunk("agent_message_chunk", "m2", "d"));
        stream.flush();

        let sent = link.updates();
        let texts: Vec<(&str, &str)> = sent
            .iter()
            .map(|update| {
                (
                    update["sessionUpdate"].as_str().unwrap(),
                    update["content"]["text"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            texts,
            [
                ("agent_message_chunk", "a"),
                ("agent_thought_chunk", "b"),
                ("agent_message_chunk", "c"),
                ("agent_message_chunk", "d"),
            ]
        );
        // Thoughts are not part of the turn's message.
        assert_eq!(stream.turn().message, "acd");
    }

    #[test]
    fn a_tool_call_and_its_update_leave_one_entry_with_the_latest_status() {
        let link = Recorder::default();
        let mut stream = Stream::new(&link, "claude", "s1");
        stream.absorb(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "title": "Read src/lib.rs",
            "kind": "read",
            "status": "pending",
        }));
        stream.absorb(
            json!({"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "Run tests"}),
        );
        stream.absorb(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
        }));

        assert_eq!(
            stream.turn().tool_calls,
            [
                ToolCallSeen {
                    id: "t1".to_string(),
                    title: "Read src/lib.rs".to_string(),
                    kind: "read".to_string(),
                    status: "completed".to_string(),
                },
                ToolCallSeen {
                    id: "t2".to_string(),
                    title: "Run tests".to_string(),
                    kind: "other".to_string(),
                    status: "pending".to_string(),
                },
            ]
        );
        // Non-chunk updates go out at once.
        assert_eq!(link.updates().len(), 3);
    }

    #[test]
    fn plan_usage_and_mode_are_kept_and_other_updates_pass_through() {
        let link = Recorder::default();
        let mut stream = Stream::new(&link, "claude", "s1");
        stream.absorb(json!({
            "sessionUpdate": "plan",
            "entries": [{"content": "read", "status": "completed"}],
        }));
        let plan = json!({
            "sessionUpdate": "plan",
            "entries": [
                {"content": "read", "status": "completed"},
                {"content": "edit", "status": "pending"},
            ],
        });
        stream.absorb(plan.clone());
        let usage = json!({"sessionUpdate": "usage_update", "used": 1200, "size": 200000});
        stream.absorb(usage.clone());
        stream.absorb(json!({"sessionUpdate": "current_mode_update", "currentModeId": "code"}));
        let commands =
            json!({"sessionUpdate": "available_commands_update", "availableCommands": []});
        stream.absorb(commands.clone());

        let turn = stream.turn();
        assert_eq!(turn.plan, Some(plan["entries"].clone()));
        assert_eq!(turn.usage, Some(usage.clone()));
        assert_eq!(turn.mode.as_deref(), Some("code"));
        assert_eq!(
            link.updates(),
            [
                json!({"sessionUpdate": "plan", "entries": [{"content": "read", "status": "completed"}]}),
                plan,
                usage,
                json!({"sessionUpdate": "current_mode_update", "currentModeId": "code"}),
                commands
            ]
        );
    }

    #[test]
    fn a_large_raw_input_is_capped_before_it_is_sent() {
        let link = Recorder::default();
        let mut stream = Stream::new(&link, "claude", "s1");
        let big = "x".repeat(200 * 1024);
        stream.absorb(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "title": "Write",
            "rawInput": {"content": big},
        }));

        let sent = link.updates();
        let content = sent[0]["rawInput"]["content"].as_str().unwrap();
        assert!(content.len() > UPDATE_BYTES);
        assert!(content.len() < UPDATE_BYTES + 64);
        assert!(content.contains("[truncated"));
    }

    #[test]
    fn due_once_the_buffered_text_has_waited_the_coalesce_window() {
        let link = Recorder::default();
        let mut stream = Stream::new(&link, "claude", "s1");
        assert!(!stream.due(Instant::now()));

        stream.absorb(chunk("agent_message_chunk", "m1", "a"));
        let started = stream.buffer.as_ref().unwrap().started;
        assert!(!stream.due(started));
        assert!(!stream.due(started + COALESCE - Duration::from_millis(1)));
        assert!(stream.due(started + COALESCE));

        stream.flush();
        assert!(!stream.due(started + COALESCE));
    }
}
