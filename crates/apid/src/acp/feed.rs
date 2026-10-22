//! A session's work as ACP `session/update`s.
//!
//! Live, [`Feed::frame`] reads the hub's bus frames for one work item:
//! - `delta {cid, work, req, delta}`: the agent's model stream → `agent_message_chunk` (text) or
//!   `agent_thought_chunk` (thinking);
//! - `transcript` entries `{cid, work, kind, data}`: `tool_request` → `tool_call` (kind from the
//!   tool's law operation, `apz_acp::map::tool_kind_for_operation`, title from the tool, raw input
//!   capped), `tool_decision` → `tool_call_update` (`in_progress`, or `failed` with the reason),
//!   `tool_result`/`tool_report` → `tool_call_update` (`completed` or `failed`, output as text
//!   content, capped), a `todo_write` call → `plan`, `model_response` → nothing (its text already
//!   streamed as deltas), `engine_update` entries → nothing (the live `engine` frames carry them);
//! - `engine {cid, work, call, engine, update}`: an engine's own stream under its `code_engine`
//!   call: its `tool_call`/`tool_call_update` re-keyed `<call>/<id>` (so they never collide with the
//!   plaza's ids), its `plan` as the session's plan, its text chunks as `agent_thought_chunk`.
//!
//! [`replay`] gives the same updates from a finished work item's transcript entries, for
//! `session/load` (text from `model_response` instead of deltas).

use std::collections::HashSet;

use apz_acp::map::{cap_update, tool_kind_for_operation};
use serde_json::{Value, json};

use crate::transcript::StoredEntry;

/// The most of a tool's input or output an update carries, in bytes.
pub const CONTENT_BYTES: usize = 16 * 1024;

/// How a work item ended, from its `work_end` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
    /// `done`, `failed` or `stopped`.
    pub status: String,
    pub summary: String,
}

/// The live feed of one work item.
pub struct Feed {
    cid: u64,
    work: u64,
    /// The calls the editor sees no tool updates for: `todo_write` (its list is the plan) and an
    /// engine's own checks (`engine.*`; the engine's frames already show that tool call, re-keyed).
    /// Their decisions and results must not reach the editor as orphan tool calls.
    hidden: HashSet<String>,
}

impl Feed {
    pub fn new(cid: u64, work: u64) -> Self {
        Self {
            cid,
            work,
            hidden: HashSet::new(),
        }
    }

