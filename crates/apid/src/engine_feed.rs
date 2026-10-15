//! What an ACP engine's stream leaves in the transcript. Every `engine_update` goes out live on the
//! bus; the transcript keeps the updates that explain a turn: a tool call and its status changes,
//! the agent's final message of each message, the latest plan and usage at the end, and mode changes.
//! The text chunks that stream past are not recorded, and each engine call's turn is capped, so a
//! chatty engine cannot fill the transcript.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use apz_proto::wire::cap_output;
use serde_json::{Value, json};

/// The transcript entries one engine call may add. The next one is a `truncated` marker, and the
/// rest of that call's updates are only published.
pub const TURN_ENTRIES: usize = 400;

/// The most of one update the transcript keeps, in bytes; a bigger one is cut head and tail.
const UPDATE_BYTES: usize = 16_000;

/// One engine call's buffered state: the text not yet recorded, the latest plan and usage, and how
/// many entries it has added.
#[derive(Default)]
struct Turn {
    /// The agent's text, per message id, in arrival order.
    messages: Vec<(Option<String>, String)>,
    plan: Option<Value>,
    usage: Option<Value>,
    entries: usize,
    truncated: bool,
}

impl Turn {
    /// Adds one entry, or the single `truncated` marker once the turn is full.
    fn admit(&mut self, call: &str, out: &mut Vec<Value>, entry: Value) {
        if self.entries < TURN_ENTRIES {
            self.entries += 1;
            out.push(entry);
        } else if !self.truncated {
            self.truncated = true;
            out.push(json!({ "call": call, "item": "truncated", "after": TURN_ENTRIES }));
        }
    }

    /// Records the buffered text, in order, as one entry per message.
    fn flush_messages(&mut self, call: &str, out: &mut Vec<Value>) {
        for (message_id, text) in std::mem::take(&mut self.messages) {
            let entry = json!({
                "call": call,
                "item": "message",
                "message_id": message_id,
                "text": cap_output(&text, UPDATE_BYTES),
            });
            self.admit(call, out, entry);
        }
    }

    /// Records the latest plan and usage (the turn's end), then anything still buffered.
    fn finish(&mut self, call: &str, out: &mut Vec<Value>) {
        self.flush_messages(call, out);
        if let Some(plan) = self.plan.take() {
            self.admit(
                call,
                out,
                json!({ "call": call, "item": "plan", "update": capped(&plan) }),
            );
        }
        if let Some(usage) = self.usage.take() {
            self.admit(
                call,
                out,
                json!({ "call": call, "item": "usage", "update": capped(&usage) }),
            );
        }
    }
}

/// The turns of every engine call, keyed by (agent, work item, call).
#[derive(Default)]
pub struct Feed {
    turns: Mutex<HashMap<(u64, u64, String), Turn>>,
}

