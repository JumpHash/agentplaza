//! The OpenAI Responses API (`openai-responses`), stateless: `store: false`
//! with encrypted reasoning items replayed verbatim. Not streamed: the final
//! text is emitted as one delta.

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::http::Req;
use crate::openai::{authorize, base};
use crate::{Ctx, GatewayError, own_meta, parse_tool_input, replayable};

const TAG: &str = "openai-responses";

pub(crate) fn complete(
    ctx: &Ctx,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let url = format!("{}/responses", base(ctx)?);
    let body = ctx.finish_body(body(req, model));
    let answer = ctx.http().json(&authorize(ctx, Req::post_json(url, &body))?)?;
    decode(&answer, sink)
}

fn body(req: &ChatRequest, model: &str) -> Value {
    let mut body = json!({
        "model": model,
        "input": input(&req.messages),
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    if let Some(system) = req.system.as_deref().filter(|s| !s.is_empty()) {
        body["instructions"] = json!(system);
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.input_schema}))
            .collect();
        body["tools"] = json!(tools);
    }
    if let Some(effort) = req.effort.as_deref() {
        body["reasoning"] = json!({"effort": effort});
    }
    if req.max_tokens > 0 {
        body["max_output_tokens"] = json!(req.max_tokens);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!((f64::from(t) * 1000.0).round() / 1000.0);
    }
    body
}

