//! Translation between ACP v1 and the v2 draft, as pure JSON.
//!
//! The hub and the harness are written against v1 JSON shapes. At the edge of a connection that speaks
//! v2, these functions turn a v2 message into the v1 shape the rest of the code handles, and back. They
//! hold no state, so where one version needs something the other never sends, the translation says what
//! it does instead: a fixed id per kind of chunk, or a dropped message.
//!
//! Every value a function produces deserializes into the schema type of its own version. The tests check
//! that for each function against the `v1` and `v2` types of the pinned schema crate.

use serde_json::{Value, json};

/// The protocol versions this build speaks, oldest first.
pub const SUPPORTED: [u16; 2] = [1, 2];

/// The id of the plan a v2 session shows. v1 plans have no id; a fixed one makes each `plan_update`
/// replace the plan the client shows.
const PLAN_ID: &str = "plan";

/// Message ids for chunks. v1 chunks carry no message id, and the translation is stateless, so each kind
/// of chunk gets a fixed one: a v2 client folds the chunks of a kind into one message.
const USER_MESSAGE_ID: &str = "user";
const AGENT_MESSAGE_ID: &str = "agent";
const THOUGHT_MESSAGE_ID: &str = "thought";

/// The version to answer an `initialize` with: the requested one when supported, else the latest one we
/// support (ACP). v2 counts as supported only when `allow_v2` is set, so a peer that did not opt in gets v1.
#[must_use]
pub fn negotiate(requested: u16, allow_v2: bool) -> u16 {
    let supported = |version: u16| SUPPORTED.contains(&version) && (version != 2 || allow_v2);
    if supported(requested) {
        requested
    } else {
        SUPPORTED
            .iter()
            .copied()
            .filter(|version| supported(*version))
            .max()
            .unwrap_or(1)
    }
}

/// A v2 client's `initialize` request as the v1 agent side reads it. v2 has no filesystem or terminal
/// client capabilities, so the v1 side sees none: it will not ask a v2 client for files.
#[must_use]
pub fn initialize_request_to_v1(v2: &Value) -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
        },
        "clientInfo": implementation(v2.get("info")),
    })
}

/// Our v1 `initialize` request as a v2 agent reads it. The v1 client capabilities have no v2 field, so
/// they do not cross.
#[must_use]
pub fn initialize_request_to_v2(v1: &Value) -> Value {
    json!({
        "protocolVersion": 2,
        "info": implementation(v1.get("clientInfo")),
        "capabilities": {},
    })
}

/// Our v1 `initialize` response as a v2 client reads it. The capabilities v2 names differently move across:
/// the prompt content kinds and the MCP transports. v1's `loadSession` has no v2 field and does not cross.
#[must_use]
pub fn initialize_response_to_v2(v1: &Value) -> Value {
    let caps = &v1["agentCapabilities"];
    let mut session = json!({});
    if let Some(prompt) = caps.get("promptCapabilities") {
        session["prompt"] = prompt.clone();
    }
    // Stdio MCP servers are part of v1 without a flag; v1's SSE transport has no v2 counterpart.
    let mut mcp = json!({"stdio": {}});
    if caps["mcpCapabilities"]["http"] == true {
        mcp["http"] = json!({});
    }
    session["mcp"] = mcp;
    json!({
        "protocolVersion": 2,
        "info": implementation(v1.get("agentInfo")),
        "capabilities": {"session": session},
        "authMethods": auth_methods_to_v2(&v1["authMethods"]),
    })
}

/// A v2 agent's `initialize` response as the v1 client side reads it.
#[must_use]
pub fn initialize_response_to_v1(v2: &Value) -> Value {
    let session = &v2["capabilities"]["session"];
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            // v2 has no load capability, so a session cannot be loaded through this translation.
            "loadSession": false,
            "promptCapabilities": session.get("prompt").cloned().unwrap_or_else(|| json!({})),
            "mcpCapabilities": {"http": session["mcp"].get("http").is_some(), "sse": false},
            "sessionCapabilities": {},
        },
        "agentInfo": implementation(v2.get("info")),
        "authMethods": auth_methods_to_v1(&v2["authMethods"]),
    })
}

