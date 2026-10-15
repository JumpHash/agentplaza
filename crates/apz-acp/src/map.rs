//! The mapping between plaza concepts and ACP's, in the places that need no other crate: the tool kind an
//! editor sees for a law operation and the plaza name an ACP tool kind runs under, the stop reason of a
//! finished work item, and the size cap on a tool update kept in the transcript. The harness (engine to
//! transcript) and the hub (transcript to editor) both use these, so the two cannot drift apart.

use serde_json::{Map, Value};

use crate::types::{StopReason, ToolKind};

/// The plaza tool name an ACP tool kind runs under, so that the laws check the call as that operation.
///
/// Search and think change nothing, so they are reads; move and delete are edits. Switching modes and any
/// kind this version does not know fall to `engine.other`.
pub fn plaza_tool_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read | ToolKind::Search | ToolKind::Think => "engine.read",
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => "engine.edit",
        ToolKind::Execute => "engine.exec",
        ToolKind::Fetch => "engine.fetch",
        _ => "engine.other",
    }
}

/// The ACP tool kind a plaza law operation is shown as. Operations the editor has no kind for are `Other`.
pub fn tool_kind_for_operation(operation: &str) -> ToolKind {
    match operation {
        "tool.read" => ToolKind::Read,
        "tool.write" => ToolKind::Edit,
        "tool.exec" => ToolKind::Execute,
        "tool.fetch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

/// The stop reason of a finished plaza work item, by its `work_end` status.
///
/// `done` is a normal end. `stopped` (cancelled, or ended by a stop condition) is `Cancelled`. ACP has no
/// stop reason for a failure: a failed turn is answered with a JSON-RPC error, so `failed` and any unknown
/// status give `None` and the caller reports the error itself.
pub fn stop_reason_for_outcome(outcome: &str) -> Option<StopReason> {
    match outcome {
        "done" => Some(StopReason::EndTurn),
        "stopped" => Some(StopReason::Cancelled),
        _ => None,
    }
}

/// Caps every string in an ACP update at `max_bytes`, cutting on a character boundary and saying how much
/// was dropped.
///
/// Tool output, raw input and diffs can be arbitrarily large, so the cap applies to every string rather than
/// to named fields: a field list would miss the raw values, which carry whatever the tool produced. Strings
/// within the limit and every other value are left as they are.
pub fn cap_update(update: Value, max_bytes: usize) -> Value {
    match update {
        Value::String(text) if text.len() > max_bytes => Value::String(cap_text(&text, max_bytes)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| cap_update(item, max_bytes))
                .collect(),
        ),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, cap_update(value, max_bytes)))
                .collect::<Map<String, Value>>(),
        ),
        other => other,
    }
}

fn cap_text(text: &str, max_bytes: usize) -> String {
    // Index 0 is always a boundary, so the loop ends.
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated {} bytes]", &text[..end], text.len() - end)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_tool_kind_runs_under_a_plaza_tool_name() {
        let cases = [
            (ToolKind::Read, "engine.read"),
            (ToolKind::Search, "engine.read"),
            (ToolKind::Think, "engine.read"),
            (ToolKind::Edit, "engine.edit"),
            (ToolKind::Delete, "engine.edit"),
            (ToolKind::Move, "engine.edit"),
            (ToolKind::Execute, "engine.exec"),
            (ToolKind::Fetch, "engine.fetch"),
            (ToolKind::SwitchMode, "engine.other"),
            (ToolKind::Other, "engine.other"),
        ];
        for (kind, name) in cases {
            assert_eq!(plaza_tool_name(kind), name, "{kind:?}");
        }
    }

    #[test]
    fn law_operations_map_to_the_tool_kind_an_editor_shows() {
        assert_eq!(tool_kind_for_operation("tool.read"), ToolKind::Read);
        assert_eq!(tool_kind_for_operation("tool.write"), ToolKind::Edit);
        assert_eq!(tool_kind_for_operation("tool.exec"), ToolKind::Execute);
        assert_eq!(tool_kind_for_operation("tool.fetch"), ToolKind::Fetch);
        assert_eq!(tool_kind_for_operation("tool.delete"), ToolKind::Other);
        assert_eq!(tool_kind_for_operation("engine.read"), ToolKind::Other);
        assert_eq!(tool_kind_for_operation(""), ToolKind::Other);
    }

    #[test]
    fn work_outcomes_map_to_stop_reasons() {
        assert_eq!(stop_reason_for_outcome("done"), Some(StopReason::EndTurn));
        assert_eq!(
            stop_reason_for_outcome("stopped"),
            Some(StopReason::Cancelled)
        );
        assert_eq!(stop_reason_for_outcome("failed"), None);
        assert_eq!(stop_reason_for_outcome("bogus"), None);
    }

    #[test]
    fn cap_update_cuts_long_strings_and_keeps_other_values() {
        // A long string is cut with a marker; a short one is kept.
        {
            let update = json!({
                "sessionUpdate": "tool_call_update",
                "rawOutput": "abcdefghijklmnopqrstuvwxyz0123456789",
                "content": [{"type": "text", "text": "xy"}],
            });
            // 16 bytes: "tool_call_update" is exactly that long, so it is kept.
            let capped = cap_update(update, 16);
            assert_eq!(
                capped["rawOutput"],
                json!("abcdefghijklmnop… [truncated 20 bytes]")
            );
            assert_eq!(capped["content"][0]["text"], json!("xy"));
            assert_eq!(capped["sessionUpdate"], json!("tool_call_update"));
        }

        // A cut lands on a character boundary.
        {
            // "é" is two bytes: a cut at byte 1 must step back to 0.
            let capped = cap_update(json!("é"), 1);
            assert_eq!(capped, json!("… [truncated 2 bytes]"));
            let capped = cap_update(json!("aé"), 2);
            assert_eq!(capped, json!("a… [truncated 2 bytes]"));
        }

        // Non-text values pass through unchanged.
        {
            let value =
                json!({"n": 12345678901234u64, "flag": true, "nothing": null, "list": [1, 2]});
            assert_eq!(cap_update(value.clone(), 0), value);
        }
    }
}
