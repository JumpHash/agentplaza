//! The Anthropic Messages API, direct (`anthropic`) and on Vertex AI
//! (`vertex-anthropic`). Always streamed.

use std::collections::BTreeMap;
use std::io::Read;

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::config::{Auth, ProviderKind};
use crate::http::{self, Req};
use crate::{Ctx, GatewayError, ModelInfo, parse_tool_input, results_first};

const DEFAULT_BASE: &str = "https://api.anthropic.com";
const FIRST_PARTY_HOST: &str = "api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const VERTEX_VERSION: &str = "vertex-2023-10-16";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const DEFAULT_MAX_TOKENS: u32 = 32_000;
/// `pause_turn` continuations per call.
const MAX_CONTINUATIONS: usize = 3;

/// What a Claude model accepts. This is the one place model ids are matched.
#[derive(Debug, PartialEq)]
struct Traits {
    /// Adaptive thinking + `output_config.effort`.
    adaptive_thinking: bool,
    /// Accepts `temperature`.
    temperature: bool,
    /// Supports server-side refusal fallbacks.
    fallbacks: bool,
}

fn traits(model: &str) -> Traits {
    const ADAPTIVE: &[&str] = &[
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude-haiku-5",
    ];
    // 4.7+ and every 5.x reject `temperature`; adaptive-thinking models
    // don't take one either (thinking pins it).
    const NO_TEMPERATURE: &[&str] = &["claude-opus-4-7", "claude-opus-4-8"];
    const FALLBACKS: &[&str] = &[
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-opus-5",
        "claude-sonnet-5-5",
    ];
    let adaptive_thinking = ADAPTIVE.iter().any(|p| family(model, p));
    Traits {
        adaptive_thinking,
        temperature: !adaptive_thinking && !NO_TEMPERATURE.iter().any(|p| family(model, p)),
        fallbacks: FALLBACKS.iter().any(|id| snapshot_of(model, id)),
    }
}

/// `model` is `prefix` or a variant of it (`prefix-…`, `prefix@…`).
fn family(model: &str, prefix: &str) -> bool {
    model
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '@', '[']))
}

