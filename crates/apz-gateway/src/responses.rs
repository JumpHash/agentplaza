//! The OpenAI Responses API (`openai-responses`), stateless: `store: false`
//! with encrypted reasoning items replayed verbatim. Streamed over the
//! Responses WebSocket mode (`options.websocket`, on by default): one socket
//! per (origin, credential) is pooled, and replaced before the server's
//! 60-minute limit. A call that no socket can carry goes over HTTP instead,
//! not streamed: the final text is emitted as one delta.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Socket};
use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::http::Req;
use crate::openai::{authorize, base};
use crate::{Ctx, GatewayError, own_meta, parse_tool_input, replayable};

const TAG: &str = "openai-responses";
/// A socket is replaced once it is this old; the server closes at 60 minutes.
const MAX_AGE: Duration = Duration::from_secs(55 * 60);
/// The most sockets open at once for one (origin, credential).
const MAX_PER_KEY: usize = 4;
/// How long one read waits before the call looks at its deadline again.
const POLL: Duration = Duration::from_millis(250);
/// The longest a call may wait for its answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Tells the calls on a socket apart (`stream_id`).
static NEXT_STREAM: AtomicU64 = AtomicU64::new(1);

pub(crate) fn complete(
    ctx: &Ctx,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let root = base(ctx)?;
    let url = format!("{root}/responses");
    let body = ctx.finish_body(body(req, model));
    if websocket(ctx)
        && let Some(route) = socket_route(ctx, &root)?
        && let Some(result) = over_socket(&route, &body, sink)
    {
        return result;
    }
    let answer = ctx.http().json(&authorize(ctx, Req::post_json(url, &body))?)?;
    decode(&answer, sink)
}

/// Whether this provider's calls use the WebSocket mode.
fn websocket(ctx: &Ctx) -> bool {
    ctx.p.options.get("websocket").and_then(Value::as_bool).unwrap_or(true)
}

/// Where a socket to this provider opens: the origin, the path of `/responses`
/// under it, the bearer and headers, and the pool key.
struct Route {
    target: Target,
    path: String,
    key: PoolKey,
}

/// `None` when the base is not http(s).
fn socket_route(ctx: &Ctx, root: &str) -> Result<Option<Route>, GatewayError> {
    let scheme = if root.starts_with("https://") {
        "https://".len()
    } else if root.starts_with("http://") {
        "http://".len()
    } else {
        return Ok(None);
    };
    let (origin, path) = match root[scheme..].find('/') {
        Some(at) => root.split_at(scheme + at),
        None => (root, ""),
    };
    let bearer = ctx.credential()?;
    let headers: Vec<(String, String)> =
        ctx.p.extra_headers.iter().map(|(name, value)| (name.clone(), value.clone())).collect();
    let mut identity = Sha256::new();
    identity.update(bearer.as_deref().unwrap_or_default());
    for (name, value) in &headers {
        identity.update(format!("\n{name}: {value}"));
    }
    Ok(Some(Route {
        target: Target { base: origin.to_string(), bearer, headers, ..Target::default() },
        path: format!("{}/responses", path.trim_end_matches('/')),
        key: (origin.to_string(), identity.finalize().into()),
    }))
}

/// The pool is keyed by origin and a hash of the credential and headers.
type PoolKey = (String, [u8; 32]);

/// The sockets of one key: the idle ones, each with the instant it opened,
/// and how many are open in all (idle or in use).
#[derive(Default)]
struct Slot {
    idle: Vec<(Socket, Instant)>,
    open: usize,
}

static POOL: OnceLock<Mutex<HashMap<PoolKey, Slot>>> = OnceLock::new();

fn pool() -> MutexGuard<'static, HashMap<PoolKey, Slot>> {
    POOL.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

enum Lease {
    /// An idle socket and the instant it opened.
    Idle(Socket, Instant),
    /// Nothing idle, but a slot is reserved: open a new socket.
    Fresh,
    /// The key is at its cap: this call goes over HTTP.
    Full,
}

fn lease(key: &PoolKey) -> Lease {
    let mut map = pool();
    let slot = map.entry(key.clone()).or_default();
    while let Some((socket, opened)) = slot.idle.pop() {
        if opened.elapsed() < MAX_AGE {
            return Lease::Idle(socket, opened);
        }
        slot.open -= 1;
    }
    if slot.open >= MAX_PER_KEY {
        return Lease::Full;
    }
    slot.open += 1;
    Lease::Fresh
}

/// Gives a lease back: a healthy, young socket returns to the idle list; `None`
/// (or a socket too old to reuse) is dropped and its slot freed.
fn give_back(key: &PoolKey, lent: Option<(Socket, Instant)>) {
    let mut map = pool();
    let slot = map.entry(key.clone()).or_default();
    match lent {
        Some((socket, opened)) if opened.elapsed() < MAX_AGE => slot.idle.push((socket, opened)),
        _ => slot.open = slot.open.saturating_sub(1),
    }
}