/// The conversation as input items, in block order.
fn input(messages: &[Message]) -> Vec<Value> {
    let mut items = Vec::new();
    for m in messages {
        match m.role {
            Role::User => {
                for b in &m.content {
                    if let Block::ToolResult { tool_use_id, content, is_error } = b {
                        let output = if *is_error { format!("Error: {content}") } else { content.clone() };
                        items.push(json!({"type": "function_call_output", "call_id": tool_use_id, "output": output}));
                    }
                }
                let parts: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text { text } if !text.is_empty() => Some(json!({"type": "input_text", "text": text})),
                        _ => None,
                    })
                    .collect();
                if !parts.is_empty() {
                    items.push(json!({"type": "message", "role": "user", "content": parts}));
                }
            }
            Role::Assistant => {
                for b in &m.content {
                    match b {
                        Block::Text { text } if !text.is_empty() => items.push(json!({
                            "type": "message", "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        })),
                        Block::ToolUse { id, name, input, meta } => {
                            let mut call = json!({"type": "function_call", "call_id": id, "name": name, "arguments": input.to_string()});
                            if let Some(item_id) = own_meta(meta, TAG).and_then(|m| m.get("id")) {
                                call["id"] = item_id.clone();
                            }
                            items.push(call);
                        }
                        other => items.extend(replayable(other, TAG).cloned()),
                    }
                }
            }
        }
    }
    items
}

fn decode(v: &Value, sink: &mut dyn FnMut(Delta)) -> Result<ChatResponse, GatewayError> {
    if v["status"] == "failed" {
        let message = v["error"]["message"].as_str().unwrap_or("the response failed").to_string();
        return Err(GatewayError::Provider { message, retryable: false });
    }
    let mut content = Vec::new();
    let mut refused = false;
    for item in v["output"].as_array().into_iter().flatten() {
        match item["type"].as_str().unwrap_or("") {
            "message" => {
                for part in item["content"].as_array().into_iter().flatten() {
                    let text = match part["type"].as_str() {
                        Some("output_text") => part["text"].as_str(),
                        Some("refusal") => {
                            refused = true;
                            part["refusal"].as_str()
                        }
                        _ => None,
                    };
                    if let Some(text) = text.filter(|t| !t.is_empty()) {
                        sink(Delta::Text { text: text.to_string() });
                        content.push(Block::Text { text: text.to_string() });
                    }
                }
            }
            "function_call" => {
                let name = item["name"].as_str().unwrap_or("").to_string();
                sink(Delta::ToolUse { name: name.clone() });
                let (input, invalid) = parse_tool_input(item["arguments"].as_str().unwrap_or(""));
                let meta = invalid.or_else(|| item.get("id").map(|id| json!({"kind": TAG, "id": id})));
                let id = item["call_id"].as_str().unwrap_or("").to_string();
                content.push(Block::ToolUse { id, name, input, meta });
            }
            kind => {
                if kind == "reasoning" {
                    let summary: Vec<&str> =
                        item["summary"].as_array().into_iter().flatten().filter_map(|s| s["text"].as_str()).collect();
                    if !summary.is_empty() {
                        let text = summary.join("\n\n");
                        sink(Delta::Thinking { text: text.clone() });
                        content.push(Block::Thinking { text });
                    }
                }
                content.push(Block::Opaque { kind: TAG.into(), raw: item.clone() });
            }
        }
    }
    let u = &v["usage"];
    let cached = u["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
    let usage = Usage {
        input_tokens: u["input_tokens"].as_u64().unwrap_or(0).saturating_sub(cached),
        output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
    };
    let has_calls = content.iter().any(|b| matches!(b, Block::ToolUse { .. }));
    let stop = match (v["status"].as_str(), v["incomplete_details"]["reason"].as_str()) {
        _ if refused => StopReason::Refusal,
        (Some("incomplete"), Some("max_output_tokens")) => StopReason::MaxTokens,
        (Some("incomplete"), Some("content_filter")) => StopReason::Refusal,
        (Some("incomplete"), reason) => StopReason::Other(reason.unwrap_or("incomplete").to_string()),
        _ if has_calls => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    };
    Ok(ChatResponse { model: v["model"].as_str().unwrap_or("").to_string(), content, stop, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Auth, ProviderKind};
    use crate::testutil::{FakeServer, Reply, gateway, provider, request};
    use apz_proto::chat::ToolSpec;

    #[test]
    fn responses_round_trip_with_reasoning_replay() {
        let reasoning = json!({"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "plan"}], "encrypted_content": "ENC"});
        let answer = json!({
            "model": "gpt-5-2025", "status": "completed",
            "output": [
                reasoning,
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Listing."}]},
                {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "bash", "arguments": "{\"command\":\"ls\"}"},
            ],
            "usage": {"input_tokens": 50, "output_tokens": 9, "input_tokens_details": {"cached_tokens": 10}},
        });
        let server = FakeServer::start(vec![Reply::json(200, &answer), Reply::json(200, &json!({"status": "completed", "output": []}))]);
        let gw = gateway(vec![provider("r", ProviderKind::OpenAiResponses, &server, Auth::Env("OPENAI_API_KEY".into()))]);
        let mut req = ChatRequest {
            model: "r/gpt-5.6-terra@max".into(),
            system: Some("sys".into()),
            tools: vec![ToolSpec { name: "bash".into(), description: "d".into(), input_schema: json!({"type": "object"}) }],
            effort: Some("low".into()),
            ..request("hi")
        };
        let mut deltas = Vec::new();
        let resp = gw.complete(&req, &mut |d| deltas.push(d)).unwrap();
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(resp.usage, Usage { input_tokens: 40, output_tokens: 9, cache_read_tokens: 10, cache_write_tokens: 0 });
        assert_eq!(deltas[0], Delta::Thinking { text: "plan".into() });
        assert!(deltas.contains(&Delta::Text { text: "Listing.".into() }));

        let b = server.requests()[0].json();
        assert_eq!((b["instructions"].clone(), b["store"].clone()), (json!("sys"), json!(false)));
        assert_eq!(b["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(b["model"], "gpt-5.6-terra");
        assert_eq!(b["reasoning"], json!({"effort": "max"}), "the pinned effort wins");
        assert_eq!(b["tools"][0], json!({"type": "function", "name": "bash", "description": "d", "parameters": {"type": "object"}}));

        // Second turn: the reasoning item is replayed verbatim, the call keeps its ids.
        req.messages.push(Message::assistant(resp.content));
        req.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult { tool_use_id: "call_1".into(), content: "a.txt".into(), is_error: false }],
        });
        gw.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(server.requests()[1].json()["input"], json!([
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "plan"}], "encrypted_content": "ENC"},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Listing."}]},
            {"type": "function_call", "call_id": "call_1", "name": "bash", "arguments": "{\"command\":\"ls\"}", "id": "fc_1"},
            {"type": "function_call_output", "call_id": "call_1", "output": "a.txt"},
        ]));
    }
}