/// Our v1 `session/new` response as a v2 client reads it. v2 has no modes, so the v1 modes are dropped;
/// the config options keep their meaning under their v2 id name. `commands` are the session's commands,
/// in v1 or v2 shape.
#[must_use]
pub fn new_session_response_to_v2(v1: &Value, commands: &[Value]) -> Value {
    json!({
        "sessionId": v1["sessionId"],
        "configOptions": map_items(&v1["configOptions"], config_option_to_v2),
        "availableCommands": commands.iter().map(command_to_v2).collect::<Vec<_>>(),
    })
}

/// One v1 session update as the v2 updates it becomes. The result is empty when v2 has no counterpart:
///
/// - Chunks gain a message id, fixed per kind (see the constants above).
/// - `tool_call` becomes `tool_call_update` with the same fields: v2 has no separate creation.
/// - `plan` becomes `plan_update` for the fixed plan id.
/// - `current_mode_update` is dropped: v2 has no modes, and this translation does not invent a mode option.
/// - The other kinds that exist in v2 keep their fields under the same name.
#[must_use]
pub fn update_to_v2(v1_update: &Value) -> Vec<Value> {
    match v1_update["sessionUpdate"].as_str().unwrap_or_default() {
        "user_message_chunk" => vec![chunk_to_v2(
            v1_update,
            "user_message_chunk",
            USER_MESSAGE_ID,
        )],
        "agent_message_chunk" => vec![chunk_to_v2(
            v1_update,
            "agent_message_chunk",
            AGENT_MESSAGE_ID,
        )],
        "agent_thought_chunk" => vec![chunk_to_v2(
            v1_update,
            "agent_thought_chunk",
            THOUGHT_MESSAGE_ID,
        )],
        "tool_call" => vec![retag(v1_update, "tool_call_update")],
        "tool_call_update" => vec![v1_update.clone()],
        "plan" => vec![json!({
            "sessionUpdate": "plan_update",
            "plan": {"type": "items", "planId": PLAN_ID, "entries": v1_update["entries"]},
        })],
        "available_commands_update" => {
            vec![map_array(v1_update, "availableCommands", command_to_v2)]
        }
        "config_option_update" => vec![map_array(v1_update, "configOptions", config_option_to_v2)],
        "session_info_update" | "usage_update" => vec![v1_update.clone()],
        _ => vec![],
    }
}

/// The v2 `state_update` that says the agent is working on a prompt. v1 has no such update: a v1 agent
/// is busy from the prompt until its response, which v2 replaces with this and the idle update.
#[must_use]
pub fn running_update() -> Value {
    json!({"sessionUpdate": "state_update", "state": "running"})
}

/// The v2 `state_update` that ends a turn: the agent is idle again, with the stop reason and, when known,
/// the usage in the v2 shape. v2 reports the end of a prompt this way rather than in the prompt's response.
#[must_use]
pub fn idle_update(stop_reason: &str, usage: Option<&Value>) -> Value {
    let mut out =
        json!({"sessionUpdate": "state_update", "state": "idle", "stopReason": stop_reason});
    if let Some(usage) = usage {
        out["usage"] = usage.clone();
    }
    out
}

/// A v1 permission request as v2 has it. The title comes from the tool call, and the tool call moves to
/// the subject, which is what v2 says the permission is about.
#[must_use]
pub fn permission_request_to_v2(v1: &Value) -> Value {
    let tool_call = &v1["toolCall"];
    let title = tool_call["title"]
        .as_str()
        .unwrap_or("Permission requested");
    json!({
        "sessionId": v1["sessionId"],
        "title": title,
        "subject": {"type": "tool_call", "toolCall": tool_call},
        "options": v1["options"],
    })
}

/// A v2 permission request as v1 has it. A subject that names no tool call still needs a tool call id for
/// v1, so a fixed one stands in; the title says what is asked.
#[must_use]
pub fn permission_request_to_v1(v2: &Value) -> Value {
    let subject = &v2["subject"];
    let tool_call = if subject["type"] == "tool_call" {
        subject["toolCall"].clone()
    } else {
        json!({
            "toolCallId": subject["toolCallId"].as_str().unwrap_or("permission"),
            "title": v2["title"],
        })
    };
    json!({"sessionId": v2["sessionId"], "toolCall": tool_call, "options": v2["options"]})
}

/// A permission answer as v2 has it. The outcome (`selected` with an option id, or `cancelled`) has the same
/// shape in both versions and moves unchanged.
#[must_use]
pub fn permission_response_to_v2(v1: &Value) -> Value {
    json!({"outcome": v1["outcome"]})
}