/// One call over a pooled socket. `None`: this call goes over HTTP.
fn over_socket(route: &Route, body: &Value, sink: &mut dyn FnMut(Delta)) -> Option<Result<ChatResponse, GatewayError>> {
    let (mut socket, opened) = match lease(&route.key) {
        Lease::Idle(socket, opened) => (socket, opened),
        Lease::Full => return None,
        Lease::Fresh => match ws::connect(&route.target, &route.path, None, CONNECT_TIMEOUT, limits()) {
            Ok(socket) => (socket, Instant::now()),
            Err(_) => {
                give_back(&route.key, None);
                return None;
            }
        },
    };
    match exchange(&mut socket, body, sink) {
        Exchange::Fallback => {
            give_back(&route.key, None);
            None
        }
        Exchange::Finished(result) => {
            // A socket that reported an error is replaced, not reused.
            let healthy = result.is_ok().then_some((socket, opened));
            give_back(&route.key, healthy);
            Some(result)
        }
    }
}

fn limits() -> Limits {
    Limits { max_frame: 8 << 20, max_message: 8 << 20, ..Limits::default() }
}

enum Exchange {
    /// The call ended: its answer, or the error the server reported.
    Finished(Result<ChatResponse, GatewayError>),
    /// The socket failed before any event: the call goes over HTTP.
    Fallback,
}

/// A socket that failed: a fallback when no event has come yet, an error once
/// the call has begun.
fn broken(seen: bool) -> Exchange {
    if seen {
        Exchange::Finished(Err(GatewayError::Provider {
            message: "the WebSocket broke before the response completed".into(),
            retryable: true,
        }))
    } else {
        Exchange::Fallback
    }
}

