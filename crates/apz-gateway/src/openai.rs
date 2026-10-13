//! OpenAI Chat Completions: `openai`, `azure-openai` (v1 API) and every
//! `openai-compatible` server. Streamed.

use std::collections::BTreeMap;
use std::io::Read;

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::config::{Auth, ProviderKind};
use crate::http::{self, Req};
use crate::{Ctx, GatewayError, ModelInfo, parse_tool_input, replayable, tool_use_count};

pub(crate) const DEFAULT_BASE: &str = "https://api.openai.com/v1";
/// The opaque kind of a turn's reasoning text (a JSON string).
const TAG: &str = "openai-reasoning";

/// The API root (`…/v1`).
pub(crate) fn base(ctx: &Ctx) -> Result<String, GatewayError> {
    match (ctx.p.kind, ctx.p.base()) {
        (ProviderKind::AzureOpenAi, Some(base)) if base.ends_with("/openai/v1") => Ok(base.to_string()),
        (ProviderKind::AzureOpenAi, Some(base)) => Ok(format!("{base}/openai/v1")),
        (_, Some(base)) => Ok(base.to_string()),
        (ProviderKind::OpenAi | ProviderKind::OpenAiResponses, None) => Ok(DEFAULT_BASE.to_string()),
        _ => Err(GatewayError::Config(format!("provider `{}` needs a base_url", ctx.p.id))),
    }
}

/// Adds the credential (`api-key` for Azure keys, bearer otherwise) and
/// `extra_headers`.
pub(crate) fn authorize(ctx: &Ctx, req: Req) -> Result<Req, GatewayError> {
    let req = match ctx.credential()? {
        None => req,
        Some(key) if ctx.p.kind == ProviderKind::AzureOpenAi && !matches!(ctx.p.auth, Auth::Command(_)) => {
            req.secret_header("api-key", &key, &key)
        }
        Some(token) => req.bearer(&token),
    };
    Ok(req.extra_headers(ctx.p.extra_headers.iter()))
}

/// Whether this provider takes `reasoning_effort` (passed through as given):
/// the `openai` kind, xAI, or any provider with `options.reasoning_effort`.
fn sends_effort(ctx: &Ctx) -> bool {
    ctx.p.kind == ProviderKind::OpenAi
        || ctx.p.options.get("reasoning_effort").and_then(Value::as_bool).unwrap_or(false)
        || ctx.p.base().is_some_and(|b| b.starts_with("https://api.x.ai/"))
}

/// Whether this provider needs the assistant's reasoning sent back
/// (`options.replay_reasoning`: DeepSeek, once tools are in use).
fn replays_reasoning(ctx: &Ctx) -> bool {
    ctx.p.options.get("replay_reasoning").and_then(Value::as_bool).unwrap_or(false)
}

/// Whether this provider is Mistral (`options.mistral_compat`): its tool-call
/// ids must be nine alphanumerics, and it takes `max_tokens`.
fn mistral_compat(ctx: &Ctx) -> bool {
    ctx.p.options.get("mistral_compat").and_then(Value::as_bool).unwrap_or(false)
}

fn body(ctx: &Ctx, req: &ChatRequest, model: &str, stream_options: bool) -> Value {
    let compat = mistral_compat(ctx);
    let mut body = json!({"model": model, "messages": messages(req, replays_reasoning(ctx), compat), "stream": true});
    if stream_options {
        body["stream_options"] = json!({"include_usage": true});
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}}))
            .collect();
        body["tools"] = json!(tools);
    }
    if req.max_tokens > 0 {
        let field = ctx.p.option("max_tokens_field").unwrap_or(match ctx.p.kind {
            ProviderKind::OpenAi if !compat => "max_completion_tokens",
            _ => "max_tokens",
        });
        body[field] = json!(req.max_tokens);
    }
    if let (true, Some(effort)) = (sends_effort(ctx), req.effort.as_deref()) {
        body["reasoning_effort"] = json!(effort);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!((f64::from(t) * 1000.0).round() / 1000.0);
    }
    ctx.finish_body(body)
}

