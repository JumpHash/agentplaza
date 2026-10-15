//! What the hub is asked before an engine acts, and how its verdict goes back to the engine.
//!
//! Every `session/request_permission` and every client method that touches the VM (`fs/*`,
//! `terminal/create`) becomes one check named `engine.<kind>` (`apz_acp::map::plaza_tool_name`) with
//! the input `{engine, title, locations, raw}`. The answer to a permission request is always a
//! one-time option: `allow_once` or `reject_once`, never an `*_always` one, so the hub decides every
//! action and the engine caches nothing.

use serde_json::Value;

use super::link::Verdict;

/// The most of the engine's raw input sent to the hub with a check, in bytes.
pub const RAW_BYTES: usize = 16 * 1024;

/// The check for `session/request_permission {sessionId, toolCall, options}`: the tool name
/// (`engine.read|edit|exec|fetch|other`, from `toolCall.kind`; a missing kind is `other`) and the
/// input `{engine, title, locations, raw}` (`raw` is `toolCall.rawInput`, capped to [`RAW_BYTES`]).
pub fn permission_check(engine: &str, params: &Value) -> (String, Value) {
    let _ = (engine, params);
    ("engine.other".to_string(), Value::Null)
}

/// The response to that permission request for `verdict`: `{outcome: {outcome: "selected", optionId}}`
/// with the first `allow_once` option for [`Verdict::Allow`] and the first `reject_once` option for
/// [`Verdict::Deny`]; `{outcome: {outcome: "cancelled"}}` for [`Verdict::Cancelled`]. When the engine
/// offered no matching one-time option the request is answered with the first `reject_once`, else
/// `cancelled`: an `*_always` option is never chosen.
pub fn permission_answer(params: &Value, verdict: &Verdict) -> Value {
    let _ = (params, verdict);
    Value::Null
}

/// The check for a client method the engine called, or `None` when it needs none
/// (`terminal/output`, `wait_for_exit`, `kill`, `release` act on a terminal already allowed).
/// `fs/read_text_file` is `engine.read`, `fs/write_text_file` is `engine.edit` and
/// `terminal/create` is `engine.exec`; `label` is the workspace-relative path the fs method names
/// (its `locations`), and the title says what is done (`write src/main.rs`, `run cargo test`).
pub fn client_check(
    engine: &str,
    method: &str,
    params: &Value,
    label: Option<&str>,
) -> Option<(String, Value)> {
    let _ = (engine, method, params, label);
    None
}