/// A v2 permission answer as v1 has it.
#[must_use]
pub fn permission_response_to_v1(v2: &Value) -> Value {
    json!({"outcome": v2["outcome"]})
}

/// One v2 session update as the v1 updates it becomes. The result is empty when v1 has no counterpart.
/// This is stateless, so a caller that keeps state must finish the job:
///
/// - Chunks lose their message id, which v1 does not have. A full `agent_message` or `agent_thought` is
///   dropped: a v2 agent streams its text as chunks, and the full message would repeat them.
/// - `tool_call_update` stays a `tool_call_update`. v1 needs a `tool_call` first for each id, and only a
///   caller that has seen the ids can tell the first update apart.
/// - `plan_update` with items becomes a `plan`.
/// - An idle `state_update` becomes the `_apz_turn_end` marker, which ends the turn for the caller. Other
///   states are dropped.
/// - Terminal output becomes a `tool_call_update` text, keyed by the terminal id: v1 has no terminal update,
///   so the caller maps the terminal to its tool call.
/// - A `tool_call_content_chunk` becomes a `tool_call_update` with the chunk as its content. v1 replaces
///   the content on update, so the caller accumulates the chunks.
#[must_use]
pub fn update_to_v1(v2_update: &Value) -> Vec<Value> {
    match v2_update["sessionUpdate"].as_str().unwrap_or_default() {
        "user_message_chunk" => vec![chunk_to_v1(v2_update, "user_message_chunk")],
        "agent_message_chunk" => vec![chunk_to_v1(v2_update, "agent_message_chunk")],
        "agent_thought_chunk" => vec![chunk_to_v1(v2_update, "agent_thought_chunk")],
        "user_message" => v2_update["content"]
            .as_array()
            .map_or_else(Vec::new, |blocks| {
                blocks
                    .iter()
                    .map(|block| json!({"sessionUpdate": "user_message_chunk", "content": block}))
                    .collect()
            }),
        "tool_call_update" => {
            let mut out = v2_update.clone();
            if let Some(object) = out.as_object_mut() {
                object.remove("name");
            }
            vec![out]
        }
        "tool_call_content_chunk" => vec![json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": v2_update["toolCallId"],
            "content": [v2_update["content"]],
        })],
        "plan_update" if v2_update["plan"]["type"] == "items" => vec![json!({
            "sessionUpdate": "plan",
            "entries": v2_update["plan"]["entries"],
        })],
        "state_update" if v2_update["state"] == "idle" => vec![json!({
            "sessionUpdate": "_apz_turn_end",
            "stopReason": v2_update["stopReason"],
        })],
        "terminal_output_chunk" => {
            terminal_text(&v2_update["terminalId"], v2_update["data"].as_str())
        }
        "terminal_update" => terminal_text(
            &v2_update["terminalId"],
            v2_update["output"]["data"].as_str(),
        ),
        "available_commands_update" => {
            vec![map_array(v2_update, "availableCommands", command_to_v1)]
        }
        "config_option_update" => vec![map_array(v2_update, "configOptions", config_option_to_v1)],
        "session_info_update" | "usage_update" => vec![v2_update.clone()],
        _ => vec![],
    }
}

/// The implementation's name and version, the fields both versions share. v2 requires them, so a peer
/// that sent none gets a placeholder.
fn implementation(info: Option<&Value>) -> Value {
    info.cloned()
        .unwrap_or_else(|| json!({"name": "unknown", "version": ""}))
}

/// v1 auth methods (`id`, `name`, `description`) as v2 agent methods. v2 also has terminal methods, which
/// v1 cannot express, so none are produced here.
fn auth_methods_to_v2(methods: &Value) -> Value {
    let items: &[Value] = methods.as_array().map_or(&[], Vec::as_slice);
    Value::Array(
        items
            .iter()
            .map(|method| {
                json!({
                    "type": "agent",
                    "methodId": method["id"],
                    "name": method["name"],
                    "description": method.get("description").cloned().unwrap_or(Value::Null),
                })
            })
            .collect(),
    )
}

