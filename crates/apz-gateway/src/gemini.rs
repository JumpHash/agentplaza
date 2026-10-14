//! The Gemini API (`gemini`) and Gemini on Vertex AI (`vertex-gemini`):
//! `streamGenerateContent` with server-sent events.

use std::collections::HashMap;
use std::io::Read;

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message, Role, StopReason, Usage};
use serde_json::{Map, Value, json};

use crate::config::{Auth, ProviderKind};
use crate::http::{self, Req};
use crate::{Ctx, GatewayError, ModelInfo, own_meta, replayable, results_first, tool_use_count};

const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com";
const TAG: &str = "gemini";

fn authorize(ctx: &Ctx, req: Req) -> Result<Req, GatewayError> {
    let req = if ctx.p.kind == ProviderKind::VertexGemini || matches!(ctx.p.auth, Auth::Command(_))
    {
        req.bearer(&ctx.required_credential()?)
    } else {
        let key = ctx.required_credential()?;
        req.secret_header("x-goog-api-key", &key, &key)
    };
    Ok(req.extra_headers(ctx.p.extra_headers.iter()))
}

pub(crate) fn complete(
    ctx: &Ctx,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let url = match ctx.p.kind {
        ProviderKind::VertexGemini => {
            ctx.vertex_url("google", model, "streamGenerateContent?alt=sse")?
        }
        _ => format!(
            "{}/v1beta/models/{model}:streamGenerateContent?alt=sse",
            ctx.p.base().unwrap_or(DEFAULT_BASE)
        ),
    };
    let body = ctx.finish_body(body(req));
    let resp = ctx
        .http()
        .send(&authorize(ctx, Req::post_json(url, &body))?)?;
    let mut answer = read_stream(http::reader(resp), sink, tool_use_count(&req.messages))?;
    if answer.model.is_empty() {
        answer.model = model.to_string();
    }
    Ok(answer)
}

fn body(req: &ChatRequest) -> Value {
    let mut generation = json!({"thinkingConfig": {"includeThoughts": true}});
    if req.max_tokens > 0 {
        generation["maxOutputTokens"] = json!(req.max_tokens);
    }
    if let Some(t) = req.temperature {
        generation["temperature"] = json!((f64::from(t) * 1000.0).round() / 1000.0);
    }
    let mut body = json!({"contents": contents(&req.messages), "generationConfig": generation});
    if let Some(system) = req.system.as_deref().filter(|s| !s.is_empty()) {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut decl = json!({"name": t.name, "description": t.description});
                let params = schema(&t.input_schema);
                if params.get("properties").is_some() {
                    decl["parameters"] = params;
                }
                decl
            })
            .collect();
        body["tools"] = json!([{"functionDeclarations": decls}]);
    }
    body
}