/// Sends one `response.create` and reads its events until the answer ends.
/// Text and reasoning deltas reach `sink` as they arrive.
fn exchange(socket: &mut Socket, body: &Value, sink: &mut dyn FnMut(Delta)) -> Exchange {
    let stream_id = format!("apz-{}", NEXT_STREAM.fetch_add(1, Ordering::Relaxed));
    let mut event = body.clone();
    event["type"] = json!("response.create");
    event["stream_id"] = json!(stream_id);
    if socket.send(&ws::Message::Text(event.to_string())).is_err() {
        return Exchange::Fallback;
    }
    let deadline = Instant::now() + CALL_TIMEOUT;
    let mut seen = false;
    let (mut streamed_text, mut streamed_thinking) = (false, false);
    loop {
        let message = match socket.recv(POLL) {
            Ok(Some(message)) => message,
            Ok(None) if Instant::now() < deadline => continue,
            Ok(None) => {
                return Exchange::Finished(Err(GatewayError::Provider {
                    message: "no answer within 10 minutes".into(),
                    retryable: true,
                }));
            }
            Err(_) => return broken(seen),
        };
        let ws::Message::Text(raw) = message else {
            return broken(seen);
        };
        let Ok(event) = serde_json::from_str::<Value>(&raw) else {
            return broken(seen);
        };
        if event.get("stream_id").is_some_and(|id| id.as_str() != Some(stream_id.as_str())) {
            continue;
        }
        seen = true;
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    streamed_text = true;
                    sink(Delta::Text { text: delta.to_string() });
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    streamed_thinking = true;
                    sink(Delta::Thinking { text: delta.to_string() });
                }
            }
            "response.completed" | "response.incomplete" => {
                // The text and reasoning summaries went out as they streamed; the
                // final object contributes what was not streamed, and the tool calls.
                let mut rest = |delta: Delta| match delta {
                    Delta::Text { .. } if streamed_text => {}
                    Delta::Thinking { .. } if streamed_thinking => {}
                    other => sink(other),
                };
                return Exchange::Finished(decode(&event["response"], &mut rest));
            }
            "response.failed" => {
                let message = event["response"]["error"]["message"].as_str().unwrap_or("the response failed");
                return Exchange::Finished(Err(GatewayError::Provider { message: message.to_string(), retryable: false }));
            }
            "error" => {
                let message = event["error"]["message"]
                    .as_str()
                    .or_else(|| event["message"].as_str())
                    .unwrap_or("the stream reported an error");
                return Exchange::Finished(Err(GatewayError::Provider { message: message.to_string(), retryable: false }));
            }
            _ => {}
        }
    }
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
    use crate::config::{Auth, ProviderConfig, ProviderKind};
    use crate::testutil::{FakeServer, Reply, gateway, provider, request};
    use apz_http::ws::Origins;
    use apz_http::{Handle, Request, Response, Server};
    use apz_proto::chat::ToolSpec;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

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
        let mut p = provider("r", ProviderKind::OpenAiResponses, &server, Auth::Env("OPENAI_API_KEY".into()));
        p.options = json!({"websocket": false});
        let gw = gateway(vec![p]);
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

    /// The events of one call, echoing its `stream_id`: a reasoning summary, two
    /// text deltas, then the completed response.
    fn events(id: &Value) -> Vec<Value> {
        let response = json!({
            "model": "gpt-5-2025", "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "plan"}], "encrypted_content": "ENC"},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hello"}]},
            ],
            "usage": {"input_tokens": 5, "output_tokens": 2},
        });
        vec![
            json!({"type": "response.created", "stream_id": id, "response": {"status": "in_progress"}}),
            json!({"type": "response.reasoning_summary_text.delta", "stream_id": id, "delta": "plan"}),
            json!({"type": "response.output_text.delta", "stream_id": id, "delta": "Hel"}),
            json!({"type": "response.output_text.delta", "stream_id": id, "delta": "lo"}),
            json!({"type": "response.completed", "stream_id": id, "response": response}),
        ]
    }

    /// Port 0, `/v1/responses`: a WebSocket that answers every `response.create`
    /// with [`events`] (or closes at once, when `close_at_once`), and a POST
    /// answered with `answer`. Counts the sockets opened and the POSTs.
    fn server(answer: Value, close_at_once: bool, accepts: Arc<AtomicUsize>, posts: Arc<AtomicUsize>) -> Handle {
        Server::bind(0).expect("bind").spawn(move |request: &Request| {
            if request.path != "/v1/responses" {
                return Response::error(404, "no such path");
            }
            if request.method == "GET" {
                accepts.fetch_add(1, Ordering::SeqCst);
                return ws::accept(request, Origins::None, None, ws::Limits::default(), move |mut socket: Socket| {
                    if close_at_once {
                        socket.close(1001, "going away");
                        return;
                    }
                    loop {
                        match socket.recv(Duration::from_secs(5)) {
                            Ok(Some(ws::Message::Text(raw))) => {
                                let create: Value = serde_json::from_str(&raw).expect("JSON");
                                for event in events(&create["stream_id"]) {
                                    socket.send(&ws::Message::Text(event.to_string())).expect("send");
                                }
                            }
                            Ok(_) => {}
                            Err(_) => return,
                        }
                    }
                });
            }
            posts.fetch_add(1, Ordering::SeqCst);
            Response::json(200, &answer)
        })
        .expect("spawn")
    }

    fn at(port: u16) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(format!("http://127.0.0.1:{port}/v1")),
            auth: Auth::None,
            ..ProviderConfig::new("r", ProviderKind::OpenAiResponses)
        }
    }

    fn call(gw: &crate::Gateway, deltas: &mut Vec<Delta>) -> ChatResponse {
        let req = ChatRequest { model: "r/gpt-5.6-terra".into(), ..request("hi") };
        gw.complete(&req, &mut |d| deltas.push(d)).unwrap()
    }

    #[test]
    fn websocket_streams_the_deltas_and_decodes_the_completed_response() {
        let (accepts, posts) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let server = server(json!({}), false, accepts.clone(), posts.clone());
        let gw = gateway(vec![at(server.port())]);
        let mut deltas = Vec::new();
        let resp = call(&gw, &mut deltas);
        assert_eq!(deltas, vec![
            Delta::Thinking { text: "plan".into() },
            Delta::Text { text: "Hel".into() },
            Delta::Text { text: "lo".into() },
        ], "streamed in order, the completed object adds nothing already sent");
        assert!(resp.content.contains(&Block::Text { text: "Hello".into() }));
        assert_eq!((resp.model.as_str(), resp.stop.clone(), resp.usage.output_tokens), ("r/gpt-5-2025", StopReason::EndTurn, 2));
        assert_eq!((accepts.load(Ordering::SeqCst), posts.load(Ordering::SeqCst)), (1, 0));
    }

    #[test]
    fn sequential_calls_reuse_one_socket() {
        let (accepts, posts) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let server = server(json!({}), false, accepts.clone(), posts.clone());
        let gw = gateway(vec![at(server.port())]);
        for _ in 0..2 {
            let mut deltas = Vec::new();
            assert!(call(&gw, &mut deltas).content.contains(&Block::Text { text: "Hello".into() }));
        }
        assert_eq!(accepts.load(Ordering::SeqCst), 1, "both calls rode one socket");
        assert_eq!(posts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_socket_closed_before_any_event_falls_back_to_http() {
        let (accepts, posts) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let answer = json!({
            "model": "gpt-5-2025", "status": "completed",
            "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "over http"}]}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        });
        let server = server(answer, true, accepts.clone(), posts.clone());
        let gw = gateway(vec![at(server.port())]);
        let mut deltas = Vec::new();
        let resp = call(&gw, &mut deltas);
        assert_eq!(deltas, vec![Delta::Text { text: "over http".into() }]);
        assert!(resp.content.contains(&Block::Text { text: "over http".into() }));
        assert_eq!((accepts.load(Ordering::SeqCst), posts.load(Ordering::SeqCst)), (1, 1));
    }
}
