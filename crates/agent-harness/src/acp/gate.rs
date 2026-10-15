//! What the hub is asked before an engine acts, and how its verdict goes back to the engine.
//!
//! Every `session/request_permission` and every client method that touches the VM (`fs/*`,
//! `terminal/create`) becomes one check named `engine.<kind>` (`apz_acp::map::plaza_tool_name`) with
//! the input `{engine, title, locations, raw}`. The answer to a permission request is always a
//! one-time option: `allow_once` or `reject_once`, never an `*_always` one, so the hub decides every
//! action and the engine caches nothing.

use apz_acp::map::{cap_update, plaza_tool_name};
use apz_acp::types::ToolKind;
use serde_json::{Value, json};

use super::link::Verdict;

/// The most of the engine's raw input sent to the hub with a check, in bytes.
pub const RAW_BYTES: usize = 16 * 1024;

/// The check for `session/request_permission {sessionId, toolCall, options}`: the tool name
/// (`engine.read|edit|exec|fetch|other`, from `toolCall.kind`; a missing kind is `other`) and the
/// input `{engine, title, locations, raw}` (`raw` is `toolCall.rawInput`, capped to [`RAW_BYTES`]).
pub fn permission_check(engine: &str, params: &Value) -> (String, Value) {
    let tool_call = &params["toolCall"];
    // An unknown kind string fails to decode and is treated like a missing one.
    let kind = tool_call
        .get("kind")
        .and_then(|kind| serde_json::from_value::<ToolKind>(kind.clone()).ok())
        .unwrap_or(ToolKind::Other);
    let locations = match tool_call.get("locations") {
        Some(locations) if locations.is_array() => locations.clone(),
        _ => json!([]),
    };
    let raw = tool_call.get("rawInput").cloned().unwrap_or(Value::Null);
    let input = json!({
        "engine": engine,
        "title": tool_call.get("title").and_then(Value::as_str).unwrap_or(""),
        "locations": locations,
        "raw": cap_update(raw, RAW_BYTES),
    });
    (plaza_tool_name(kind).to_string(), input)
}

/// The response to that permission request for `verdict`: `{outcome: {outcome: "selected", optionId}}`
/// with the first `allow_once` option for [`Verdict::Allow`] and the first `reject_once` option for
/// [`Verdict::Deny`]; `{outcome: {outcome: "cancelled"}}` for [`Verdict::Cancelled`]. When the engine
/// offered no matching one-time option the request is answered with the first `reject_once`, else
/// `cancelled`: an `*_always` option is never chosen.
pub fn permission_answer(params: &Value, verdict: &Verdict) -> Value {
    let option = match verdict {
        Verdict::Allow => {
            first_option(params, "allow_once").or_else(|| first_option(params, "reject_once"))
        }
        Verdict::Deny(_) => first_option(params, "reject_once"),
        Verdict::Cancelled => None,
    };
    match option {
        Some(option_id) => json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

/// The `optionId` of the first option of `kind` the engine offered, if any.
fn first_option<'a>(params: &'a Value, kind: &str) -> Option<&'a str> {
    params["options"]
        .as_array()?
        .iter()
        .find(|option| option["kind"] == kind)
        .and_then(|option| option["optionId"].as_str())
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
    let (tool, title) = match method {
        "fs/read_text_file" => ("engine.read", titled("read", label.unwrap_or(""))),
        "fs/write_text_file" => ("engine.edit", titled("write", label.unwrap_or(""))),
        "terminal/create" => ("engine.exec", titled("run", &command_line(params))),
        _ => return None,
    };
    let locations = match label {
        Some(path) if method.starts_with("fs/") => json!([{"path": path}]),
        _ => json!([]),
    };
    let mut raw = params.clone();
    if method == "fs/write_text_file" {
        // The content is what the edit writes, not what the hub needs to decide it: its size stands
        // in for it, so a large file never reaches the transcript through the check.
        if let Some(fields) = raw.as_object_mut() {
            let bytes = fields
                .remove("content")
                .as_ref()
                .and_then(Value::as_str)
                .map_or(0, str::len);
            fields.insert("bytes".to_string(), json!(bytes));
        }
    }
    let input = json!({
        "engine": engine,
        "title": title,
        "locations": locations,
        "raw": cap_update(raw, RAW_BYTES),
    });
    Some((tool.to_string(), input))
}