/// The agent auth methods of a v2 response as v1 has them. Terminal methods have no v1 form and are skipped.
fn auth_methods_to_v1(methods: &Value) -> Value {
    let items: &[Value] = methods.as_array().map_or(&[], Vec::as_slice);
    Value::Array(
        items
            .iter()
            .filter(|method| method["type"] == "agent")
            .map(|method| {
                json!({
                    "id": method["methodId"],
                    "name": method["name"],
                    "description": method.get("description").cloned().unwrap_or(Value::Null),
                })
            })
            .collect(),
    )
}

/// A command in v2 shape. v1's `input` is `{hint}`; v2 names the kind of input, so a hint becomes a text input.
/// Commands already in v2 shape are left as they are.
fn command_to_v2(command: &Value) -> Value {
    let mut out = command.clone();
    if let Some(input) = out.get_mut("input").and_then(Value::as_object_mut) {
        if input.contains_key("hint") && !input.contains_key("type") {
            input.insert("type".to_owned(), json!("text"));
        }
    }
    out
}

/// A command in v1 shape: the text input loses its kind, which v1 does not have.
fn command_to_v1(command: &Value) -> Value {
    let mut out = command.clone();
    if let Some(input) = out.get_mut("input").and_then(Value::as_object_mut) {
        if input.get("type") == Some(&json!("text")) {
            input.remove("type");
        }
    }
    out
}

/// A config option in v2 shape: v2 names the id `configId`. The rest of the option is the same in both.
fn config_option_to_v2(option: &Value) -> Value {
    rename_key(option, "id", "configId")
}

/// A config option in v1 shape: `configId` is `id` in v1.
fn config_option_to_v1(option: &Value) -> Value {
    rename_key(option, "configId", "id")
}

/// Copies `value` with its key `from` renamed to `to`. An option already in the target shape is unchanged.
fn rename_key(value: &Value, from: &str, to: &str) -> Value {
    let mut out = value.clone();
    if let Some(object) = out.as_object_mut() {
        if let Some(field) = object.remove(from) {
            object.entry(to.to_owned()).or_insert(field);
        }
    }
    out
}

/// The items of a v1 array mapped through `f`. A missing array becomes an empty one.
fn map_items(items: &Value, f: fn(&Value) -> Value) -> Value {
    Value::Array(
        items
            .as_array()
            .map_or_else(Vec::new, |items| items.iter().map(f).collect()),
    )
}

/// Copies `update` with the array under `key` mapped through `f`.
fn map_array(update: &Value, key: &str, f: fn(&Value) -> Value) -> Value {
    let mut out = update.clone();
    out[key] = map_items(&update[key], f);
    out
}

/// Copies `update` under a new `sessionUpdate` name.
fn retag(update: &Value, name: &str) -> Value {
    let mut out = update.clone();
    out["sessionUpdate"] = json!(name);
    out
}

/// A v1 chunk as a v2 chunk of `name`, with the fixed message id for its kind.
fn chunk_to_v2(update: &Value, name: &str, message_id: &str) -> Value {
    let mut out = retag(update, name);
    out["messageId"] = json!(message_id);
    out
}

/// A v2 chunk as a v1 chunk of `name`, without the message id v1 does not have.
fn chunk_to_v1(update: &Value, name: &str) -> Value {
    let mut out = retag(update, name);
    if let Some(object) = out.as_object_mut() {
        object.remove("messageId");
    }
    out
}