    /// The updates one bus frame (`kind`, its JSON `data`) gives this work item; none for another
    /// agent's or another work item's frames.
    pub fn frame(&mut self, kind: &str, data: &Value) -> Vec<Value> {
        if !self.is_mine(data) {
            return Vec::new();
        }
        let updates = match kind {
            "delta" => delta_update(&data["delta"]).into_iter().collect(),
            "transcript" => self.entry(str_of(data, "kind"), &data["data"], false),
            "engine" => engine_frame(str_of(data, "call"), &data["update"])
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        capped(updates)
    }

    /// The work item's end, when this frame is its `work_end` transcript entry.
    pub fn end(&self, kind: &str, data: &Value) -> Option<End> {
        if kind != "transcript" || data["kind"].as_str() != Some("work_end") || !self.is_mine(data)
        {
            return None;
        }
        Some(End {
            status: data["data"]["status"].as_str()?.to_string(),
            summary: str_of(&data["data"], "summary").to_string(),
        })
    }

    fn is_mine(&self, data: &Value) -> bool {
        data["cid"].as_u64() == Some(self.cid) && data["work"].as_u64() == Some(self.work)
    }

    /// One transcript entry's updates. Live, only the tool entries speak; replayed, the prompt,
    /// the response's text and the engine's recorded updates speak too, since no delta or engine
    /// frame carries them then.
    fn entry(&mut self, kind: &str, data: &Value, replaying: bool) -> Vec<Value> {
        match kind {
            "tool_request" => self.request(data),
            "tool_decision" => self.decision(data),
            "tool_result" => self.result(data),
            "work_start" if replaying => data["prompt"]
                .as_str()
                .map(|prompt| vec![text_update("user_message_chunk", prompt)])
                .unwrap_or_default(),
            "model_response" if replaying => {
                let text = response_text(&data["content"]);
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![text_update("agent_message_chunk", &text)]
                }
            }
            "engine_update" if replaying => engine_item(data).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    fn request(&mut self, data: &Value) -> Vec<Value> {
        let call = str_of(data, "call");
        let name = str_of(data, "name");
        let input = &data["input"];
        if name == "todo_write" {
            self.hidden.insert(call.to_string());
            return vec![plan(input)];
        }
        if name.starts_with("engine.") {
            self.hidden.insert(call.to_string());
            return Vec::new();
        }
        vec![json!({
            "sessionUpdate": "tool_call",
            "toolCallId": call,
            "title": name,
            "kind": tool_kind(name),
            "status": "pending",
            "rawInput": input,
        })]
    }

    fn decision(&mut self, data: &Value) -> Vec<Value> {
        let call = str_of(data, "call");
        if self.hidden.contains(call) {
            return Vec::new();
        }
        // `done` is a host tool that ran and returned its output, so it is final here.
        let status = match data["decision"].as_str() {
            Some("allow") | Some("pending") => "in_progress",
            Some("deny") => "failed",
            Some("done") => "completed",
            _ => return Vec::new(),
        };
        vec![call_update(call, status, str_of(data, "output"))]
    }

    fn result(&mut self, data: &Value) -> Vec<Value> {
        let call = str_of(data, "call");
        if self.hidden.contains(call) {
            return Vec::new();
        }
        let status = if data["ok"].as_bool().unwrap_or(false) {
            "completed"
        } else {
            "failed"
        };
        vec![call_update(call, status, str_of(data, "output"))]
    }
}

/// The updates that replay one work item from its transcript entries (other works' are skipped),
/// starting with its prompt as a `user_message_chunk`.
pub fn replay(work: u64, entries: &[StoredEntry]) -> Vec<Value> {
    let frames: Vec<Value> = entries
        .iter()
        .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
        .collect();
    replay_frames(work, &frames)
}

/// [`replay`] over the entries as the bus carries them: each frame is `{cid, work, kind, data, ...}`.
fn replay_frames(work: u64, frames: &[Value]) -> Vec<Value> {
    let mut state: Option<Feed> = None;
    let mut updates = Vec::new();
    for frame in frames
        .iter()
        .filter(|frame| frame["work"].as_u64() == Some(work))
    {
        let cid = frame["cid"].as_u64().unwrap_or_default();
        let feed = state.get_or_insert_with(|| Feed::new(cid, work));
        if cid == feed.cid {
            updates.extend(feed.entry(str_of(frame, "kind"), &frame["data"], true));
        }
    }
    capped(updates)
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn capped(updates: Vec<Value>) -> Vec<Value> {
    updates
        .into_iter()
        .map(|update| cap_update(update, CONTENT_BYTES))
        .collect()
}

fn text_update(session_update: &str, text: &str) -> Value {
    json!({ "sessionUpdate": session_update, "content": { "type": "text", "text": text } })
}

/// The tool's kind for the editor, from the law operation the plaza checks the call as.
fn tool_kind(name: &str) -> apz_acp::types::ToolKind {
    let operation = match name {
        "bash" | "bash_start" | "code_engine" => "tool.exec",
        "fetch_url" | "download" => "tool.fetch",
        _ if crate::laws::risk_class(name) == "read" => "tool.read",
        _ => "tool.write",
    };
    tool_kind_for_operation(operation)
}

/// A `tool_call_update`; the output is shown as text and raw, and an empty one adds nothing.
fn call_update(call: &str, status: &str, output: &str) -> Value {
    let mut update = json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": call,
        "status": status,
    });
    if !output.is_empty() {
        update["content"] =
            json!([{ "type": "content", "content": { "type": "text", "text": output } }]);
        update["rawOutput"] = json!(output);
    }
    update
}

/// The todo list as the session's plan: `todo_write`'s todos, in order.
fn plan(input: &Value) -> Value {
    let entries: Vec<Value> = input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|todo| {
            json!({
                "content": str_of(todo, "content"),
                "priority": "medium",
                "status": plan_status(str_of(todo, "status")),
            })
        })
        .collect();
    json!({ "sessionUpdate": "plan", "entries": entries })
}

fn plan_status(status: &str) -> &'static str {
    match status {
        "in_progress" => "in_progress",
        "completed" => "completed",
        _ => "pending",
    }
}