/// A short title for a verb and its object, without the trailing space an empty object leaves.
fn titled(verb: &str, object: &str) -> String {
    format!("{verb} {object}").trim_end().to_string()
}

/// The command and its arguments as the shell line the user would read them.
fn command_line(params: &Value) -> String {
    let mut words = vec![params["command"].as_str().unwrap_or("")];
    words.extend(
        params["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    );
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(kind: Option<&str>) -> Value {
        let mut tool_call = json!({
            "toolCallId": "t1",
            "title": "Read file",
            "rawInput": {"path": "a.rs"},
        });
        if let Some(kind) = kind {
            tool_call["kind"] = json!(kind);
        }
        json!({"sessionId": "s1", "toolCall": tool_call, "options": []})
    }

    #[test]
    fn each_tool_kind_is_checked_under_its_plaza_name() {
        let cases = [
            ("read", "engine.read"),
            ("search", "engine.read"),
            ("think", "engine.read"),
            ("edit", "engine.edit"),
            ("delete", "engine.edit"),
            ("move", "engine.edit"),
            ("execute", "engine.exec"),
            ("fetch", "engine.fetch"),
            ("switch_mode", "engine.other"),
            ("other", "engine.other"),
            ("from_a_newer_schema", "engine.other"),
        ];
        for (kind, name) in cases {
            let (tool, _) = permission_check("claude", &request(Some(kind)));
            assert_eq!(tool, name, "{kind}");
        }
        let (tool, _) = permission_check("claude", &request(None));
        assert_eq!(tool, "engine.other");
    }

    #[test]
    fn the_check_input_carries_the_engine_title_locations_and_raw_input() {
        let mut params = request(Some("edit"));
        params["toolCall"]["locations"] = json!([{"path": "src/main.rs", "line": 3}]);
        let (tool, input) = permission_check("claude", &params);
        assert_eq!(tool, "engine.edit");
        assert_eq!(
            input,
            json!({
                "engine": "claude",
                "title": "Read file",
                "locations": [{"path": "src/main.rs", "line": 3}],
                "raw": {"path": "a.rs"},
            })
        );
    }

    #[test]
    fn a_missing_title_locations_or_raw_input_is_empty() {
        let params = json!({"toolCall": {"toolCallId": "t1"}, "options": []});
        let (tool, input) = permission_check("claude", &params);
        assert_eq!(tool, "engine.other");
        assert_eq!(
            input,
            json!({"engine": "claude", "title": "", "locations": [], "raw": null})
        );
    }

    #[test]
    fn raw_input_is_capped_to_raw_bytes() {
        let mut params = request(Some("execute"));
        params["toolCall"]["rawInput"] = json!({"command": "x".repeat(RAW_BYTES + 100)});
        let (_, input) = permission_check("claude", &params);
        let command = input["raw"]["command"].as_str().unwrap();
        assert!(command.starts_with(&"x".repeat(RAW_BYTES)));
        assert!(command.ends_with("… [truncated 100 bytes]"));
    }

    fn offered(options: &[(&str, &str)]) -> Value {
        let options: Vec<Value> = options
            .iter()
            .map(|(id, kind)| json!({"optionId": id, "name": id, "kind": kind}))
            .collect();
        json!({"sessionId": "s1", "toolCall": {"toolCallId": "t1"}, "options": options})
    }

    fn selected(option_id: &str) -> Value {
        json!({"outcome": {"outcome": "selected", "optionId": option_id}})
    }

    fn cancelled() -> Value {
        json!({"outcome": {"outcome": "cancelled"}})
    }

    #[test]
    fn allow_picks_the_one_time_allow_even_when_always_is_listed_first() {
        let params = offered(&[
            ("always", "allow_always"),
            ("once", "allow_once"),
            ("no", "reject_once"),
        ]);
        assert_eq!(
            permission_answer(&params, &Verdict::Allow),
            selected("once")
        );
    }

    #[test]
    fn deny_picks_the_one_time_reject() {
        let params = offered(&[
            ("always", "allow_always"),
            ("no", "reject_once"),
            ("never", "reject_always"),
        ]);
        assert_eq!(
            permission_answer(&params, &Verdict::Deny("a law".to_string())),
            selected("no")
        );
    }

    #[test]
    fn a_cancelled_decision_is_answered_as_cancelled() {
        let params = offered(&[("once", "allow_once"), ("no", "reject_once")]);
        assert_eq!(permission_answer(&params, &Verdict::Cancelled), cancelled());
    }

    #[test]
    fn allow_with_only_allow_always_offered_is_refused_with_reject_once() {
        let params = offered(&[("always", "allow_always"), ("no", "reject_once")]);
        assert_eq!(permission_answer(&params, &Verdict::Allow), selected("no"));
    }

    #[test]
    fn an_always_option_is_never_chosen() {
        let params = offered(&[("always", "allow_always"), ("never", "reject_always")]);
        assert_eq!(permission_answer(&params, &Verdict::Allow), cancelled());
        assert_eq!(
            permission_answer(&params, &Verdict::Deny(String::new())),
            cancelled()
        );
    }

    #[test]
    fn fs_read_is_checked_as_a_read_of_the_path_it_names() {
        let params = json!({"sessionId": "s1", "path": "/workspace/src/main.rs"});
        let check = client_check("claude", "fs/read_text_file", &params, Some("src/main.rs"));
        assert_eq!(
            check,
            Some((
                "engine.read".to_string(),
                json!({
                    "engine": "claude",
                    "title": "read src/main.rs",
                    "locations": [{"path": "src/main.rs"}],
                    "raw": {"sessionId": "s1", "path": "/workspace/src/main.rs"},
                }),
            ))
        );
    }

    #[test]
    fn fs_write_is_checked_as_an_edit_with_its_size_and_not_its_content() {
        let params = json!({"sessionId": "s1", "path": "/workspace/a.txt", "content": "hello"});
        let (tool, input) =
            client_check("claude", "fs/write_text_file", &params, Some("a.txt")).unwrap();
        assert_eq!(tool, "engine.edit");
        assert_eq!(input["title"], json!("write a.txt"));
        assert_eq!(input["locations"], json!([{"path": "a.txt"}]));
        assert_eq!(
            input["raw"],
            json!({"sessionId": "s1", "path": "/workspace/a.txt", "bytes": 5})
        );
    }

    #[test]
    fn terminal_create_is_checked_as_an_exec_titled_by_its_command_line() {
        let params = json!({
            "sessionId": "s1",
            "command": "cargo",
            "args": ["test", "-p", "agent-harness"],
        });
        let (tool, input) = client_check("claude", "terminal/create", &params, None).unwrap();
        assert_eq!(tool, "engine.exec");
        assert_eq!(input["title"], json!("run cargo test -p agent-harness"));
        assert_eq!(input["locations"], json!([]));

        let bare = json!({"command": "ls"});
        let (_, input) = client_check("claude", "terminal/create", &bare, None).unwrap();
        assert_eq!(input["title"], json!("run ls"));
    }

    #[test]
    fn client_calls_on_an_open_terminal_need_no_check() {
        for method in [
            "terminal/output",
            "terminal/wait_for_exit",
            "terminal/kill",
            "terminal/release",
        ] {
            assert_eq!(
                client_check("claude", method, &json!({}), None),
                None,
                "{method}"
            );
        }
    }
}