/// Terminal output as a `tool_call_update` text. Nothing is produced when there is no text yet.
fn terminal_text(terminal_id: &Value, data: Option<&str>) -> Vec<Value> {
    data.map_or_else(Vec::new, |text| {
        vec![json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": terminal_id,
            "content": [{"type": "content", "content": {"type": "text", "text": text}}],
        })]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{self, v2};
    use serde::de::DeserializeOwned;

    /// Deserializes `value` into the schema type `T`, or fails the test with the value in the message.
    fn parse<T: DeserializeOwned>(value: &Value) -> T {
        serde_json::from_value(value.clone()).unwrap_or_else(|error| panic!("{error} in {value}"))
    }

    #[test]
    fn negotiate_answers_the_requested_version_or_the_latest_supported() {
        assert_eq!(negotiate(1, true), 1);
        assert_eq!(negotiate(2, true), 2);
        assert_eq!(
            negotiate(2, false),
            1,
            "v2 is answered only to peers that opted in"
        );
        assert_eq!(
            negotiate(3, true),
            2,
            "an unknown version gets the latest we support"
        );
        assert_eq!(negotiate(3, false), 1);
        assert_eq!(negotiate(0, false), 1);
    }

    #[test]
    fn initialize_request_translates_both_ways() {
        let v2_request = json!({
            "protocolVersion": 2,
            "info": {"name": "zed", "version": "0.200.0"},
            "capabilities": {"auth": {}},
        });
        let out = initialize_request_to_v1(&v2_request);
        let _: types::InitializeRequest = parse(&out);
        assert_eq!(out["clientInfo"]["name"], "zed");
        assert_eq!(out["protocolVersion"], 1);

        let v1_request = json!({
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": true},
            "clientInfo": {"name": "hub", "version": "1.0.0"},
        });
        let out = initialize_request_to_v2(&v1_request);
        let _: v2::InitializeRequest = parse(&out);
        assert_eq!(out["protocolVersion"], 2);
        assert_eq!(out["info"]["name"], "hub");
    }

    #[test]
    fn initialize_response_translates_both_ways() {
        let v1_response = json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": true, "audio": false, "embeddedContext": true},
                "mcpCapabilities": {"http": true, "sse": false},
                "sessionCapabilities": {},
            },
            "agentInfo": {"name": "apz", "version": "1.0.0"},
            "authMethods": [],
        });
        let out = initialize_response_to_v2(&v1_response);
        let _: v2::InitializeResponse = parse(&out);
        assert_eq!(out["protocolVersion"], 2);
        assert_eq!(out["capabilities"]["session"]["prompt"]["image"], true);
        assert!(out["capabilities"]["session"]["mcp"]["http"].is_object());

        let v2_response = json!({
            "protocolVersion": 2,
            "info": {"name": "apz", "version": "1.0.0"},
            "capabilities": {"session": {"prompt": {"image": true}, "mcp": {"stdio": {}}}},
            "authMethods": [{"type": "agent", "methodId": "login", "name": "Log in", "description": "Sign in"}],
        });
        let out = initialize_response_to_v1(&v2_response);
        let _: types::InitializeResponse = parse(&out);
        assert_eq!(out["protocolVersion"], 1);
        assert_eq!(
            out["agentCapabilities"]["promptCapabilities"]["image"],
            true
        );
        assert_eq!(out["agentCapabilities"]["mcpCapabilities"]["http"], false);
        assert_eq!(out["authMethods"][0]["id"], "login");
    }

    #[test]
    fn new_session_drops_modes_and_renames_config_ids() {
        let v1_response = json!({
            "sessionId": "s1",
            "modes": {"currentModeId": "code", "availableModes": [{"id": "code", "name": "Code"}]},
            "configOptions": [{
                "id": "model",
                "name": "Model",
                "type": "select",
                "currentValue": "fast",
                "options": [{"value": "fast", "name": "Fast"}],
            }],
        });
        let commands =
            [json!({"name": "compact", "description": "Compact", "input": {"hint": "focus"}})];
        let out = new_session_response_to_v2(&v1_response, &commands);
        let _: v2::NewSessionResponse = parse(&out);
        assert!(out.get("modes").is_none());
        assert_eq!(out["configOptions"][0]["configId"], "model");
        assert!(out["configOptions"][0].get("id").is_none());
        assert_eq!(out["availableCommands"][0]["input"]["type"], "text");
    }

    #[test]
    fn update_to_v2_translates_every_update_kind() {
        // agent_chunks_get_a_message_id: each chunk becomes one v2 update with a message id.
        {
            let updates = [
                json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "Fix it"}}),
                json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Done."}}),
                json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Hmm"}}),
            ];
            for update in &updates {
                let out = update_to_v2(update);
                assert_eq!(out.len(), 1);
                assert!(out[0]["messageId"].is_string());
                let _: v2::SessionUpdate = parse(&out[0]);
            }
        }
        // tool_call_becomes_a_tool_call_update_and_plans_get_a_plan_id: a tool call becomes a
        // tool_call_update, and a plan gets a plan id and the items type.
        {
            let tool_call = json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "t1",
                "title": "Read file",
                "kind": "read",
                "status": "pending",
                "locations": [{"path": "/workspace/src/lib.rs"}],
            });
            let out = update_to_v2(&tool_call);
            let _: v2::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["sessionUpdate"], "tool_call_update");
            assert_eq!(out[0]["toolCallId"], "t1");

            let plan = json!({
                "sessionUpdate": "plan",
                "entries": [{"content": "Read the schema", "priority": "high", "status": "in_progress"}],
            });
            let out = update_to_v2(&plan);
            let _: v2::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["plan"]["planId"], "plan");
            assert_eq!(out[0]["plan"]["type"], "items");
        }
        // modes_are_dropped_and_the_other_updates_keep_their_fields: mode updates are dropped and
        // the other updates keep their fields.
        {
            assert!(
                update_to_v2(
                    &json!({"sessionUpdate": "current_mode_update", "currentModeId": "code"})
                )
                .is_empty()
            );

            let updates = [
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
                       "content": [{"type": "content", "content": {"type": "text", "text": "ok"}}]}),
                json!({"sessionUpdate": "available_commands_update", "availableCommands":
                       [{"name": "compact", "description": "Compact", "input": {"hint": "focus"}}]}),
                json!({"sessionUpdate": "config_option_update", "configOptions":
                       [{"id": "model", "name": "Model", "type": "select", "currentValue": "fast",
                         "options": [{"value": "fast", "name": "Fast"}]}]}),
                json!({"sessionUpdate": "session_info_update", "title": "Fix the bug"}),
                json!({"sessionUpdate": "usage_update", "used": 1200, "size": 200000}),
            ];
            let out: Vec<Value> = updates.iter().flat_map(update_to_v2).collect();
            assert_eq!(out.len(), updates.len());
            let _: Vec<v2::SessionUpdate> = out.iter().map(parse).collect();
            assert_eq!(out[1]["availableCommands"][0]["input"]["type"], "text");
            assert_eq!(out[2]["configOptions"][0]["configId"], "model");
        }
        // state_updates_are_v2_states: idle and running updates are v2 states.
        {
            let usage = json!({"totalTokens": 30, "inputTokens": 10, "outputTokens": 20});
            let idle = idle_update("end_turn", Some(&usage));
            let _: v2::SessionUpdate = parse(&idle);
            assert_eq!(idle["state"], "idle");
            assert_eq!(idle["usage"]["totalTokens"], 30);

            let _: v2::SessionUpdate = parse(&idle_update("cancelled", None));
            let _: v2::SessionUpdate = parse(&running_update());
        }
    }

    #[test]
    fn permission_request_moves_the_tool_call_to_the_subject() {
        let v1_request = json!({
            "sessionId": "s1",
            "toolCall": {"toolCallId": "t1", "title": "Read src/lib.rs", "kind": "read", "status": "pending"},
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ],
        });
        let out = permission_request_to_v2(&v1_request);
        let _: v2::RequestPermissionRequest = parse(&out);
        assert_eq!(out["title"], "Read src/lib.rs");
        assert_eq!(out["subject"]["type"], "tool_call");
        assert_eq!(out["subject"]["toolCall"]["toolCallId"], "t1");
    }

    #[test]
    fn permission_request_and_answer_translate_back_to_v1() {
        let v2_request = json!({
            "sessionId": "s1",
            "title": "Read src/lib.rs",
            "subject": {"type": "tool_call", "toolCall": {"toolCallId": "t1", "title": "Read src/lib.rs"}},
            "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}],
        });
        let out = permission_request_to_v1(&v2_request);
        let _: types::RequestPermissionRequest = parse(&out);
        assert_eq!(out["toolCall"]["toolCallId"], "t1");

        let v1_answer = json!({"outcome": {"outcome": "selected", "optionId": "allow"}});
        let out = permission_response_to_v2(&v1_answer);
        let _: v2::RequestPermissionResponse = parse(&out);
        assert_eq!(out["outcome"]["optionId"], "allow");

        let out = permission_response_to_v1(&json!({"outcome": {"outcome": "cancelled"}}));
        let _: types::RequestPermissionResponse = parse(&out);
    }

    #[test]
    fn update_to_v1_translates_every_v2_update() {
        // v2_chunks_lose_their_message_id_and_full_messages_are_dropped: chunks lose their message
        // id, and full messages are dropped.
        {
            let chunk = json!({"sessionUpdate": "agent_message_chunk", "messageId": "m1",
                               "content": {"type": "text", "text": "Hi"}});
            let out = update_to_v1(&chunk);
            let _: types::SessionUpdate = parse(&out[0]);
            assert!(out[0].get("messageId").is_none());

            let full = json!({"sessionUpdate": "agent_message", "messageId": "m1",
                              "content": [{"type": "text", "text": "Hi"}]});
            assert!(update_to_v1(&full).is_empty());
        }
        // v2_user_message_becomes_one_chunk_per_block: a user message becomes one chunk per block.
        {
            let message = json!({"sessionUpdate": "user_message", "messageId": "u1", "content": [
                {"type": "text", "text": "one"},
                {"type": "text", "text": "two"},
            ]});
            let out = update_to_v1(&message);
            assert_eq!(out.len(), 2);
            for update in &out {
                let _: types::SessionUpdate = parse(update);
            }
        }
        // v2_tool_call_update_and_plan_translate_to_v1: a tool call update drops its name, and a
        // plan update becomes a plan.
        {
            let update = json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
                                "name": "read_file", "content": [{"type": "content",
                                "content": {"type": "text", "text": "ok"}}]});
            let out = update_to_v1(&update);
            let _: types::SessionUpdate = parse(&out[0]);
            assert!(out[0].get("name").is_none());

            let plan = json!({"sessionUpdate": "plan_update", "plan": {"type": "items", "planId": "plan",
                              "entries": [{"content": "Read the schema", "priority": "high", "status": "pending"}]}});
            let out = update_to_v1(&plan);
            let _: types::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["sessionUpdate"], "plan");
        }
        // v2_idle_state_becomes_the_turn_end_marker: an idle state becomes the turn end marker, and
        // a running state is dropped.
        {
            let out = update_to_v1(&idle_update("end_turn", None));
            assert_eq!(
                out,
                vec![json!({"sessionUpdate": "_apz_turn_end", "stopReason": "end_turn"})]
            );
            assert!(update_to_v1(&running_update()).is_empty());
        }
        // terminal_output_becomes_tool_call_text: terminal output becomes tool call text.
        {
            let chunk = json!({"sessionUpdate": "terminal_output_chunk", "terminalId": "term1", "data": "running\n"});
            let out = update_to_v1(&chunk);
            let _: types::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["toolCallId"], "term1");
            assert_eq!(out[0]["content"][0]["content"]["text"], "running\n");

            let update = json!({"sessionUpdate": "terminal_update", "terminalId": "term1",
                                "output": {"data": "all done\n"}});
            let out = update_to_v1(&update);
            let _: types::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["content"][0]["content"]["text"], "all done\n");
        }
        // tool_call_content_chunk_becomes_tool_call_content: a content chunk becomes tool call
        // content.
        {
            let chunk = json!({"sessionUpdate": "tool_call_content_chunk", "toolCallId": "t1",
                               "content": {"type": "content", "content": {"type": "text", "text": "line"}}});
            let out = update_to_v1(&chunk);
            let _: types::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["content"].as_array().map(Vec::len), Some(1));
        }
        // v2_commands_and_config_options_translate_to_v1: commands drop the input type, and config
        // options take the id field.
        {
            let commands = json!({"sessionUpdate": "available_commands_update", "availableCommands":
                                  [{"name": "compact", "description": "Compact", "input": {"type": "text", "hint": "focus"}}]});
            let out = update_to_v1(&commands);
            let _: types::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["availableCommands"][0]["input"]["hint"], "focus");
            assert!(
                out[0]["availableCommands"][0]["input"]
                    .get("type")
                    .is_none()
            );

            let options = json!({"sessionUpdate": "config_option_update", "configOptions":
                                 [{"configId": "model", "name": "Model", "type": "select", "currentValue": "fast",
                                   "options": [{"value": "fast", "name": "Fast"}]}]});
            let out = update_to_v1(&options);
            let _: types::SessionUpdate = parse(&out[0]);
            assert_eq!(out[0]["configOptions"][0]["id"], "model");
        }
        // info_and_usage_pass_through_to_v1: session info and usage pass through unchanged.
        {
            let info = json!({"sessionUpdate": "session_info_update", "title": "Fix the bug"});
            let _: types::SessionUpdate = parse(&update_to_v1(&info)[0]);
            let usage = json!({"sessionUpdate": "usage_update", "used": 1200, "size": 200000});
            let _: types::SessionUpdate = parse(&update_to_v1(&usage)[0]);
        }
    }
}