/// A model delta as a chunk; a tool-use delta is shown by its tool call instead.
fn delta_update(delta: &Value) -> Option<Value> {
    let text = delta["text"].as_str()?;
    match delta["type"].as_str()? {
        "text" => Some(text_update("agent_message_chunk", text)),
        "thinking" => Some(text_update("agent_thought_chunk", text)),
        _ => None,
    }
}

/// An engine's update under its `code_engine` call. Only the kinds an editor shows are forwarded;
/// its text is folded into thoughts, as the plaza's own engine text is.
fn engine_frame(call: &str, update: &Value) -> Option<Value> {
    let mut update = update.clone();
    let session_update = update["sessionUpdate"].as_str()?.to_owned();
    match session_update.as_str() {
        "tool_call" | "tool_call_update" => {
            if let Some(id) = update["toolCallId"].as_str() {
                let id = format!("{call}/{id}");
                update["toolCallId"] = json!(id);
            }
        }
        "plan" => {}
        "agent_message_chunk" | "agent_thought_chunk" => {
            update["sessionUpdate"] = json!("agent_thought_chunk");
        }
        _ => return None,
    }
    Some(update)
}

/// One recorded `engine_update` entry (`{call, item, update | text}`) as the update it was live.
fn engine_item(data: &Value) -> Option<Value> {
    let call = str_of(data, "call");
    match data["item"].as_str()? {
        "tool_call" | "tool_status" | "plan" => engine_frame(call, &data["update"]),
        "message" => Some(text_update("agent_thought_chunk", data["text"].as_str()?)),
        _ => None,
    }
}

