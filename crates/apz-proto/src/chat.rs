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
        Message { role: Role::User, content: vec![Block::Text { text: text.into() }] }
    }

    pub fn assistant(content: Vec<Block>) -> Self {
        Message { role: Role::Assistant, content }
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
}

/// One model call. `model` is `"<provider-id>/<model-id>"`; the guest leaves
/// it empty and `apid` fills in the agent's configured model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// 0 means "the configured default".
    #[serde(default)]
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Reasoning effort (`low`..`max`) where the provider supports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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
            Block::ToolUse { id, name, input, .. } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

/// A streamed fragment, for live display only (the final [`ChatResponse`] is
/// authoritative).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    Text { text: String },
    Thinking { text: String },
    ToolUse { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blocks_round_trip_with_their_tags() {
        let blocks = vec![
            Block::Text { text: "hi".into() },
            Block::ToolUse { id: "t1".into(), name: "bash".into(), input: json!({"command": "ls"}), meta: None },
            Block::ToolResult { tool_use_id: "t1".into(), content: "a\nb".into(), is_error: false },
            Block::Opaque { kind: "anthropic".into(), raw: json!({"type": "thinking", "signature": "s"}) },
        ];
        let line = serde_json::to_string(&blocks).unwrap();
        assert!(line.contains(r#""type":"tool_use""#));
        assert_eq!(serde_json::from_str::<Vec<Block>>(&line).unwrap(), blocks);
    }

    #[test]
    fn a_guest_request_needs_only_messages() {
        let req: ChatRequest = serde_json::from_value(json!({"messages": []})).unwrap();
        assert_eq!(req.model, "");
        assert_eq!(req.max_tokens, 0);
        assert!(req.tools.is_empty());
    }
}