impl Feed {
    fn locked(&self) -> MutexGuard<'_, HashMap<(u64, u64, String), Turn>> {
        self.turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Takes one ACP session update of engine call `call` and returns the transcript entries (kind
    /// `engine_update`) it adds now. Text is held until the message's next non-text update, the plan
    /// and usage until [`Feed::finish`].
    pub fn absorb(&self, cid: u64, work: u64, call: &str, update: &Value) -> Vec<Value> {
        let mut turns = self.locked();
        let turn = turns.entry((cid, work, call.to_string())).or_default();
        let mut out = Vec::new();
        match update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("")
        {
            "agent_message_chunk" => {
                let message_id = update
                    .get("messageId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let text = update
                    .pointer("/content/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match turn.messages.last_mut() {
                    Some((id, buffered)) if *id == message_id => buffered.push_str(text),
                    _ => turn.messages.push((message_id, text.to_string())),
                }
            }
            "plan" => turn.plan = Some(update.clone()),
            "usage_update" => turn.usage = Some(update.clone()),
            "tool_call" => {
                turn.flush_messages(call, &mut out);
                let entry = json!({ "call": call, "item": "tool_call", "update": capped(update) });
                turn.admit(call, &mut out, entry);
            }
            // A status change only: an update that adds content but leaves the status alone is bus-only.
            "tool_call_update" if update.get("status").is_some() => {
                turn.flush_messages(call, &mut out);
                let entry =
                    json!({ "call": call, "item": "tool_status", "update": capped(update) });
                turn.admit(call, &mut out, entry);
            }
            "current_mode_update" => {
                turn.flush_messages(call, &mut out);
                let entry = json!({ "call": call, "item": "mode", "update": capped(update) });
                turn.admit(call, &mut out, entry);
            }
            // Thoughts, the user's echoed text and the rest are for the live view only.
            _ => {}
        }
        out
    }

    /// Ends every engine call's turn under work item `work` of agent `cid`: the buffered text, the
    /// latest plan and usage are recorded, and the turn is forgotten.
    pub fn finish(&self, cid: u64, work: u64) -> Vec<Value> {
        let mut turns = self.locked();
        let ended: Vec<String> = turns
            .keys()
            .filter(|(c, w, _)| *c == cid && *w == work)
            .map(|(_, _, call)| call.clone())
            .collect();
        let mut out = Vec::new();
        for call in ended {
            if let Some(mut turn) = turns.remove(&(cid, work, call.clone())) {
                turn.finish(&call, &mut out);
            }
        }
        out
    }
}

/// The update as the transcript keeps it: whole when small, else cut (its JSON text, head and tail).
fn capped(update: &Value) -> Value {
    let text = update.to_string();
    if text.len() <= UPDATE_BYTES {
        update.clone()
    } else {
        json!({ "truncated": cap_output(&text, UPDATE_BYTES) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, text: &str) -> Value {
        json!({
            "sessionUpdate": "agent_message_chunk",
            "messageId": id,
            "content": { "type": "text", "text": text },
        })
    }

    fn items(entries: &[Value]) -> Vec<&str> {
        entries
            .iter()
            .map(|e| e["item"].as_str().unwrap_or(""))
            .collect()
    }

    #[test]
    fn chunks_are_held_and_the_message_is_recorded_once_when_the_turn_ends() {
        let feed = Feed::default();
        assert!(feed.absorb(3, 7, "toolu_1", &chunk("m1", "Hel")).is_empty());
        assert!(feed.absorb(3, 7, "toolu_1", &chunk("m1", "lo")).is_empty());
        assert!(
            feed.absorb(3, 7, "toolu_1", &chunk("m2", "again"))
                .is_empty()
        );
        let recorded = feed.finish(3, 7);
        assert_eq!(items(&recorded), ["message", "message"]);
        assert_eq!(recorded[0]["text"], "Hello");
        assert_eq!(recorded[0]["message_id"], "m1");
        assert_eq!(recorded[1]["text"], "again");
        // The turn is forgotten: a second finish has nothing to add.
        assert!(feed.finish(3, 7).is_empty());
    }

    #[test]
    fn thoughts_and_content_only_updates_are_not_recorded() {
        let feed = Feed::default();
        let thought = json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hmm"}});
        assert!(feed.absorb(1, 1, "c", &thought).is_empty());
        let content_only =
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t", "content": []});
        assert!(feed.absorb(1, 1, "c", &content_only).is_empty());
        assert!(feed.finish(1, 1).is_empty());
    }

    #[test]
    fn a_tool_call_and_its_status_changes_are_recorded_after_the_text_before_them() {
        let feed = Feed::default();
        feed.absorb(1, 2, "c", &chunk("m", "Editing."));
        let call = json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "edit", "status": "pending"});
        let status =
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"});
        let recorded = feed.absorb(1, 2, "c", &call);
        assert_eq!(items(&recorded), ["message", "tool_call"]);
        assert_eq!(recorded[1]["update"]["title"], "edit");
        let recorded = feed.absorb(1, 2, "c", &status);
        assert_eq!(items(&recorded), ["tool_status"]);
        assert_eq!(recorded[0]["update"]["status"], "completed");
    }

    #[test]
    fn the_plan_and_usage_are_latest_only_and_recorded_at_the_end() {
        let feed = Feed::default();
        let plan = |n: u32| json!({"sessionUpdate": "plan", "entries": [{"content": format!("step {n}")}]});
        let usage = |n: u64| json!({"sessionUpdate": "usage_update", "used": n, "size": 200000});
        for n in 1..=3 {
            assert!(feed.absorb(4, 5, "c", &plan(n)).is_empty());
            assert!(feed.absorb(4, 5, "c", &usage(n as u64 * 10)).is_empty());
        }
        let recorded = feed.finish(4, 5);
        assert_eq!(items(&recorded), ["plan", "usage"]);
        assert_eq!(recorded[0]["update"]["entries"][0]["content"], "step 3");
        assert_eq!(recorded[1]["update"]["used"], 30);
    }

    #[test]
    fn a_turn_is_capped_and_then_marked_truncated_once() {
        let feed = Feed::default();
        let call = json!({"sessionUpdate": "tool_call", "toolCallId": "t", "title": "x"});
        let mut recorded = Vec::new();
        for _ in 0..(TURN_ENTRIES + 25) {
            recorded.extend(feed.absorb(9, 9, "c", &call));
        }
        assert_eq!(recorded.len(), TURN_ENTRIES + 1);
        assert_eq!(recorded.last().unwrap()["item"], "truncated");
        // Another call's turn has its own budget.
        assert_eq!(feed.absorb(9, 9, "other", &call).len(), 1);
    }

    #[test]
    fn finish_ends_only_the_given_work_item_of_the_given_agent() {
        let feed = Feed::default();
        feed.absorb(1, 10, "c", &chunk("m", "mine"));
        feed.absorb(1, 11, "c", &chunk("m", "later"));
        feed.absorb(2, 10, "c", &chunk("m", "other agent"));
        let ended = feed.finish(1, 10);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0]["text"], "mine");
        assert_eq!(feed.finish(1, 11).len(), 1);
        assert_eq!(feed.finish(2, 10).len(), 1);
    }

    #[test]
    fn a_large_update_is_cut_for_the_transcript() {
        let big = json!({"sessionUpdate": "tool_call", "title": "y".repeat(3 * UPDATE_BYTES)});
        let feed = Feed::default();
        let recorded = feed.absorb(1, 1, "c", &big);
        let update = &recorded[0]["update"];
        assert!(update.get("truncated").is_some(), "{update}");
        assert!(update["truncated"].as_str().unwrap().len() <= UPDATE_BYTES + 64);
    }
}