/// The conversation as Chat Completions messages: tool results become `tool`
/// messages (ahead of the user's text). Reasoning is dropped unless `replay`,
/// in which case an assistant turn's reasoning text goes back as
/// `reasoning_content`. With `compat`, tool-call ids are sent in Mistral's form.
fn messages(req: &ChatRequest, replay: bool, compat: bool) -> Vec<Value> {
    let mut out = Vec::new();
    let call_id = |id: &str| if compat { compat_id(id) } else { id.to_string() };
    if let Some(system) = req.system.as_deref().filter(|s| !s.is_empty()) {
        out.push(json!({"role": "system", "content": system}));
    }
    for m in &req.messages {
        let text = m.text();
        match m.role {
            Role::User => {
                for b in &m.content {
                    if let Block::ToolResult { tool_use_id, content, is_error } = b {
                        let content = if *is_error { format!("Error: {content}") } else { content.clone() };
                        out.push(json!({"role": "tool", "tool_call_id": call_id(tool_use_id), "content": content}));
                    }
                }
                if !text.is_empty() {
                    out.push(json!({"role": "user", "content": text}));
                }
            }
            Role::Assistant => {
                let calls: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolUse { id, name, input, .. } => Some(json!({
                            "id": call_id(id), "type": "function",
                            "function": {"name": name, "arguments": input.to_string()},
                        })),
                        _ => None,
                    })
                    .collect();
                let reasoning = m.content.iter().find_map(|b| replayable(b, TAG)).and_then(Value::as_str).filter(|_| replay);
                if text.is_empty() && calls.is_empty() && reasoning.is_none() {
                    continue;
                }
                let mut msg = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                if let Some(reasoning) = reasoning {
                    msg["reasoning_content"] = json!(reasoning);
                }
                out.push(msg);
            }
        }
    }
    out
}

/// A tool-call id as Mistral wants it: its own id when that is nine ASCII
/// letters or digits, else a deterministic nine-character stand-in (a hash of
/// the id), so the same id always maps the same way and a call still pairs
/// with its result.
fn compat_id(id: &str) -> String {
    if id.len() == 9 && id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return id.to_string();
    }
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    // FNV-1a, 64 bit.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let mut n = hash;
    (0..9)
        .map(|_| {
            let c = char::from(ALPHABET[(n % 62) as usize]);
            n /= 62;
            c
        })
        .collect()
}

/// A delta's `content`: a plain string, or Mistral's chunks, `text` parts
/// (`{"type":"text","text"}`) and `thinking` parts (`{"type":"thinking",
/// "thinking":[{"type":"text","text"}, …]}`). Returns (answer, reasoning).
fn content_parts(content: &Value) -> (String, String) {
    let (mut answer, mut reasoning) = (String::new(), String::new());
    match content {
        Value::String(s) => answer.push_str(s),
        Value::Array(chunks) => {
            for chunk in chunks {
                if chunk["type"] == "thinking" {
                    for part in chunk["thinking"].as_array().into_iter().flatten() {
                        reasoning.push_str(part["text"].as_str().unwrap_or(""));
                    }
                } else {
                    answer.push_str(chunk["text"].as_str().unwrap_or(""));
                }
            }
        }
        _ => {}
    }
    (answer, reasoning)
}

pub(crate) fn complete(
    ctx: &Ctx,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let url = format!("{}/chat/completions", base(ctx)?);
    let mut stream_options = !ctx.gw.no_stream_options.lock().unwrap_or_else(|e| e.into_inner()).contains(&ctx.p.id);
    let resp = loop {
        let http_req = authorize(ctx, Req::post_json(&url, &body(ctx, req, model, stream_options)))?;
        match ctx.http().send(&http_req) {
            // Some compatible servers reject `stream_options`: drop it, for good.
            Err(GatewayError::Http { status: 400, body, .. }) if stream_options && body.contains("stream_options") => {
                ctx.gw.no_stream_options.lock().unwrap_or_else(|e| e.into_inner()).insert(ctx.p.id.clone());
                stream_options = false;
            }
            other => break other?,
        }
    };
    read_stream(http::reader(resp), sink, tool_use_count(&req.messages), replays_reasoning(ctx))
}

