//! The provider-neutral chat model.
//!
//! The shape follows the Anthropic Messages API (content blocks, `tool_use` /
//! `tool_result` pairing) because it is the most expressive of the provider
//! formats; every gateway adapter translates to and from it.
//!
//! Provider-specific state that must round-trip unchanged (Anthropic thinking
//! signatures, OpenAI encrypted reasoning items, Gemini thought signatures,
//! Bedrock reasoning blocks) travels as [`Block::Opaque`] or in
//! [`Block::ToolUse::meta`], tagged with the provider *kind* that produced it.
//! An adapter replays opaque state only to the kind that produced it and drops
//! it for every other kind.
//!
//! Conversations are append-only: a harness never edits or removes an earlier
//! turn (several providers reject edited history that carries reasoning
//! state). A conversation that grows too long is closed and a new one started.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: vec![Block::Text { text: text.into() }],
        }
    }

    pub fn assistant(content: Vec<Block>) -> Self {
        Message {
            role: Role::Assistant,
            content,
        }
    }

    /// The concatenated text blocks.
    pub fn text(&self) -> String {
        blocks_text(&self.content)
    }
}

/// One content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    /// The model asks for a tool. `id` is unique within the conversation.
    ToolUse {
        id: String,
        name: String,
        input: Value,
        /// Provider state attached to the call (e.g. a Gemini thought
        /// signature), replayed only to `meta.kind`'s provider kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<Value>,
    },
    /// The answer to a [`Block::ToolUse`] with the same id.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// Readable reasoning text (a summary at most); display only, never
    /// sent back to a provider. Replayable reasoning state is [`Block::Opaque`].
    Thinking {
        text: String,
    },
    /// Provider state to replay verbatim to the same provider kind.
    Opaque {
        kind: String,
        raw: Value,
    },
    /// An image the user sends (a screenshot, say): its media type (`image/png`, …) and its bytes in base64.
    /// Only the providers whose wire format has an image block take it; the others refuse the call.
    Image {
        media_type: String,
        data: String,
    },
}

/// The concatenated text of `blocks`' text blocks, newline-joined.
pub fn blocks_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// A JSON Schema object for the tool's input.
    pub input_schema: Value,
    /// Offered by name and one line only: the model fetches the schema through `tool_search` before it calls the
    /// tool. The harness sends such a tool with an empty schema until then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deferred: bool,
}

/// One part of the system prompt. The parts are sent most stable first; `cache` puts a cache breakpoint after the
/// part, so every call that starts with the same parts reads them from the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPart {
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cache: bool,
}

/// One model call. `model` is `"<provider-id>/<model-id>"`; the guest leaves
/// it empty and `apid` fills in the agent's configured model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// The system prompt in parts, most stable first. When non-empty it replaces `system`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_parts: Vec<SystemPart>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// A cache breakpoint at the end of the conversation's last message, so the next call reads the whole prefix.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cache_last_message: bool,
    /// 0 means "the configured default".
    #[serde(default)]
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Reasoning effort (`low`..`max`) where the provider supports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl ChatRequest {
    /// Whether any message carries an image block.
    pub fn has_images(&self) -> bool {
        self.messages
            .iter()
            .any(|m| m.content.iter().any(|b| matches!(b, Block::Image { .. })))
    }

    /// The system prompt as one text: the parts joined by a blank line, or `system`. `None` when it is empty.
    pub fn system_text(&self) -> Option<String> {
        if self.system_parts.is_empty() {
            return self.system.clone().filter(|text| !text.is_empty());
        }
        let text = self
            .system_parts
            .iter()
            .map(|part| part.text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!text.is_empty()).then_some(text)
    }
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// The provider declined (a safety refusal).
    Refusal,
    Other(String),
}

/// Token accounting for one call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

/// One model answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// The model that actually answered (`provider/model`).
    pub model: String,
    pub content: Vec<Block>,
    pub stop: StopReason,
    pub usage: Usage,
}

impl ChatResponse {
    pub fn text(&self) -> String {
        blocks_text(&self.content)
    }

    /// The tool calls in this answer, in order.
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            Block::ToolUse {
                id, name, input, ..
            } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

/// A streamed fragment: text for live display, and a tool call as the provider
/// writes it. A call opens with its [`Delta::ToolUse`], takes its input JSON in
/// [`Delta::ToolInput`] chunks and is whole at its [`Delta::BlockEnd`]. `index`
/// names the call's block within the answer. The final [`ChatResponse`] is
/// authoritative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    /// A tool call opens at block `index`: its tool `name` and the call's `id`.
    ToolUse {
        name: String,
        index: usize,
        id: String,
    },
    /// The next chunk of block `index`'s input JSON.
    ToolInput {
        index: usize,
        json: String,
    },
    /// Block `index` is complete: its input JSON has been sent whole.
    BlockEnd {
        index: usize,
    },
}