/// `model` is `id` or a dated snapshot of it (`id-20260101`, `id@…`).
fn snapshot_of(model: &str, id: &str) -> bool {
    model.strip_prefix(id).is_some_and(|rest| {
        rest.is_empty()
            || rest.starts_with('@')
            || rest
                .strip_prefix('-')
                .is_some_and(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Where and how one call goes.
struct Endpoint {
    url: String,
    vertex: bool,
    /// The request reaches Anthropic's own API (eager tool streaming and
    /// fallbacks are first-party features).
    first_party: bool,
    /// `options.refusal_fallbacks: true`. Off by default: a fallback model may
    /// be one the plaza's model policy does not allow.
    refusal_fallbacks: bool,
}

fn endpoint(ctx: &Ctx, model: &str) -> Result<Endpoint, GatewayError> {
    if ctx.p.kind == ProviderKind::VertexAnthropic {
        let url = ctx.vertex_url("anthropic", model, "streamRawPredict")?;
        return Ok(Endpoint {
            url,
            vertex: true,
            first_party: false,
            refusal_fallbacks: false,
        });
    }
    let base = ctx.p.base().unwrap_or(DEFAULT_BASE);
    let host = base
        .split("://")
        .nth(1)
        .unwrap_or(base)
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    let first_party = ctx
        .p
        .options
        .as_ref()
        .and_then(|o| o.first_party)
        .unwrap_or(host == FIRST_PARTY_HOST);
    let refusal_fallbacks = ctx
        .p
        .options
        .as_ref()
        .and_then(|o| o.refusal_fallbacks)
        .unwrap_or(false);
    Ok(Endpoint {
        url: format!("{base}/v1/messages"),
        vertex: false,
        first_party,
        refusal_fallbacks,
    })
}

/// The request with its auth and beta headers.
fn request(
    ctx: &Ctx,
    url: String,
    body: Option<&Value>,
    betas: &[&str],
) -> Result<Req, GatewayError> {
    let mut req = match body {
        Some(body) => Req::post_json(url, body),
        None => Req::get(url),
    };
    if ctx.p.kind == ProviderKind::VertexAnthropic {
        req = req.bearer(&ctx.required_credential()?);
    } else {
        req.set_header("anthropic-version", API_VERSION);
        if let Some(key) = ctx.credential()? {
            let bearer = matches!(&ctx.p.auth, Auth::Command(_))
                || matches!(&ctx.p.auth, Auth::Env(name) if name == "ANTHROPIC_AUTH_TOKEN")
                || key.starts_with("sk-ant-oat");
            req = if bearer {
                req.bearer(&key)
            } else {
                req.secret_header("x-api-key", &key, &key)
            };
            if bearer {
                return finish(ctx, req, &[betas, &[OAUTH_BETA]].concat());
            }
        }
    }
    finish(ctx, req, betas)
}

/// Applies `extra_headers`, comma-joining any `anthropic-beta` with ours.
fn finish(ctx: &Ctx, req: Req, betas: &[&str]) -> Result<Req, GatewayError> {
    let mut all: Vec<&str> = betas.to_vec();
    let extra_beta = ctx
        .p
        .extra_headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("anthropic-beta"));
    if let Some((_, v)) = extra_beta {
        all.extend(v.split(',').map(str::trim).filter(|b| !b.is_empty()));
    }
    let mut req = req.extra_headers(ctx.p.extra_headers.iter());
    if !all.is_empty() {
        req.set_header("anthropic-beta", &all.join(","));
    }
    Ok(req)
}

/// The request body (before `extra_body`).
fn body(req: &ChatRequest, model: &str, ep: &Endpoint) -> Value {
    let t = traits(model);
    let max_tokens = if req.max_tokens == 0 {
        DEFAULT_MAX_TOKENS
    } else {
        req.max_tokens
    };
    let mut body = json!({
        "max_tokens": max_tokens,
        "messages": messages(&req.messages),
        "stream": true,
        "cache_control": {"type": "ephemeral"},
    });
    if ep.vertex {
        body["anthropic_version"] = json!(VERTEX_VERSION);
    } else {
        body["model"] = json!(model);
    }
    if let Some(system) = req.system.as_deref().filter(|s| !s.is_empty()) {
        body["system"] = json!(system);
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut tool = json!({"name": t.name, "description": t.description, "input_schema": t.input_schema});
                if ep.first_party {
                    tool["eager_input_streaming"] = json!(true);
                }
                tool
            })
            .collect();
        body["tools"] = json!(tools);
        // Forced tool choice is rejected with thinking on; `auto` always works.
        body["tool_choice"] = json!({"type": "auto"});
    }
    if t.adaptive_thinking {
        body["thinking"] = json!({"type": "adaptive"});
        body["output_config"] = json!({"effort": req.effort.as_deref().unwrap_or("high")});
    }
    if let (true, Some(temperature)) = (t.temperature, req.temperature) {
        body["temperature"] = json!((f64::from(temperature) * 1000.0).round() / 1000.0);
    }
    if t.fallbacks && ep.first_party && ep.refusal_fallbacks {
        body["fallbacks"] = json!("default");
    }
    body
}

/// The conversation in Messages API form: display-only thinking and other
/// kinds' opaque state are dropped; Anthropic's own is replayed verbatim.
fn messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|m| {
            let content = blocks(results_first(&m.content));
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            (!content.is_empty()).then(|| json!({"role": role, "content": content}))
        })
        .collect()
}

fn blocks<'b>(blocks: impl IntoIterator<Item = &'b Block>) -> Vec<Value> {
    blocks
        .into_iter()
        .filter_map(|b| match b {
            Block::Text { text } if text.is_empty() => None,
            Block::Text { text } => Some(json!({"type": "text", "text": text})),
            Block::ToolUse {
                id, name, input, ..
            } => Some(json!({"type": "tool_use", "id": id, "name": name, "input": input})),
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let mut v =
                    json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content});
                if *is_error {
                    v["is_error"] = json!(true);
                }
                Some(v)
            }
            Block::Thinking { .. } => None,
            other => crate::replayable(other, "anthropic").cloned(),
        })
        .collect()
}