/// A tool call being streamed.
#[derive(Default)]
struct Call {
    id: String,
    name: String,
    args: String,
}

fn read_stream(
    body: impl Read,
    sink: &mut dyn FnMut(Delta),
    first_call: usize,
    keep_reasoning: bool,
) -> Result<ChatResponse, GatewayError> {
    let (mut text, mut thinking, mut model) = (String::new(), String::new(), String::new());
    let mut calls: BTreeMap<u64, Call> = BTreeMap::new();
    let mut usage = Usage::default();
    let mut finish: Option<String> = None;
    let mut refused = false;
    let mut done = false;
    http::sse(body, |_, data| {
        if data.trim() == "[DONE]" {
            done = true;
            return Ok(false);
        }
        let v: Value = serde_json::from_str(data).map_err(GatewayError::decode)?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            let message = err["message"].as_str().map(str::to_string).unwrap_or_else(|| err.to_string());
            return Err(GatewayError::Provider { message, retryable: false });
        }
        if let Some(m) = v["model"].as_str().filter(|_| model.is_empty()) {
            model = m.to_string();
        }
        if v["usage"].is_object() {
            let u = &v["usage"];
            // OpenAI's `prompt_tokens` includes its cache hits; DeepSeek's splits the prompt into
            // hits and misses, and its miss count is the input outside the cache.
            let cached = u["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .or_else(|| u["prompt_cache_hit_tokens"].as_u64())
                .unwrap_or(0);
            let prompt = u["prompt_tokens"].as_u64().unwrap_or(0);
            usage.input_tokens = match u["prompt_cache_miss_tokens"].as_u64() {
                Some(miss) => miss,
                None => prompt.saturating_sub(cached),
            };
            usage.output_tokens = u["completion_tokens"].as_u64().unwrap_or(0);
            usage.cache_read_tokens = cached;
        }
        for choice in v["choices"].as_array().into_iter().flatten() {
            let d = &choice["delta"];
            // DeepSeek / vLLM say `reasoning_content`; Ollama / OpenRouter say `reasoning`.
            let own = d["reasoning_content"].as_str().or(d["reasoning"].as_str()).unwrap_or("");
            // Mistral's content may come as chunks, thinking ones included.
            let (answer, chunk_reasoning) = content_parts(&d["content"]);
            for reasoning in [own, chunk_reasoning.as_str()] {
                if !reasoning.is_empty() {
                    thinking.push_str(reasoning);
                    sink(Delta::Thinking { text: reasoning.to_string() });
                }
            }
            let refusal = d["refusal"].as_str().unwrap_or("");
            for (is_refusal, t) in [(false, answer.as_str()), (true, refusal)] {
                if !t.is_empty() {
                    refused |= is_refusal;
                    text.push_str(t);
                    sink(Delta::Text { text: t.to_string() });
                }
            }
            for (pos, tc) in d["tool_calls"].as_array().into_iter().flatten().enumerate() {
                let call = calls.entry(tc["index"].as_u64().unwrap_or(pos as u64)).or_default();
                if let Some(id) = tc["id"].as_str().filter(|s| !s.is_empty()) {
                    call.id = id.to_string();
                }
                if let Some(name) = tc["function"]["name"].as_str().filter(|s| !s.is_empty()) {
                    if call.name.is_empty() {
                        sink(Delta::ToolUse { name: name.to_string() });
                    }
                    call.name = name.to_string();
                }
                call.args.push_str(tc["function"]["arguments"].as_str().unwrap_or(""));
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                finish = Some(reason.to_string());
            }
        }
        Ok(true)
    })?;
    if !done && finish.is_none() {
        return Err(GatewayError::Transport("stream ended before the answer finished".into()));
    }
    let mut content = Vec::new();
    if !thinking.is_empty() {
        // Kept whole only for a provider that needs it back (`replay_reasoning`).
        if keep_reasoning {
            content.push(Block::Opaque { kind: TAG.into(), raw: Value::String(thinking.clone()) });
        }
        content.push(Block::Thinking { text: thinking });
    }
    if !text.is_empty() {
        content.push(Block::Text { text });
    }
    let has_calls = !calls.is_empty();
    for (n, call) in calls.into_values().enumerate() {
        let (input, meta) = parse_tool_input(&call.args);
        let id = if call.id.is_empty() { format!("call_{}", first_call + n) } else { call.id };
        content.push(Block::ToolUse { id, name: call.name, input, meta });
    }
    let stop = match finish.as_deref() {
        _ if refused => StopReason::Refusal,
        Some("tool_calls" | "function_call") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::Refusal,
        None | Some("stop") if has_calls => StopReason::ToolUse,
        None | Some("stop") => StopReason::EndTurn,
        Some(other) => StopReason::Other(other.to_string()),
    };
    Ok(ChatResponse { model, content, stop, usage })
}