/// The in-VM tools the tool library names by path: `code.read` is the guest tool `read`. The harness rewrites a
/// `tools_call` for one of these into the tool itself, so the call runs under the policy like a call of its own.
pub const GUEST_TOOL_PATHS: &[(&str, &str)] = &[
    ("code.read", "read"),
    ("code.edit", "edit"),
    ("code.find", "find"),
    ("code.search", "search"),
    ("code.run_tests", "run_tests"),
    ("code.symbols", "symbols"),
    ("code.definition", "definition"),
    ("code.references", "references"),
    ("code.diagnostics", "diagnostics"),
    ("code.outline", "outline"),
    ("code.engine", "code_engine"),
    ("shell.run", "run"),
    ("shell.script", "run_script"),
    ("helpers.spawn", "spawn_helper"),
    ("helpers.await", "await_helper"),
    ("files.share", "share_file"),
    ("files.deliver", "deliver_file"),
    ("web.fetch", "fetch_url"),
    ("web.download", "download"),
    ("packages.install", "install_packages"),
    ("todo.write", "todo_write"),
    ("artifacts.create", "artifact_create"),
    ("artifacts.update", "artifact_update"),
    ("artifacts.attach_data", "artifact_attach_data"),
    ("artifacts.share_request", "artifact_share_request"),
    ("tools.package", "tool_package"),
    ("checkpoints.list", "checkpoint_list"),
    ("checkpoints.diff", "checkpoint_diff"),
    ("checkpoints.restore", "checkpoint_restore"),
];

/// The guest tool a library path names, if it names one.
pub fn guest_tool_of(path: &str) -> Option<&'static str> {
    GUEST_TOOL_PATHS
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, tool)| *tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blocks_round_trip_with_their_tags() {
        let blocks = vec![
            Block::Text { text: "hi".into() },
            Block::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: json!({"command": "ls"}),
                meta: None,
            },
            Block::ToolResult {
                tool_use_id: "t1".into(),
                content: "a\nb".into(),
                is_error: false,
            },
            Block::Opaque {
                kind: "anthropic".into(),
                raw: json!({"type": "thinking", "signature": "s"}),
            },
        ];
        let line = serde_json::to_string(&blocks).unwrap();
        assert!(line.contains(r#""type":"tool_use""#));
        assert_eq!(serde_json::from_str::<Vec<Block>>(&line).unwrap(), blocks);
    }

    #[test]
    fn a_tool_call_streams_as_open_input_and_end() {
        let stream = vec![
            Delta::ToolUse {
                name: "bash".into(),
                index: 1,
                id: "t1".into(),
            },
            Delta::ToolInput {
                index: 1,
                json: "{\"command\"".into(),
            },
            Delta::BlockEnd { index: 1 },
        ];
        let line = serde_json::to_string(&stream).unwrap();
        assert_eq!(
            line,
            r#"[{"type":"tool_use","name":"bash","index":1,"id":"t1"},{"type":"tool_input","index":1,"json":"{\"command\""},{"type":"block_end","index":1}]"#
        );
        assert_eq!(serde_json::from_str::<Vec<Delta>>(&line).unwrap(), stream);
    }

    #[test]
    fn a_request_with_parts_round_trips_and_reads_as_one_system_text() {
        let req = ChatRequest {
            model: String::new(),
            system: Some("ignored once parts are set".into()),
            system_parts: vec![
                SystemPart {
                    text: "plaza".into(),
                    cache: true,
                },
                SystemPart {
                    text: String::new(),
                    cache: false,
                },
                SystemPart {
                    text: "seat".into(),
                    cache: false,
                },
            ],
            messages: vec![Message::user_text("hi")],
            tools: vec![ToolSpec {
                name: "later".into(),
                description: "a deferred tool".into(),
                input_schema: json!({"type": "object"}),
                deferred: true,
            }],
            cache_last_message: true,
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        assert_eq!(req.system_text().as_deref(), Some("plaza\n\nseat"));
        let line = serde_json::to_string(&req).unwrap();
        assert!(line.contains(r#""cache":true"#) && line.contains(r#""deferred":true"#));
        assert_eq!(serde_json::from_str::<ChatRequest>(&line).unwrap(), req);
    }

    #[test]
    fn a_plain_system_or_none_reads_as_its_text() {
        let mut req: ChatRequest = serde_json::from_value(json!({"messages": []})).unwrap();
        assert_eq!(req.system_text(), None);
        req.system = Some("be brief".into());
        assert_eq!(req.system_text().as_deref(), Some("be brief"));
        // A tool's flags are off by default and left off the wire.
        let tool: ToolSpec = serde_json::from_value(
            json!({"name": "bash", "description": "run", "input_schema": {}}),
        )
        .unwrap();
        assert!(!tool.deferred);
        assert!(!serde_json::to_string(&tool).unwrap().contains("deferred"));
    }

    #[test]
    fn a_guest_request_needs_only_messages() {
        let req: ChatRequest = serde_json::from_value(json!({"messages": []})).unwrap();
        assert_eq!(req.model, "");
        assert_eq!(req.max_tokens, 0);
        assert!(req.tools.is_empty());
    }
}