/// The text of a model response, as the web's transcript view reads it: its text blocks, trimmed,
/// one paragraph each.
fn response_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"].as_str() == Some("text"))
        .filter_map(|block| block["text"].as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CID: u64 = 7;
    const WORK: u64 = 3;

    fn feed() -> Feed {
        Feed::new(CID, WORK)
    }

    /// A bus `transcript` frame's data: the entry serialized.
    fn transcript(cid: u64, work: u64, kind: &str, data: Value) -> Value {
        json!({ "seq": 1, "at_ms": 1, "cid": cid, "work": work, "kind": kind, "data": data })
    }

    fn entry(work: u64, kind: &str, data: Value) -> Value {
        transcript(CID, work, kind, data)
    }

    #[test]
    fn deltas_stream_as_message_and_thought_chunks() {
        let mut feed = feed();
        let text =
            json!({"cid": 7, "work": 3, "req": "r1", "delta": {"type": "text", "text": "hi"}});
        assert_eq!(
            feed.frame("delta", &text),
            vec![
                json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}})
            ]
        );
        let thinking =
            json!({"cid": 7, "work": 3, "req": "r1", "delta": {"type": "thinking", "text": "hm"}});
        assert_eq!(
            feed.frame("delta", &thinking),
            vec![
                json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}})
            ]
        );
        let tool_use = json!({"cid": 7, "work": 3, "req": "r1", "delta": {"type": "tool_use", "name": "bash"}});
        assert!(feed.frame("delta", &tool_use).is_empty());
    }

    #[test]
    fn tool_requests_become_pending_tool_calls_with_their_kind() {
        let mut feed = feed();
        let request = |name: &str, input: Value| {
            transcript(
                CID,
                WORK,
                "tool_request",
                json!({"call": "c1", "name": name, "input": input}),
            )
        };
        assert_eq!(
            feed.frame("transcript", &request("read_file", json!({"path": "a.rs"}))),
            vec![json!({
                "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "read_file",
                "kind": "read", "status": "pending", "rawInput": {"path": "a.rs"}
            })]
        );
        let kinds = [
            ("write_file", "edit"),
            ("bash", "execute"),
            ("code_engine", "execute"),
            ("fetch_url", "fetch"),
        ];
        for (name, kind) in kinds {
            let updates = feed.frame("transcript", &request(name, json!({})));
            assert_eq!(updates[0]["kind"], json!(kind), "{name}");
        }
    }

    #[test]
    fn decisions_show_progress_denials_and_host_results() {
        let mut feed = feed();
        let decision = |decision: &str, output: &str| {
            transcript(
                CID,
                WORK,
                "tool_decision",
                json!({"call": "c1", "decision": decision, "output": output}),
            )
        };
        assert_eq!(
            feed.frame("transcript", &decision("allow", "")),
            vec![
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress"})
            ]
        );
        assert_eq!(
            feed.frame("transcript", &decision("deny", "not granted")),
            vec![json!({
                "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "failed",
                "content": [{"type": "content", "content": {"type": "text", "text": "not granted"}}],
                "rawOutput": "not granted"
            })]
        );
        assert_eq!(
            feed.frame("transcript", &decision("done", "42")),
            vec![json!({
                "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
                "content": [{"type": "content", "content": {"type": "text", "text": "42"}}],
                "rawOutput": "42"
            })]
        );
    }

    #[test]
    fn tool_results_complete_or_fail_the_call() {
        let mut feed = feed();
        let result = |ok: bool| {
            transcript(
                CID,
                WORK,
                "tool_result",
                json!({"call": "c1", "ok": ok, "output": "3 lines"}),
            )
        };
        assert_eq!(
            feed.frame("transcript", &result(true)),
            vec![json!({
                "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
                "content": [{"type": "content", "content": {"type": "text", "text": "3 lines"}}],
                "rawOutput": "3 lines"
            })]
        );
        assert_eq!(
            feed.frame("transcript", &result(false))[0]["status"],
            json!("failed")
        );
    }

    #[test]
    fn todo_write_is_the_plan_and_its_calls_are_not_tool_calls() {
        let mut feed = feed();
        let request = transcript(
            CID,
            WORK,
            "tool_request",
            json!({"call": "p1", "name": "todo_write", "input": {"todos": [
                {"content": "read the spec", "status": "completed"},
                {"content": "fill the feed", "status": "in_progress"},
                {"content": "commit", "status": "pending"}
            ]}}),
        );
        assert_eq!(
            feed.frame("transcript", &request),
            vec![json!({
                "sessionUpdate": "plan",
                "entries": [
                    {"content": "read the spec", "priority": "medium", "status": "completed"},
                    {"content": "fill the feed", "priority": "medium", "status": "in_progress"},
                    {"content": "commit", "priority": "medium", "status": "pending"}
                ]
            })]
        );
        let decision = transcript(
            CID,
            WORK,
            "tool_decision",
            json!({"call": "p1", "decision": "allow", "output": ""}),
        );
        let result = transcript(
            CID,
            WORK,
            "tool_result",
            json!({"call": "p1", "ok": true, "output": "ok"}),
        );
        assert!(feed.frame("transcript", &decision).is_empty());
        assert!(feed.frame("transcript", &result).is_empty());
    }

    #[test]
    fn model_responses_and_engine_updates_are_live_only_in_the_transcript() {
        let mut feed = feed();
        let response = transcript(
            CID,
            WORK,
            "model_response",
            json!({"content": [{"type": "text", "text": "x"}]}),
        );
        let engine = transcript(
            CID,
            WORK,
            "engine_update",
            json!({"call": "e1", "item": "message", "text": "x"}),
        );
        assert!(feed.frame("transcript", &response).is_empty());
        assert!(feed.frame("transcript", &engine).is_empty());
    }

    #[test]
    fn engine_frames_are_rekeyed_and_their_text_folded_into_thoughts() {
        let mut feed = feed();
        let frame = |update: Value| json!({"cid": 7, "work": 3, "call": "c1", "engine": "codex", "session": "s", "seq": 1, "update": update});
        assert_eq!(
            feed.frame("engine", &frame(json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "grep", "status": "pending"}))),
            vec![json!({"sessionUpdate": "tool_call", "toolCallId": "c1/t1", "title": "grep", "status": "pending"})]
        );
        assert_eq!(
            feed.frame("engine", &frame(json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"}))),
            vec![json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1/t1", "status": "completed"})]
        );
        let plan = json!({"sessionUpdate": "plan", "entries": [{"content": "a", "priority": "medium", "status": "pending"}]});
        assert_eq!(feed.frame("engine", &frame(plan.clone())), vec![plan]);
        assert_eq!(
            feed.frame("engine", &frame(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hm"}}))),
            vec![json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}})]
        );
        assert!(
            feed.frame(
                "engine",
                &frame(json!({"sessionUpdate": "usage_update", "used": 1}))
            )
            .is_empty()
        );
    }

    #[test]
    fn foreign_frames_are_ignored() {
        let mut feed = feed();
        let other_agent =
            json!({"cid": 8, "work": 3, "req": "r1", "delta": {"type": "text", "text": "x"}});
        let other_work = json!({"cid": 7, "work": 4, "call": "c1", "update": {"sessionUpdate": "plan", "entries": []}});
        let other_entry = transcript(
            7,
            4,
            "tool_request",
            json!({"call": "c2", "name": "bash", "input": {}}),
        );
        assert!(feed.frame("delta", &other_agent).is_empty());
        assert!(feed.frame("engine", &other_work).is_empty());
        assert!(feed.frame("transcript", &other_entry).is_empty());
        let other_end = transcript(8, 3, "work_end", json!({"status": "done", "summary": ""}));
        assert_eq!(feed.end("transcript", &other_end), None);
    }

    #[test]
    fn long_content_is_capped() {
        let mut feed = feed();
        let big = "a".repeat(CONTENT_BYTES * 2);
        let result = transcript(
            CID,
            WORK,
            "tool_result",
            json!({"call": "c1", "ok": true, "output": big}),
        );
        let updates = feed.frame("transcript", &result);
        let text = updates[0]["content"][0]["content"]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(text.starts_with("aaaa"));
        assert!(text.len() < CONTENT_BYTES * 2);
    }

    #[test]
    fn work_end_ends_this_work_only() {
        let feed = feed();
        let end = transcript(
            CID,
            WORK,
            "work_end",
            json!({"status": "done", "summary": "all good"}),
        );
        assert_eq!(
            feed.end("transcript", &end),
            Some(End {
                status: "done".into(),
                summary: "all good".into()
            })
        );
        let other = transcript(
            CID,
            WORK,
            "tool_result",
            json!({"call": "c1", "ok": true, "output": ""}),
        );
        assert_eq!(feed.end("transcript", &other), None);
        assert_eq!(feed.end("delta", &end), None);
    }

    #[test]
    fn replay_gives_the_prompt_then_the_turn_in_order() {
        let entries = vec![
            entry(
                WORK,
                "work_start",
                json!({"kind": "chat", "thread": "t", "prompt": "fix it", "origin": "o"}),
            ),
            entry(9, "work_start", json!({"prompt": "someone else"})),
            entry(
                WORK,
                "model_response",
                json!({"content": [
                    {"type": "thinking", "text": "hmm"},
                    {"type": "text", "text": " On it. "}
                ]}),
            ),
            entry(
                WORK,
                "tool_request",
                json!({"call": "c1", "name": "read_file", "input": {"path": "a.rs"}}),
            ),
            entry(
                WORK,
                "tool_decision",
                json!({"call": "c1", "decision": "allow", "output": ""}),
            ),
            entry(
                WORK,
                "tool_result",
                json!({"call": "c1", "ok": true, "output": "fn main"}),
            ),
            entry(
                WORK,
                "engine_update",
                json!({"call": "e1", "item": "tool_call", "update": {
                    "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "grep", "kind": "search", "status": "pending"
                }}),
            ),
            entry(
                WORK,
                "engine_update",
                json!({"call": "e1", "item": "usage", "update": {"sessionUpdate": "usage_update"}}),
            ),
            entry(
                WORK,
                "engine_update",
                json!({"call": "e1", "item": "message", "text": "Done."}),
            ),
            entry(
                WORK,
                "engine_update",
                json!({"call": "e1", "item": "plan", "update": {"sessionUpdate": "plan", "entries": []}}),
            ),
            entry(WORK, "work_end", json!({"status": "done", "summary": ""})),
        ];
        assert_eq!(
            replay_frames(WORK, &entries),
            vec![
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "fix it"}}),
                json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "On it."}}),
                json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "read_file", "kind": "read", "status": "pending", "rawInput": {"path": "a.rs"}}),
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress"}),
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
                    "content": [{"type": "content", "content": {"type": "text", "text": "fn main"}}],
                    "rawOutput": "fn main"
                }),
                json!({"sessionUpdate": "tool_call", "toolCallId": "e1/t1", "title": "grep", "kind": "search", "status": "pending"}),
                json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Done."}}),
                json!({"sessionUpdate": "plan", "entries": []}),
            ]
        );
    }
}