/// `GET {base}/models`.
pub(crate) fn list_models(ctx: &Ctx) -> Result<Vec<ModelInfo>, GatewayError> {
    let page = ctx.http().json(&authorize(ctx, Req::get(format!("{}/models", base(ctx)?)))?)?;
    Ok(page["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ModelInfo {
                id: m["id"].as_str()?.to_string(),
                name: m["name"].as_str().map(str::to_string),
                context: ["context_length", "context_window", "max_model_len"].iter().find_map(|k| m[*k].as_u64()),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderConfig;
    use crate::testutil::{FakeServer, Reply, gateway, provider, request, sse_data};
    use apz_proto::chat::{Message, ToolSpec};

    fn conversation() -> ChatRequest {
        ChatRequest {
            system: Some("sys".into()),
            messages: vec![
                Message::user_text("hi"),
                Message::assistant(vec![
                    Block::Thinking { text: "x".into() },
                    Block::Text { text: "running".into() },
                    Block::ToolUse { id: "c1".into(), name: "bash".into(), input: json!({"command": "ls"}), meta: None },
                    Block::ToolUse { id: "c2".into(), name: "list".into(), input: json!({}), meta: None },
                ]),
                Message {
                    role: Role::User,
                    content: vec![
                        Block::ToolResult { tool_use_id: "c1".into(), content: "a".into(), is_error: false },
                        Block::ToolResult { tool_use_id: "c2".into(), content: "denied".into(), is_error: true },
                        Block::Text { text: "go on".into() },
                    ],
                },
            ],
            tools: vec![ToolSpec { name: "bash".into(), description: "d".into(), input_schema: json!({"type": "object"}) }],
            max_tokens: 500,
            effort: Some("xhigh".into()),
            temperature: None,
            ..request("")
        }
    }

    /// The reasoning state a turn keeps for replay.
    fn opaque(reasoning: &str) -> Block {
        Block::Opaque { kind: TAG.into(), raw: json!(reasoning) }
    }

    const STREAM: &[&str] = &[
        r#"{"model":"gpt-5-2025","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"think"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"content":"Let me "}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"content":"look."}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"bash","arguments":""}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"list","arguments":"{}"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":60}}}"#,
        "[DONE]",
    ];

    #[test]
    fn openai_request_translation_and_stream_decoding() {
        let server = FakeServer::start(vec![Reply::sse(sse_data(STREAM))]);
        let gw = gateway(vec![provider("o", ProviderKind::OpenAi, &server, Auth::Env("OPENAI_API_KEY".into()))]);
        let mut deltas = Vec::new();
        let resp = gw.complete(&ChatRequest { model: "o/gpt-6-astra".into(), ..conversation() }, &mut |d| deltas.push(d)).unwrap();
        let sent = &server.requests()[0];
        assert_eq!(sent.path, "/chat/completions");
        assert_eq!(sent.header("authorization"), Some("Bearer env-OPENAI_API_KEY"));
        let b = sent.json();
        assert_eq!(b["stream_options"], json!({"include_usage": true}));
        assert_eq!(b["max_completion_tokens"], 500);
        assert_eq!(b["reasoning_effort"], "xhigh");
        assert_eq!(b["tools"][0], json!({"type": "function", "function": {"name": "bash", "description": "d", "parameters": {"type": "object"}}}));
        assert_eq!(b["messages"], json!([
            {"role": "system", "content": "sys"},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "running", "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "bash", "arguments": "{\"command\":\"ls\"}"}},
                {"id": "c2", "type": "function", "function": {"name": "list", "arguments": "{}"}},
            ]},
            {"role": "tool", "tool_call_id": "c1", "content": "a"},
            {"role": "tool", "tool_call_id": "c2", "content": "Error: denied"},
            {"role": "user", "content": "go on"},
        ]));

        assert_eq!(resp.model, "o/gpt-5-2025");
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert_eq!(resp.usage, Usage { input_tokens: 40, output_tokens: 20, cache_read_tokens: 60, cache_write_tokens: 0 });
        assert_eq!(resp.content, vec![
            Block::Thinking { text: "think".into() },
            Block::Text { text: "Let me look.".into() },
            Block::ToolUse { id: "call_a".into(), name: "bash".into(), input: json!({"command": "ls"}), meta: None },
            Block::ToolUse { id: "call_b".into(), name: "list".into(), input: json!({}), meta: None },
        ]);
        assert_eq!(deltas[0], Delta::Thinking { text: "think".into() });
        assert_eq!(deltas.iter().filter(|d| matches!(d, Delta::ToolUse { .. })).count(), 2);
    }

    /// The usage a streamed answer ends with, read through `read_stream`.
    fn usage_of(usage: &str) -> Usage {
        let sse = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"hi\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: {{\"choices\":[],\"usage\":{usage}}}\n\ndata: [DONE]\n\n");
        read_stream(sse.as_bytes(), &mut |_| {}, 0, false).unwrap().usage
    }

    #[test]
    fn deepseek_usage_bills_its_misses_as_input_and_meters_its_hits_as_cache_reads() {
        let with_miss = usage_of(r#"{"prompt_tokens":100,"completion_tokens":5,"prompt_cache_hit_tokens":70,"prompt_cache_miss_tokens":30}"#);
        assert_eq!(with_miss, Usage { input_tokens: 30, output_tokens: 5, cache_read_tokens: 70, cache_write_tokens: 0 });
        let hit_only = usage_of(r#"{"prompt_tokens":100,"completion_tokens":5,"prompt_cache_hit_tokens":70}"#);
        assert_eq!(hit_only, Usage { input_tokens: 30, output_tokens: 5, cache_read_tokens: 70, cache_write_tokens: 0 });
    }

    #[test]
    fn openai_cached_prompt_tokens_leave_the_input_and_never_underflow_it() {
        let cached = usage_of(r#"{"prompt_tokens":2000,"completion_tokens":9,"prompt_tokens_details":{"cached_tokens":1920}}"#);
        assert_eq!(cached, Usage { input_tokens: 80, output_tokens: 9, cache_read_tokens: 1920, cache_write_tokens: 0 });
        let over = usage_of(r#"{"prompt_tokens":5,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":9}}"#);
        assert_eq!(over, Usage { input_tokens: 0, output_tokens: 1, cache_read_tokens: 9, cache_write_tokens: 0 });
    }

    #[test]
    fn compatible_server_without_stream_options_or_key() {
        let stream = sse_data(&[
            r#"{"model":"qwen3:8b","choices":[{"delta":{"reasoning":"hmm "}}]}"#,
            r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":"length"}]}"#,
            "[DONE]",
        ]);
        let rejected = Reply::json(400, &json!({"error": {"message": "unknown field stream_options"}}));
        let server = FakeServer::start(vec![rejected, Reply::sse(stream.clone()), Reply::sse(stream)]);
        let mut p = provider("ollama", ProviderKind::OpenAiCompatible, &server, Auth::None);
        p.extra_body = json!({"keep_alive": "5m"});
        let gw = gateway(vec![p]);
        let req = ChatRequest { model: "ollama/qwen3:8b".into(), max_tokens: 64, effort: Some("high".into()), ..request("hi") };
        let resp = gw.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(resp.content, vec![Block::Thinking { text: "hmm ".into() }, Block::Text { text: "hello".into() }]);
        assert_eq!(resp.stop, StopReason::MaxTokens);
        gw.complete(&req, &mut |_| {}).unwrap();
        let sent = server.requests();
        assert_eq!(sent.len(), 3);
        assert!(sent[0].json().get("stream_options").is_some());
        // Retried without it, and remembered for the next call.
        for later in &sent[1..] {
            let b = later.json();
            assert!(b.get("stream_options").is_none() && b.get("reasoning_effort").is_none());
            assert_eq!((b["max_tokens"].clone(), b["keep_alive"].clone()), (json!(64), json!("5m")));
            assert!(later.header("authorization").is_none());
        }
    }

    #[test]
    fn meta_preset_shapes_the_request_and_flags_its_contributor_models() {
        let stream = sse_data(&[r#"{"model":"muse-spark-1.3","choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#, "[DONE]"]);
        let server = FakeServer::start(vec![Reply::sse(stream)]);
        let preset = crate::presets().into_iter().find(|p| p.id == "meta").unwrap();
        let p = ProviderConfig { base_url: Some(server.url()), ..preset.config() };
        let gw = gateway(vec![p]);
        let req = ChatRequest { model: "meta/muse-spark-1.3@high".into(), max_tokens: 2000, effort: Some("low".into()), ..request("hi") };
        assert_eq!(gw.complete(&req, &mut |_| {}).unwrap().model, "meta/muse-spark-1.3");
        let sent = &server.requests()[0];
        assert_eq!(sent.header("authorization"), Some("Bearer env-MODEL_API_KEY"));
        let b = sent.json();
        assert_eq!((b["model"].clone(), b["reasoning_effort"].clone(), b["max_tokens"].clone()), (json!("muse-spark-1.3"), json!("high"), json!(2000)));
        assert!(crate::trains_on_data("meta/muse-spark-1.3-contributor@low"));
        assert!(!crate::trains_on_data("meta/muse-spark-1.3"));
        assert!(!crate::trains_on_data("deepseek/muse-spark-1.3-contributor"));
    }

    #[test]
    fn xai_preset_sends_the_pinned_reasoning_effort() {
        let stream = sse_data(&[r#"{"model":"grok-4.7","choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#, "[DONE]"]);
        let server = FakeServer::start(vec![Reply::sse(stream)]);
        let preset = crate::presets().into_iter().find(|p| p.id == "xai").unwrap();
        assert_eq!(preset.base_url, Some("https://api.x.ai/v1"));
        let p = ProviderConfig { base_url: Some(server.url()), ..preset.config() };
        let gw = gateway(vec![p]);
        let req = ChatRequest { model: "xai/grok-4.7@low".into(), max_tokens: 2000, effort: Some("high".into()), ..request("hi") };
        assert_eq!(gw.complete(&req, &mut |_| {}).unwrap().model, "xai/grok-4.7");
        let sent = &server.requests()[0];
        assert_eq!(sent.header("authorization"), Some("Bearer env-XAI_API_KEY"));
        let b = sent.json();
        assert_eq!((b["model"].clone(), b["reasoning_effort"].clone(), b["max_tokens"].clone()), (json!("grok-4.7"), json!("low"), json!(2000)));
    }

    #[test]
    fn deepseek_reasoning_is_replayed_only_when_the_provider_asks() {
        let done = sse_data(&[r#"{"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}"#, "[DONE]"]);
        let server = FakeServer::start(vec![Reply::sse(sse_data(STREAM)), Reply::sse(done.clone()), Reply::sse(done)]);
        let mut ds = provider("ds", ProviderKind::OpenAiCompatible, &server, Auth::None);
        ds.options = json!({"replay_reasoning": true});
        let gw = gateway(vec![ds, provider("plain", ProviderKind::OpenAiCompatible, &server, Auth::None)]);
        let first = gw.complete(&ChatRequest { model: "ds/deepseek-flash".into(), ..conversation() }, &mut |_| {}).unwrap();
        assert_eq!(first.content[0], opaque("think"));
        // The next turn, with tools in use, sent to both providers.
        let turn = |model: &str| ChatRequest {
            model: model.into(),
            messages: vec![Message::user_text("hi"), Message::assistant(first.content.clone()), Message::user_text("go on")],
            tools: conversation().tools,
            ..request("")
        };
        gw.complete(&turn("ds/deepseek-flash"), &mut |_| {}).unwrap();
        gw.complete(&turn("plain/deepseek-flash"), &mut |_| {}).unwrap();
        let sent = server.requests();
        let replayed = &sent[1].json()["messages"][1];
        assert_eq!((replayed["reasoning_content"].clone(), replayed["content"].clone()), (json!("think"), json!("Let me look.")));
        assert!(sent[2].json()["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn mistral_tool_call_ids_are_mapped_consistently_and_valid_ones_kept() {
        let done = sse_data(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#, "[DONE]"]);
        let server = FakeServer::start(vec![Reply::sse(done.clone()), Reply::sse(done)]);
        let mut mi = provider("mi", ProviderKind::OpenAiCompatible, &server, Auth::None);
        mi.options = json!({"mistral_compat": true});
        let gw = gateway(vec![mi, provider("plain", ProviderKind::OpenAiCompatible, &server, Auth::None)]);
        let turn = |model: &str| ChatRequest {
            model: model.into(),
            messages: vec![
                Message::user_text("hi"),
                Message::assistant(vec![
                    Block::ToolUse { id: "call_a1".into(), name: "bash".into(), input: json!({}), meta: None },
                    Block::ToolUse { id: "Xy12Z9qLm".into(), name: "list".into(), input: json!({}), meta: None },
                ]),
                Message {
                    role: Role::User,
                    content: vec![
                        Block::ToolResult { tool_use_id: "call_a1".into(), content: "a".into(), is_error: false },
                        Block::ToolResult { tool_use_id: "Xy12Z9qLm".into(), content: "b".into(), is_error: false },
                    ],
                },
            ],
            ..request("")
        };
        gw.complete(&turn("mi/mistral-large"), &mut |_| {}).unwrap();
        gw.complete(&turn("plain/mistral-large"), &mut |_| {}).unwrap();
        let sent = server.requests();
        let mapped = compat_id("call_a1");
        assert!(mapped.len() == 9 && mapped.bytes().all(|b| b.is_ascii_alphanumeric()));
        let b = sent[0].json()["messages"].clone();
        assert_eq!(b[1]["tool_calls"][0]["id"], json!(mapped));
        assert_eq!(b[1]["tool_calls"][1]["id"], json!("Xy12Z9qLm"));
        assert_eq!((b[2]["tool_call_id"].clone(), b[3]["tool_call_id"].clone()), (json!(mapped), json!("Xy12Z9qLm")));
        // Without the flag, the ids go out as they are.
        assert_eq!(sent[1].json()["messages"][1]["tool_calls"][0]["id"], json!("call_a1"));
    }

    #[test]
    fn mistral_compat_sends_max_tokens() {
        let done = sse_data(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#, "[DONE]"]);
        let server = FakeServer::start(vec![Reply::sse(done)]);
        let mut mi = provider("mi", ProviderKind::OpenAi, &server, Auth::None);
        mi.options = json!({"mistral_compat": true});
        let gw = gateway(vec![mi]);
        gw.complete(&ChatRequest { model: "mi/mistral-large".into(), max_tokens: 300, ..request("hi") }, &mut |_| {}).unwrap();
        let b = server.requests()[0].json();
        assert_eq!((b["max_tokens"].clone(), b.get("max_completion_tokens").is_none()), (json!(300), true));
    }

    #[test]
    fn mistral_content_chunks_decode_to_reasoning_and_text() {
        let stream = sse_data(&[
            r#"{"choices":[{"delta":{"content":[{"type":"thinking","thinking":[{"type":"text","text":"Weigh "},{"type":"text","text":"it."}]}]}}]}"#,
            r#"{"choices":[{"delta":{"content":[{"type":"text","text":"Answer."}]},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        let server = FakeServer::start(vec![Reply::sse(stream)]);
        let gw = gateway(vec![provider("mi", ProviderKind::OpenAiCompatible, &server, Auth::None)]);
        let mut deltas = Vec::new();
        let resp = gw.complete(&ChatRequest { model: "mi/magistral".into(), ..request("hi") }, &mut |d| deltas.push(d)).unwrap();
        assert_eq!(resp.content, vec![Block::Thinking { text: "Weigh it.".into() }, Block::Text { text: "Answer.".into() }]);
        assert_eq!(deltas, vec![Delta::Thinking { text: "Weigh it.".into() }, Delta::Text { text: "Answer.".into() }]);
    }

    #[test]
    fn base_url_joins_without_double_slashes() {
        let stream = sse_data(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#, "[DONE]"]);
        let models = Reply::json(200, &json!({"data": [{"id": "m1"}]}));
        let server = FakeServer::start(vec![Reply::sse(stream), models]);
        let mut p = provider("ds", ProviderKind::OpenAiCompatible, &server, Auth::None);
        p.base_url = Some(format!("{}/", server.url()));
        let gw = gateway(vec![p]);
        gw.complete(&ChatRequest { model: "ds/deepseek-flash".into(), ..request("hi") }, &mut |_| {}).unwrap();
        assert_eq!(gw.list_models("ds").unwrap()[0].id, "m1");
        let sent = server.requests();
        assert_eq!((sent[0].path.as_str(), sent[1].path.as_str()), ("/chat/completions", "/models"));
        // The DeepSeek preset's own root joins the same way.
        let root = crate::presets().into_iter().find(|p| p.id == "deepseek").unwrap().config().base().unwrap().to_string();
        assert_eq!(format!("{root}/chat/completions"), "https://api.deepseek.com/chat/completions");
        assert_eq!(format!("{root}/models"), "https://api.deepseek.com/models");
    }

    /// Live: `cargo test -p apz-gateway -- --ignored live_ollama` with Ollama
    /// serving `qwen3:8b` on its default port.
    #[test]
    #[ignore]
    fn live_ollama_tool_call() {
        let p = crate::presets().into_iter().find(|p| p.id == "ollama").unwrap().config();
        let gw = crate::Gateway::new(vec![p], |_: &str, _: &crate::SecretRead<'_>| None);
        let req = ChatRequest {
            model: "ollama/qwen3:8b".into(),
            tools: vec![ToolSpec {
                name: "bash".into(),
                description: "Run a shell command".into(),
                input_schema: json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
            }],
            ..request("List the files in /workspace using the bash tool.")
        };
        let mut deltas = 0;
        let resp = gw.complete(&req, &mut |_| deltas += 1).unwrap();
        eprintln!("{deltas} deltas: {resp:#?}");
        assert_eq!(resp.stop, StopReason::ToolUse);
        assert!(resp.usage.input_tokens > 0);
    }

    #[test]
    fn azure_uses_the_v1_path_and_api_key_header() {
        let stream = sse_data(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#, "[DONE]"]);
        let server = FakeServer::start(vec![Reply::sse(stream)]);
        let gw = gateway(vec![provider("az", ProviderKind::AzureOpenAi, &server, Auth::Secret("s".into()))]);
        let resp = gw.complete(&ChatRequest { model: "az/my-deploy".into(), max_tokens: 9, ..request("hi") }, &mut |_| {}).unwrap();
        assert_eq!((resp.text(), resp.model.as_str(), resp.stop), ("ok".to_string(), "az/my-deploy", StopReason::EndTurn));
        let sent = &server.requests()[0];
        assert_eq!(sent.path, "/openai/v1/chat/completions");
        assert_eq!(sent.header("api-key"), Some("sk-secret-value"));
        assert_eq!(sent.json()["model"], "my-deploy");
        assert_eq!(sent.json()["max_tokens"], 9);
    }
}