pub(crate) fn complete(
    ctx: &Ctx,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let ep = endpoint(ctx, model)?;
    let mut body = ctx.finish_body(body(req, model, &ep));
    let betas: &[&str] = if body.get("fallbacks").is_some() {
        &[FALLBACK_BETA]
    } else {
        &[]
    };
    let mut resp = ChatResponse {
        model: model.to_string(),
        content: Vec::new(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let mut continued = false;
    for round in 0..=MAX_CONTINUATIONS {
        let http_req = request(ctx, ep.url.clone(), Some(&body), betas)?;
        let turn = read_stream(http::reader(ctx.http().send(&http_req)?), sink)?;
        resp.usage.add(&turn.usage);
        if !turn.model.is_empty() {
            resp.model = turn.model;
        }
        resp.content.extend(turn.content);
        resp.stop = stop_reason(turn.stop.as_deref());
        if turn.stop.as_deref() != Some("pause_turn") || round == MAX_CONTINUATIONS {
            break;
        }
        // The server paused a long turn: send it back and let it continue.
        let partial = json!({"role": "assistant", "content": blocks(&resp.content)});
        let msgs = body["messages"]
            .as_array_mut()
            .expect("messages is an array");
        if continued {
            msgs.pop();
        }
        msgs.push(partial);
        continued = true;
    }
    Ok(resp)
}

fn stop_reason(stop: Option<&str>) -> StopReason {
    match stop {
        None | Some("end_turn" | "stop_sequence") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens" | "model_context_window_exceeded") => StopReason::MaxTokens,
        Some("refusal") => StopReason::Refusal,
        Some(other) => StopReason::Other(other.to_string()),
    }
}

/// One streamed message.
#[derive(Default)]
struct Turn {
    content: Vec<Block>,
    stop: Option<String>,
    usage: Usage,
    model: String,
}

/// A content block being streamed.
enum Acc {
    Text(String),
    Thinking {
        text: String,
        signature: String,
    },
    Tool {
        id: String,
        name: String,
        json: String,
    },
    /// Any other block, kept verbatim (`redacted_thinking`, `fallback`, server
    /// tools, …); streamed input JSON is collected beside it.
    Raw(Value, String),
}

fn read_stream(body: impl Read, sink: &mut dyn FnMut(Delta)) -> Result<Turn, GatewayError> {
    let mut turn = Turn::default();
    let mut blocks: BTreeMap<u64, Acc> = BTreeMap::new();
    let mut finished = false;
    http::sse(body, |event, data| {
        let v: Value = serde_json::from_str(data).map_err(GatewayError::decode)?;
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        let index = v["index"].as_u64().unwrap_or(0);
        match v["type"].as_str().unwrap_or(event) {
            "message_start" => {
                turn.model = s(&v["message"]["model"]);
                apply_usage(&v["message"]["usage"], &mut turn.usage);
            }
            "content_block_start" => {
                let cb = &v["content_block"];
                let acc = match cb["type"].as_str().unwrap_or("") {
                    "text" => Acc::Text(s(&cb["text"])),
                    "thinking" => Acc::Thinking {
                        text: s(&cb["thinking"]),
                        signature: s(&cb["signature"]),
                    },
                    "tool_use" => {
                        sink(Delta::ToolUse {
                            name: s(&cb["name"]),
                        });
                        Acc::Tool {
                            id: s(&cb["id"]),
                            name: s(&cb["name"]),
                            json: String::new(),
                        }
                    }
                    _ => Acc::Raw(cb.clone(), String::new()),
                };
                blocks.insert(index, acc);
            }
            "content_block_delta" => {
                let d = &v["delta"];
                let field = |name: &str| d[name].as_str().unwrap_or("");
                match (blocks.get_mut(&index), d["type"].as_str().unwrap_or("")) {
                    (Some(Acc::Text(text)), "text_delta") => {
                        text.push_str(field("text"));
                        sink(Delta::Text {
                            text: field("text").to_string(),
                        });
                    }
                    (Some(Acc::Thinking { text, .. }), "thinking_delta") => {
                        text.push_str(field("thinking"));
                        sink(Delta::Thinking {
                            text: field("thinking").to_string(),
                        });
                    }
                    (Some(Acc::Thinking { signature, .. }), "signature_delta") => {
                        signature.push_str(field("signature"))
                    }
                    (Some(Acc::Tool { json, .. } | Acc::Raw(_, json)), "input_json_delta") => {
                        json.push_str(field("partial_json"))
                    }
                    (Some(Acc::Raw(raw, _)), _) => append_strings(raw, d),
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(stop) = v["delta"]["stop_reason"].as_str() {
                    turn.stop = Some(stop.to_string());
                }
                apply_usage(&v["usage"], &mut turn.usage);
            }
            "message_stop" => {
                finished = true;
                return Ok(false);
            }
            "error" => {
                let kind = s(&v["error"]["type"]);
                let retryable = matches!(
                    kind.as_str(),
                    "overloaded_error" | "api_error" | "rate_limit_error" | "timeout_error"
                );
                return Err(GatewayError::Provider {
                    message: format!("{kind}: {}", s(&v["error"]["message"])),
                    retryable,
                });
            }
            _ => {}
        }
        Ok(true)
    })?;
    if !finished {
        return Err(GatewayError::Transport(
            "stream ended before message_stop".into(),
        ));
    }
    for acc in blocks.into_values() {
        match acc {
            Acc::Text(text) if text.is_empty() => {}
            Acc::Text(text) => turn.content.push(Block::Text { text }),
            Acc::Thinking { text, signature } => {
                let raw = json!({"type": "thinking", "thinking": text, "signature": signature});
                if !text.is_empty() {
                    turn.content.push(Block::Thinking { text });
                }
                turn.content.push(Block::Opaque {
                    kind: "anthropic".into(),
                    raw,
                });
            }
            Acc::Tool { id, name, json } => {
                let (input, meta) = parse_tool_input(&json);
                turn.content.push(Block::ToolUse {
                    id,
                    name,
                    input,
                    meta,
                });
            }
            Acc::Raw(mut raw, json) => {
                if let Ok(input) = serde_json::from_str::<Value>(&json) {
                    raw["input"] = input;
                }
                turn.content.push(Block::Opaque {
                    kind: "anthropic".into(),
                    raw,
                });
            }
        }
    }
    Ok(turn)
}

/// Appends a delta's string fields to the same-named string fields of an
/// unknown block, so its streamed text is not lost.
fn append_strings(raw: &mut Value, delta: &Value) {
    let Some(delta) = delta.as_object() else {
        return;
    };
    for (key, value) in delta.iter().filter(|(k, _)| *k != "type") {
        if let (Some(Value::String(dst)), Some(src)) = (raw.get_mut(key), value.as_str()) {
            dst.push_str(src);
        }
    }
}

/// Overwrites the counters `u` carries (stream usage is cumulative).
fn apply_usage(u: &Value, usage: &mut Usage) {
    let set = |field: &str, slot: &mut u64| {
        if let Some(n) = u[field].as_u64() {
            *slot = n;
        }
    };
    set("input_tokens", &mut usage.input_tokens);
    set("output_tokens", &mut usage.output_tokens);
    set("cache_read_input_tokens", &mut usage.cache_read_tokens);
    set("cache_creation_input_tokens", &mut usage.cache_write_tokens);
}

/// `GET /v1/models`, following `has_more` / `last_id`.
pub(crate) fn list_models(ctx: &Ctx) -> Result<Vec<ModelInfo>, GatewayError> {
    let base = ctx.p.base().unwrap_or(DEFAULT_BASE);
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut url = format!("{base}/v1/models?limit=1000");
        if let Some(after) = &after {
            url.push_str(&format!("&after_id={after}"));
        }
        let page = ctx.http().json(&request(ctx, url, None, &[])?)?;
        for m in page["data"].as_array().into_iter().flatten() {
            let Some(id) = m["id"].as_str() else { continue };
            models.push(ModelInfo {
                id: id.to_string(),
                name: m["display_name"].as_str().map(str::to_string),
                context: m["max_input_tokens"].as_u64(),
            });
        }
        match (page["has_more"].as_bool(), page["last_id"].as_str()) {
            (Some(true), Some(last)) if after.as_deref() != Some(last) => {
                after = Some(last.to_string())
            }
            _ => return Ok(models),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeServer, Reply, gateway, provider, sse_body};
    use apz_proto::chat::ToolSpec;

    fn tool() -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: "run".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn history() -> Vec<Message> {
        vec![
            Message::user_text("list files"),
            Message::assistant(vec![
                Block::Thinking {
                    text: "display only".into(),
                },
                Block::Opaque {
                    kind: "anthropic".into(),
                    raw: json!({"type": "thinking", "thinking": "t", "signature": "sig"}),
                },
                Block::Opaque {
                    kind: "gemini".into(),
                    raw: json!({"thoughtSignature": "g"}),
                },
                Block::ToolUse {
                    id: "tu1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                    meta: None,
                },
            ]),
            Message {
                role: Role::User,
                content: vec![
                    Block::Text {
                        text: "and then?".into(),
                    },
                    Block::ToolResult {
                        tool_use_id: "tu1".into(),
                        content: "a.txt".into(),
                        is_error: true,
                    },
                ],
            },
        ]
    }

    const TEXT_STREAM: &[(&str, &str)] = &[
        (
            "message_start",
            r#"{"type":"message_start","message":{"model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":4,"cache_creation_input_tokens":2,"output_tokens":1}}}"#,
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG"}}"#,
        ),
        (
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        ),
        ("ping", r#"{"type":"ping"}"#),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"lo"}}"#,
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"redacted_thinking","data":"xyz"}}"#,
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"tu9","name":"bash","input":{}}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":" \"ls\"}"}}"#,
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":4,"content_block":{"type":"tool_use","id":"tu10","name":"bash","input":{}}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":4,"delta":{"type":"input_json_delta","partial_json":"{\"command\": \"l"}}"#,
        ),
        (
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}"#,
        ),
        ("message_stop", r#"{"type":"message_stop"}"#),
    ];

    #[test]
    fn request_translation_for_an_adaptive_first_party_model() {
        let server = FakeServer::start(vec![Reply::sse(sse_body(TEXT_STREAM))]);
        let mut p = provider(
            "a",
            ProviderKind::Anthropic,
            &server,
            Auth::Env("ANTHROPIC_API_KEY".into()),
        );
        p.options = Some(
            serde_json::from_value(json!({"first_party": true, "refusal_fallbacks": true}))
                .unwrap(),
        );
        p.extra_headers
            .insert("anthropic-beta".into(), "extra-beta".into());
        let gw = gateway(vec![p]);
        let req = ChatRequest {
            model: "a/claude-opus-5-5".into(),
            system: Some("be brief".into()),
            messages: history(),
            tools: vec![tool()],
            max_tokens: 0,
            temperature: Some(0.5),
            effort: Some("medium".into()),
        };
        gw.complete(&req, &mut |_| {}).unwrap();
        let sent = &server.requests()[0];
        assert_eq!(sent.path, "/v1/messages");
        assert_eq!(sent.header("x-api-key"), Some("env-ANTHROPIC_API_KEY"));
        assert_eq!(sent.header("anthropic-version"), Some(API_VERSION));
        assert_eq!(
            sent.header("anthropic-beta"),
            Some("server-side-fallback-2026-07-01,extra-beta")
        );
        let b = sent.json();
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["max_tokens"], 32000);
        assert_eq!(b["stream"], true);
        assert_eq!(b["system"], "be brief");
        assert_eq!(b["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(b["thinking"], json!({"type": "adaptive"}));
        assert_eq!(b["output_config"], json!({"effort": "medium"}));
        assert_eq!(b["fallbacks"], "default");
        assert!(b.get("temperature").is_none());
        assert_eq!(b["tool_choice"], json!({"type": "auto"}));
        assert_eq!(
            b["tools"][0],
            json!({"name": "bash", "description": "run", "input_schema": {"type": "object"}, "eager_input_streaming": true})
        );
        // Opaque anthropic state replayed verbatim; display thinking and gemini state dropped.
        assert_eq!(
            b["messages"][1]["content"],
            json!([
                {"type": "thinking", "thinking": "t", "signature": "sig"},
                {"type": "tool_use", "id": "tu1", "name": "bash", "input": {"command": "ls"}},
            ])
        );
        // Tool results come before text.
        assert_eq!(
            b["messages"][2]["content"],
            json!([
                {"type": "tool_result", "tool_use_id": "tu1", "content": "a.txt", "is_error": true},
                {"type": "text", "text": "and then?"},
            ])
        );
    }

    #[test]
    fn haiku_and_bearer_tokens_get_neither_thinking_nor_fallbacks() {
        let server = FakeServer::start(vec![Reply::sse(sse_body(TEXT_STREAM))]);
        let p = provider(
            "a",
            ProviderKind::Anthropic,
            &server,
            Auth::Env("ANTHROPIC_AUTH_TOKEN".into()),
        );
        let gw = gateway(vec![p]);
        let req = ChatRequest {
            model: "a/claude-haiku-4-5-20251001".into(),
            system: None,
            messages: vec![Message::user_text("hi")],
            tools: vec![tool()],
            max_tokens: 100,
            temperature: Some(0.25),
            effort: Some("low".into()),
        };
        gw.complete(&req, &mut |_| {}).unwrap();
        let sent = &server.requests()[0];
        assert_eq!(
            sent.header("authorization"),
            Some("Bearer env-ANTHROPIC_AUTH_TOKEN")
        );
        assert_eq!(sent.header("anthropic-beta"), Some(OAUTH_BETA));
        assert!(sent.header("x-api-key").is_none());
        let b = sent.json();
        assert_eq!(b["max_tokens"], 100);
        assert_eq!(b["temperature"], 0.25);
        assert!(
            b.get("thinking").is_none()
                && b.get("output_config").is_none()
                && b.get("fallbacks").is_none()
        );
        // Not first party (a local URL): no eager streaming flag.
        assert!(b["tools"][0].get("eager_input_streaming").is_none());
    }

    #[test]
    fn model_table() {
        assert!(traits("claude-sonnet-4-6").adaptive_thinking);
        assert!(!traits("claude-sonnet-4-5").adaptive_thinking);
        assert!(traits("claude-sonnet-4-5").temperature);
        assert!(!traits("claude-opus-4-7").temperature);
        assert!(traits("claude-opus-5").fallbacks && traits("claude-fable-5-1").fallbacks);
        assert!(
            !traits("claude-opus-5-1").fallbacks && traits("claude-opus-5-1").adaptive_thinking
        );
        assert!(
            !traits("claude-haiku-4-5").adaptive_thinking && !traits("claude-haiku-4-5").fallbacks
        );
        assert!(
            traits("claude-haiku-5-5").adaptive_thinking && !traits("claude-haiku-5-5").temperature
        );
    }

    #[test]
    fn effort_pin_haiku_5_and_fallbacks_off_by_default() {
        let server = FakeServer::start(vec![
            Reply::sse(sse_body(TEXT_STREAM)),
            Reply::sse(sse_body(TEXT_STREAM)),
        ]);
        let mut p = provider("a", ProviderKind::Anthropic, &server, Auth::None);
        p.options = Some(serde_json::from_value(json!({"first_party": true})).unwrap());
        let gw = gateway(vec![p]);
        let pinned = ChatRequest {
            model: "a/claude-haiku-5-5@low".into(),
            effort: Some("high".into()),
            ..crate::testutil::request("hi")
        };
        gw.complete(&pinned, &mut |_| {}).unwrap();
        gw.complete(
            &ChatRequest {
                model: "a/claude-opus-5-5".into(),
                ..crate::testutil::request("hi")
            },
            &mut |_| {},
        )
        .unwrap();
        let sent = server.requests();
        let haiku = sent[0].json();
        assert_eq!(haiku["model"], "claude-haiku-5-5");
        assert_eq!(haiku["thinking"], json!({"type": "adaptive"}));
        assert_eq!(haiku["output_config"], json!({"effort": "low"}));
        let opus = sent[1].json();
        assert_eq!(opus["output_config"], json!({"effort": "high"}));
        assert!(opus.get("fallbacks").is_none() && sent[1].header("anthropic-beta").is_none());
    }

    #[test]
    fn stream_decoding() {
        let server = FakeServer::start(vec![Reply::sse(sse_body(TEXT_STREAM))]);
        let gw = gateway(vec![provider(
            "a",
            ProviderKind::Anthropic,
            &server,
            Auth::None,
        )]);
        let req = ChatRequest {
            model: "a/claude-opus-5-5".into(),
            ..crate::testutil::request("hi")
        };
        let mut deltas = Vec::new();
        let resp = gw.complete(&req, &mut |d| deltas.push(d)).unwrap();
        assert_eq!(resp.model, "a/claude-opus-5-5");
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(
            resp.usage,
            Usage {
                input_tokens: 10,
                output_tokens: 42,
                cache_read_tokens: 4,
                cache_write_tokens: 2
            }
        );
        assert_eq!(
            &resp.content[..4],
            &[
                Block::Thinking { text: "hmm".into() },
                Block::Opaque {
                    kind: "anthropic".into(),
                    raw: json!({"type": "thinking", "thinking": "hmm", "signature": "SIG"})
                },
                Block::Text {
                    text: "Hello".into()
                },
                Block::Opaque {
                    kind: "anthropic".into(),
                    raw: json!({"type": "redacted_thinking", "data": "xyz"})
                },
            ]
        );
        assert_eq!(
            resp.content[4],
            Block::ToolUse {
                id: "tu9".into(),
                name: "bash".into(),
                input: json!({"command": "ls"}),
                meta: None
            }
        );
        // Eager streaming may cut the JSON short: no input, an INVALID_JSON note.
        let Block::ToolUse { input, meta, .. } = &resp.content[5] else {
            panic!("{:?}", resp.content[5])
        };
        assert_eq!(input, &json!({}));
        assert_eq!(
            crate::tool_input_error(meta.as_ref()),
            Some(crate::INVALID_JSON)
        );
        assert_eq!(
            deltas[..3],
            [
                Delta::Thinking { text: "hmm".into() },
                Delta::Text { text: "Hel".into() },
                Delta::Text { text: "lo".into() },
            ]
        );
        assert!(deltas.contains(&Delta::ToolUse {
            name: "bash".into()
        }));
    }

    #[test]
    fn pause_turn_continues_with_the_partial_content() {
        let paused = sse_body(&[
            (
                "message_start",
                r#"{"type":"message_start","message":{"model":"m","usage":{"input_tokens":5}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"part one"}}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"pause_turn"},"usage":{"output_tokens":3}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ]);
        let done = sse_body(&[
            (
                "message_start",
                r#"{"type":"message_start","message":{"model":"m","usage":{"input_tokens":7}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"part two"}}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ]);
        let server = FakeServer::start(vec![Reply::sse(paused), Reply::sse(done)]);
        let gw = gateway(vec![provider(
            "a",
            ProviderKind::Anthropic,
            &server,
            Auth::None,
        )]);
        let resp = gw
            .complete(
                &ChatRequest {
                    model: "a/m".into(),
                    ..crate::testutil::request("go")
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(resp.text(), "part one\npart two");
        assert_eq!(resp.stop, StopReason::EndTurn);
        assert_eq!((resp.usage.input_tokens, resp.usage.output_tokens), (12, 7));
        let second = server.requests()[1].json();
        assert_eq!(
            second["messages"][1],
            json!({"role": "assistant", "content": [{"type": "text", "text": "part one"}]})
        );
    }

    #[test]
    fn vertex_anthropic_body_and_command_token() {
        let server = FakeServer::start(vec![Reply::sse(sse_body(TEXT_STREAM))]);
        let mut p = provider(
            "v",
            ProviderKind::VertexAnthropic,
            &server,
            Auth::Command("printf 'ya29.tok\\n'".into()),
        );
        p.options = Some(
            serde_json::from_value(json!({"project": "proj", "location": "us-east5"})).unwrap(),
        );
        let gw = gateway(vec![p]);
        let req = ChatRequest {
            model: "v/claude-opus-5-5".into(),
            ..crate::testutil::request("hi")
        };
        gw.complete(&req, &mut |_| {}).unwrap();
        let sent = &server.requests()[0];
        assert_eq!(
            sent.path,
            "/v1/projects/proj/locations/us-east5/publishers/anthropic/models/claude-opus-5-5:streamRawPredict"
        );
        assert_eq!(sent.header("authorization"), Some("Bearer ya29.tok"));
        let b = sent.json();
        assert_eq!(b["anthropic_version"], VERTEX_VERSION);
        assert!(b.get("model").is_none() && b.get("fallbacks").is_none());
        assert!(sent.header("anthropic-beta").is_none());
    }
}