/// The conversation as `contents`. A function response needs the tool's
/// name, found from the call with the same id earlier in the conversation.
fn contents(messages: &[Message]) -> Vec<Value> {
    let names: HashMap<&str, &str> = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            Block::ToolUse { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for m in messages {
        let mut parts: Vec<Value> = Vec::new();
        for b in results_first(&m.content) {
            match b {
                Block::Text { text } if !text.is_empty() => parts.push(json!({"text": text})),
                Block::ToolUse {
                    name, input, meta, ..
                } => {
                    let mut part = json!({"functionCall": {"name": name, "args": input}});
                    if let Some(sig) = own_meta(meta, TAG).and_then(|m| m.get("thoughtSignature")) {
                        part["thoughtSignature"] = sig.clone();
                    }
                    parts.push(part);
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let name = names
                        .get(tool_use_id.as_str())
                        .copied()
                        .unwrap_or(tool_use_id);
                    parts.push(json!({"functionResponse": {"name": name, "response": {"content": content}}}));
                }
                other => {
                    let Some(raw) = replayable(other, TAG) else {
                        continue;
                    };
                    // A bare signature belongs on the part before it.
                    match (raw.as_object(), parts.last_mut()) {
                        (Some(o), Some(prev))
                            if o.len() == 1 && o.contains_key("thoughtSignature") =>
                        {
                            prev["thoughtSignature"] = o["thoughtSignature"].clone()
                        }
                        (Some(o), _) if o.len() == 1 && o.contains_key("thoughtSignature") => {}
                        _ => parts.push(raw.clone()),
                    }
                }
            }
        }
        if !parts.is_empty() {
            let role = if m.role == Role::User {
                "user"
            } else {
                "model"
            };
            out.push(json!({"role": role, "parts": parts}));
        }
    }
    out
}

/// A JSON schema cut down to Gemini's OpenAPI subset.
fn schema(v: &Value) -> Value {
    let Some(obj) = v.as_object() else {
        return v.clone();
    };
    let mut out = Map::new();
    for (key, value) in obj {
        match key.as_str() {
            "type" => match value.as_array() {
                // ["string", "null"] → "string" + nullable.
                Some(types) => {
                    if let Some(t) = types.iter().find(|t| t.as_str() != Some("null")) {
                        out.insert("type".into(), t.clone());
                    }
                    if types.iter().any(|t| t.as_str() == Some("null")) {
                        out.insert("nullable".into(), json!(true));
                    }
                }
                None => {
                    out.insert("type".into(), value.clone());
                }
            },
            "properties" => {
                let props: Map<String, Value> = value
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (k.clone(), schema(v)))
                    .collect();
                if !props.is_empty() {
                    out.insert(key.clone(), Value::Object(props));
                }
            }
            "items" => {
                out.insert(key.clone(), schema(value));
            }
            "anyOf" => {
                out.insert(
                    key.clone(),
                    json!(
                        value
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(schema)
                            .collect::<Vec<_>>()
                    ),
                );
            }
            "required" | "enum" | "description" | "format" | "nullable" => {
                out.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    Value::Object(out)
}

fn read_stream(
    body: impl Read,
    sink: &mut dyn FnMut(Delta),
    first_call: usize,
) -> Result<ChatResponse, GatewayError> {
    let mut content: Vec<Block> = Vec::new();
    let mut usage = Usage::default();
    let (mut model, mut finish, mut blocked) = (String::new(), None::<String>, false);
    let mut calls = 0;
    http::sse(body, |_, data| {
        let v: Value = serde_json::from_str(data).map_err(GatewayError::decode)?;
        if let Some(err) = v.get("error") {
            let message = err["message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| err.to_string());
            return Err(GatewayError::Provider {
                message,
                retryable: err["code"].as_u64().is_some_and(|c| c >= 500),
            });
        }
        if let Some(m) = v["modelVersion"].as_str() {
            model = m.to_string();
        }
        let u = &v["usageMetadata"];
        if u.is_object() {
            let n = |k: &str| u[k].as_u64().unwrap_or(0);
            let cached = n("cachedContentTokenCount");
            usage = Usage {
                input_tokens: n("promptTokenCount").saturating_sub(cached),
                output_tokens: n("candidatesTokenCount") + n("thoughtsTokenCount"),
                cache_read_tokens: cached,
                cache_write_tokens: 0,
            };
        }
        blocked |= v["promptFeedback"]["blockReason"].is_string();
        let candidate = &v["candidates"][0];
        for part in candidate["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let text = part["text"].as_str();
            let signature = part.get("thoughtSignature").cloned();
            if part["thought"] == true {
                let text = text.unwrap_or("");
                match content.last_mut() {
                    Some(Block::Thinking { text: acc }) => acc.push_str(text),
                    _ => content.push(Block::Thinking {
                        text: text.to_string(),
                    }),
                }
                if !text.is_empty() {
                    sink(Delta::Thinking {
                        text: text.to_string(),
                    });
                }
            } else if let Some(call) = part.get("functionCall") {
                let name = call["name"].as_str().unwrap_or("").to_string();
                sink(Delta::ToolUse { name: name.clone() });
                let input = if call["args"].is_object() {
                    call["args"].clone()
                } else {
                    json!({})
                };
                let meta = signature.map(|sig| json!({"kind": TAG, "thoughtSignature": sig}));
                content.push(Block::ToolUse {
                    id: format!("call_{}", first_call + calls),
                    name,
                    input,
                    meta,
                });
                calls += 1;
            } else if let Some(text) = text {
                if !text.is_empty() {
                    match content.last_mut() {
                        Some(Block::Text { text: acc }) => acc.push_str(text),
                        _ => content.push(Block::Text {
                            text: text.to_string(),
                        }),
                    }
                    sink(Delta::Text {
                        text: text.to_string(),
                    });
                }
                if let Some(sig) = signature {
                    content.push(Block::Opaque {
                        kind: TAG.into(),
                        raw: json!({"thoughtSignature": sig}),
                    });
                }
            } else {
                content.push(Block::Opaque {
                    kind: TAG.into(),
                    raw: part.clone(),
                });
            }
        }
        if let Some(reason) = candidate["finishReason"].as_str() {
            finish = Some(reason.to_string());
        }
        Ok(true)
    })?;
    content.retain(|b| !matches!(b, Block::Thinking { text } if text.is_empty()));
    let stop = match finish.as_deref() {
        _ if blocked => StopReason::Refusal,
        Some("STOP") | None if calls > 0 => StopReason::ToolUse,
        Some("STOP") | None => StopReason::EndTurn,
        Some("MAX_TOKENS") => StopReason::MaxTokens,
        Some(
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY",
        ) => StopReason::Refusal,
        Some(other) => StopReason::Other(other.to_string()),
    };
    Ok(ChatResponse {
        model,
        content,
        stop,
        usage,
    })
}

/// `GET /v1beta/models`: models that can `generateContent`, without `models/`.
pub(crate) fn list_models(ctx: &Ctx) -> Result<Vec<ModelInfo>, GatewayError> {
    let base = ctx.p.base().unwrap_or(DEFAULT_BASE);
    let mut models = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut url = format!("{base}/v1beta/models?pageSize=1000");
        if let Some(t) = &token {
            url.push_str(&format!("&pageToken={t}"));
        }
        let page = ctx.http().json(&authorize(ctx, Req::get(url))?)?;
        for m in page["models"].as_array().into_iter().flatten() {
            let generates = m["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|ms| ms.iter().any(|x| x == "generateContent"));
            let Some(name) = m["name"].as_str().filter(|_| generates) else {
                continue;
            };
            models.push(ModelInfo {
                id: name.strip_prefix("models/").unwrap_or(name).to_string(),
                name: m["displayName"].as_str().map(str::to_string),
                context: m["inputTokenLimit"].as_u64(),
            });
        }
        match page["nextPageToken"].as_str().filter(|t| !t.is_empty()) {
            Some(t) if token.as_deref() != Some(t) => token = Some(t.to_string()),
            _ => return Ok(models),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeServer, Reply, gateway, provider, request, sse_data};
    use apz_proto::chat::ToolSpec;

    const STREAM: &[&str] = &[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"considering","thought":true}]}}],"modelVersion":"gemini-3-pro"}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"I will "}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"look."},{"functionCall":{"name":"bash","args":{"command":"ls"}},"thoughtSignature":"SIG1"},{"functionCall":{"name":"list","args":{}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":30,"candidatesTokenCount":12,"thoughtsTokenCount":5,"cachedContentTokenCount":10}}"#,
    ];

    #[test]
    fn gemini_request_translation_and_stream_decoding() {
        let server = FakeServer::start(vec![
            Reply::sse(sse_data(STREAM)),
            Reply::sse(sse_data(STREAM)),
        ]);
        let gw = gateway(vec![provider(
            "g",
            ProviderKind::Gemini,
            &server,
            Auth::Env("GEMINI_API_KEY".into()),
        )]);
        let schema = json!({
            "$schema": "x", "type": "object", "additionalProperties": false,
            "properties": {"command": {"type": ["string", "null"], "description": "cmd", "default": "ls", "examples": ["ls"]},
                           "args": {"type": "array", "items": {"type": "string", "minLength": 1}}},
            "required": ["command"],
        });
        let mut req = ChatRequest {
            model: "g/gemini-3-pro".into(),
            system: Some("sys".into()),
            tools: vec![
                ToolSpec {
                    name: "bash".into(),
                    description: "run".into(),
                    input_schema: schema,
                },
                ToolSpec {
                    name: "list".into(),
                    description: "ls".into(),
                    input_schema: json!({"type": "object", "properties": {}}),
                },
            ],
            max_tokens: 1000,
            ..request("hi")
        };
        let mut deltas = Vec::new();
        let resp = gw.complete(&req, &mut |d| deltas.push(d)).unwrap();
        let sent = &server.requests()[0];
        assert_eq!(
            sent.path,
            "/v1beta/models/gemini-3-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(sent.header("x-goog-api-key"), Some("env-GEMINI_API_KEY"));
        let b = sent.json();
        assert_eq!(b["systemInstruction"], json!({"parts": [{"text": "sys"}]}));
        assert_eq!(
            b["generationConfig"],
            json!({"maxOutputTokens": 1000, "thinkingConfig": {"includeThoughts": true}})
        );
        assert_eq!(
            b["tools"][0]["functionDeclarations"],
            json!([
                {"name": "bash", "description": "run", "parameters": {
                    "type": "object", "required": ["command"],
                    "properties": {"command": {"type": "string", "nullable": true, "description": "cmd"},
                                   "args": {"type": "array", "items": {"type": "string"}}}}},
                {"name": "list", "description": "ls"},
            ])
        );

        assert_eq!(resp.model, "g/gemini-3-pro");
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(
            resp.usage,
            Usage {
                input_tokens: 20,
                output_tokens: 17,
                cache_read_tokens: 10,
                cache_write_tokens: 0
            }
        );
        assert_eq!(
            resp.content,
            vec![
                Block::Thinking {
                    text: "considering".into()
                },
                Block::Text {
                    text: "I will look.".into()
                },
                Block::ToolUse {
                    id: "call_0".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                    meta: Some(json!({"kind": "gemini", "thoughtSignature": "SIG1"}))
                },
                Block::ToolUse {
                    id: "call_1".into(),
                    name: "list".into(),
                    input: json!({}),
                    meta: None
                },
            ]
        );
        assert_eq!(
            deltas[0],
            Delta::Thinking {
                text: "considering".into()
            }
        );

        // Next turn: the signature is echoed on its part; responses carry the tool's name.
        req.messages.push(Message::assistant(resp.content));
        req.messages.push(Message {
            role: Role::User,
            content: vec![
                Block::ToolResult {
                    tool_use_id: "call_0".into(),
                    content: "a.txt".into(),
                    is_error: false,
                },
                Block::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "b".into(),
                    is_error: false,
                },
            ],
        });
        let next = gw.complete(&req, &mut |_| {}).unwrap();
        assert!(
            matches!(&next.content[2], Block::ToolUse { id, .. } if id == "call_2"),
            "ids stay unique"
        );
        assert_eq!(
            server.requests()[1].json()["contents"],
            json!([
                {"role": "user", "parts": [{"text": "hi"}]},
                {"role": "model", "parts": [
                    {"text": "I will look."},
                    {"functionCall": {"name": "bash", "args": {"command": "ls"}}, "thoughtSignature": "SIG1"},
                    {"functionCall": {"name": "list", "args": {}}},
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "bash", "response": {"content": "a.txt"}}},
                    {"functionResponse": {"name": "list", "response": {"content": "b"}}},
                ]},
            ])
        );
    }

    #[test]
    fn vertex_gemini_endpoint_and_safety_stop() {
        let stream = sse_data(&[r#"{"candidates":[{"finishReason":"SAFETY"}]}"#]);
        let server = FakeServer::start(vec![Reply::sse(stream)]);
        let mut p = provider(
            "vg",
            ProviderKind::VertexGemini,
            &server,
            Auth::Secret("s".into()),
        );
        p.options = json!({"project": "p1"});
        let gw = gateway(vec![p]);
        let resp = gw
            .complete(
                &ChatRequest {
                    model: "vg/gemini-2.5-pro".into(),
                    ..request("hi")
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(resp.stop, StopReason::Refusal);
        let sent = &server.requests()[0];
        assert_eq!(
            sent.path,
            "/v1/projects/p1/locations/global/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(sent.header("authorization"), Some("Bearer sk-secret-value"));
    }
}
