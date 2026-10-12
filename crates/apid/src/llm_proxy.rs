//! The model gateway for coding engines inside agents' VMs. Claude Code, Codex and
//! OpenCode point their base URLs at `plaza.gateway`; the hub relays each request to
//! `/internal/v1/agents/{cid}/gateway/...`. apid holds the real provider keys, applies
//! the owner's model policy, streams the answer back and meters what it can.
//!
//! Two surfaces are WebSockets, for clients that keep one connection open: the OpenAI
//! Responses socket relays frames to the provider unchanged, and the Anthropic bridge carries
//! each Messages stream as frames on the socket. Both run the same policy, keys and metering
//! as the HTTP paths.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Message, Origins, Socket};
use apz_http::{Request, Response, Router};
use serde_json::{json, Value};

use crate::agents::Agent;
use crate::internal::authorized;
use crate::settings::{tier_of, Tier};
use crate::state::App;

/// How long one wait on a socket blocks: a relay reads its two sockets in turns.
const TICK: Duration = Duration::from_millis(20);
/// A socket lives this long at most; the client reconnects (close 1001).
const LIFETIME: Duration = Duration::from_secs(55 * 60);
/// Calls a client may queue on one Messages socket, behind the one running.
const MAX_QUEUED: usize = 4;
/// The subprotocol the Anthropic bridge requires.
const MESSAGES_PROTOCOL: &str = "apz.anthropic.v1";

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/internal/v1/agents/{cid}/gateway/anthropic/v1/messages", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| relay(app, req, cid, "anthropic", "/v1/messages"))
        })
        .get("/internal/v1/agents/{cid}/gateway/anthropic/v1/messages", |app, req, p| {
            if !is_upgrade(req) {
                return not_upgrade();
            }
            authorized(app, req, p.get("cid"), |cid| messages_socket(app, req, cid))
        })
        .get("/internal/v1/agents/{cid}/gateway/anthropic/v1/models", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| list_models(app, cid, "anthropic"))
        })
        .post("/internal/v1/agents/{cid}/gateway/openai/v1/responses", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| relay(app, req, cid, "openai", "/v1/responses"))
        })
        .get("/internal/v1/agents/{cid}/gateway/openai/v1/responses", |app, req, p| {
            if !is_upgrade(req) {
                return not_upgrade();
            }
            authorized(app, req, p.get("cid"), |cid| responses_socket(app, req, cid))
        })
        .post("/internal/v1/agents/{cid}/gateway/openai/v1/chat/completions", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| relay(app, req, cid, "openai", "/v1/chat/completions"))
        })
        .get("/internal/v1/agents/{cid}/gateway/openai/v1/models", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| list_models(app, cid, "openai"))
        })
}

/// A GET without an upgrade to a socket path: the 405 the router gives a path that takes only POST.
fn not_upgrade() -> Response {
    Response::error(405, "this endpoint accepts POST").with_header("Allow", "POST")
}

fn is_upgrade(req: &Request) -> bool {
    req.header("upgrade").is_some()
}

/// What the gateway does with the `model` a request names.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// On the agent's tier list: forwarded as asked.
    Allow,
    /// Not on the list, but the agent's own model is on the same provider: forwarded with this model.
    Rewrite(String),
    /// Not on the list and no same-provider fallback: refused, naming the allowed models.
    Refuse(String),
}

/// A model name without its effort suffix: `anthropic/claude-opus-5-5@low` is `anthropic/claude-opus-5-5`.
fn canonical(model: &str) -> &str {
    model.split('@').next().unwrap_or(model)
}

/// The policy decision for a request to `provider` naming `requested`, from an agent whose
/// own model is `own` and whose tier list is `allowed`.
pub fn decide_model(provider: &str, requested: &str, own: &str, allowed: &[String]) -> Decision {
    let candidate = format!("{provider}/{requested}");
    if allowed.iter().any(|m| canonical(m) == candidate.as_str()) {
        return Decision::Allow;
    }
    if let Some((own_provider, own_model)) = canonical(own).split_once('/') {
        if own_provider == provider {
            return Decision::Rewrite(own_model.to_string());
        }
    }
    let names: Vec<&str> = allowed.iter().map(|m| canonical(m)).collect();
    Decision::Refuse(format!("`{requested}` is not allowed for this agent; allowed: {}", names.join(", ")))
}

/// Applies the policy to a request body: an allowed model passes, an unlisted one is rewritten
/// to the agent's own model when it is on the same provider, and refused otherwise.
fn check_model(body: &mut Value, provider: &str, own: &str, list: &[String]) -> Result<(), String> {
    let requested = body.get("model").and_then(Value::as_str).unwrap_or_default();
    match decide_model(provider, requested, own, list) {
        Decision::Allow => Ok(()),
        Decision::Rewrite(model) => {
            body["model"] = json!(model);
            Ok(())
        }
        Decision::Refuse(message) => Err(message),
    }
}

/// The tier list an agent's model calls are held to: `None` when they are not held to one
/// (dev models, or a worker).
fn held_to(app: &App, agent: &Agent) -> Option<Vec<String>> {
    if app.settings.dev_models() || tier_of(&agent.role) != Tier::Orchestrator {
        return None;
    }
    Some(app.settings.get().model_policy.orchestrator)
}

/// The upstream URL for `path` under `base`. A base that already ends in `/v1` takes the
/// path without its own `/v1`.
pub fn upstream_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = if base.ends_with("/v1") { path.strip_prefix("/v1").unwrap_or(path) } else { path };
    format!("{base}{path}")
}

/// The host part of a provider base, for a WebSocket target (which takes no path).
fn ws_base(base: &str) -> String {
    let base = base.trim_end_matches('/');
    base.strip_suffix("/v1").unwrap_or(base).to_string()
}

/// Input and output tokens of one response or stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
}

type Counts = (Option<u64>, Option<u64>);

/// Folds the usage objects of one JSON event (top level, `message` or `response`) into `acc`;
/// a later count replaces an earlier one.
fn merge(acc: &mut Counts, event: &Value) {
    for usage in [event.get("usage"), event.pointer("/message/usage"), event.pointer("/response/usage")]
        .into_iter()
        .flatten()
    {
        let input = usage.get("input_tokens").or_else(|| usage.get("prompt_tokens")).and_then(Value::as_u64);
        let output = usage.get("output_tokens").or_else(|| usage.get("completion_tokens")).and_then(Value::as_u64);
        acc.0 = input.or(acc.0);
        acc.1 = output.or(acc.1);
    }
}

fn finish(acc: Counts) -> Option<Tokens> {
    if acc.0.is_none() && acc.1.is_none() {
        return None;
    }
    Some(Tokens { input: acc.0.unwrap_or(0), output: acc.1.unwrap_or(0) })
}

/// The usage of a buffered Anthropic or OpenAI response body.
pub fn tokens_in(body: &Value) -> Option<Tokens> {
    let mut acc = (None, None);
    merge(&mut acc, body);
    finish(acc)
}

/// The usage of a whole stream from its SSE text. Anthropic's `message_start` gives the
/// input and its `message_delta` the final output, so the last count of each wins.
pub fn tokens_in_sse(text: &str) -> Option<Tokens> {
    let mut acc = (None, None);
    for data in text.lines().filter_map(|line| line.strip_prefix("data:")) {
        if let Ok(event) = serde_json::from_str::<Value>(data.trim()) {
            merge(&mut acc, &event);
        }
    }
    finish(acc)
}

/// Usage read from streams and sockets, waiting to be recorded: the stream writer has no
/// `App`, so the scheduler's loop drains this every couple of seconds ([`drain_usage`]).
fn pending_usage() -> &'static std::sync::Mutex<Vec<(u64, String, Tokens)>> {
    static PENDING: std::sync::OnceLock<std::sync::Mutex<Vec<(u64, String, Tokens)>>> = std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

fn defer_usage(cid: u64, model: String, tokens: Tokens) {
    pending_usage().lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((cid, model, tokens));
}

/// Records the usage of streams that have ended.
pub fn drain_usage(app: &App) {
    let done: Vec<_> = std::mem::take(&mut *pending_usage().lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for (cid, model, tokens) in done {
        record_usage(app, cid, &model, tokens);
    }
}

/// Adds one call's tokens to the agent's usage and records it in the transcript.
fn record_usage(app: &App, cid: u64, model: &str, tokens: Tokens) {
    let usage = serde_json::from_value(json!({ "input_tokens": tokens.input, "output_tokens": tokens.output }))
        .unwrap_or_default();
    app.agents.update(cid, |a| a.usage.add(&usage));
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "model_response",
        json!({
            "via": "gateway",
            "model": model,
            "usage": { "input_tokens": tokens.input, "output_tokens": tokens.output },
        }),
    );
}

/// An upstream client that returns error statuses as responses, so their bodies pass through.
fn http_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::config::Config::builder().http_status_as_error(false).build())
}

/// The Anthropic headers a client sends that the upstream must get too.
fn anthropic_headers(req: &Request) -> Vec<(String, String)> {
    ["anthropic-version", "anthropic-beta"]
        .into_iter()
        .filter_map(|name| req.header(name).map(|value| (name.to_string(), value.to_string())))
        .collect()
}

/// The provider's upstream base and key, as the HTTP path takes them.
fn provider_upstream(app: &App, provider: &str) -> Result<(String, String), Response> {
    let (base, key) = app
        .providers
        .upstream(provider)
        .ok_or_else(|| Response::error(503, &format!("no `{provider}` provider is configured")))?;
    let key = key.ok_or_else(|| Response::error(503, &format!("the `{provider}` provider has no key")))?;
    Ok((base, key))
}

/// One POST to the provider's `path`, with its key in the provider's header shape.
fn upstream_post(
    provider: &str,
    base: &str,
    key: &str,
    path: &str,
    headers: &[(String, String)],
    body: &Value,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let client = http_agent();
    let url = upstream_url(base, path);
    let mut call = client.post(url.as_str());
    if provider == "anthropic" {
        call = call.header("x-api-key", key);
        for (name, value) in headers {
            call = call.header(name.as_str(), value.as_str());
        }
    } else {
        call = call.header("authorization", format!("Bearer {key}"));
    }
    call.send_json(body).map_err(|e| format!("upstream: {e}"))
}

/// Relays one model call for agent `cid` to `provider`'s `path`, streaming when asked.
fn relay(app: &App, req: &Request, cid: u64, provider: &str, path: &str) -> Result<Response, Response> {
    let agent = app.agents.get(cid).ok_or_else(|| Response::error(404, "unknown agent"))?;
    let mut body: Value = req.json().map_err(|e| Response::error(400, &e.to_string()))?;
    // Workers may call any model; the orchestrator is held to its list.
    if let Some(list) = held_to(app, &agent) {
        let own = crate::agents::model_of(app, &agent);
        check_model(&mut body, provider, &own, &list)
            .map_err(|message| Response::json(403, &json!({ "error": message })))?;
    }
    let model = body.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
    let (base, key) = provider_upstream(app, provider)?;
    let headers = anthropic_headers(req);
    let resp = upstream_post(provider, &base, &key, path, &headers, &body).map_err(|e| Response::error(502, &e))?;

    let status = resp.status().as_u16();
    let ok = (200..300).contains(&status);
    let streaming = body.get("stream").and_then(Value::as_bool) == Some(true);
    let mut reader = resp.into_body().into_reader();
    if streaming && ok {
        return Ok(Response::sse(move |sse| {
            // The stream's tail carries its usage; it is metered once the stream ends.
            let tail = read_sse(BufReader::new(reader), |kind, data| sse.event(kind, data))?;
            if let Some(tokens) = tokens_in_sse(&tail) {
                defer_usage(cid, model, tokens);
            }
            Ok(())
        }));
    }

    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|e| Response::error(502, &format!("upstream: {e}")))?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| Response::error(502, "the upstream body is not JSON"))?;
    if ok {
        if let Some(tokens) = tokens_in(&value) {
            record_usage(app, cid, &model, tokens);
        }
    }
    Ok(Response::json(status, &value))
}

/// Reads an SSE body, handing each event (its name, `message` when unnamed, and its data) to
/// `on_event`. Returns the stream's tail, for metering.
fn read_sse(reader: impl BufRead, mut on_event: impl FnMut(&str, &str) -> io::Result<()>) -> io::Result<String> {
    let mut event = String::new();
    let mut data = String::new();
    let mut tail = String::new();
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.is_empty() {
            if !data.is_empty() {
                let kind = if event.is_empty() { "message" } else { event.as_str() };
                on_event(kind, &data)?;
            }
            event.clear();
            data.clear();
        } else if let Some(name) = line.strip_prefix("event:") {
            event = name.trim().to_string();
        } else if let Some(text) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(text.strip_prefix(' ').unwrap_or(text));
        }
        tail.push_str(&line);
        tail.push('\n');
        if tail.len() > 64 * 1024 {
            let mut cut = tail.len() - 32 * 1024;
            while !tail.is_char_boundary(cut) {
                cut += 1;
            }
            tail.drain(..cut);
        }
    }
    Ok(tail)
}

/// What a WebSocket's model calls need, read when the socket opens: the agent's own model, the
/// list it is held to (`None`: not held to one), and the provider's upstream.
struct Grant {
    cid: u64,
    own: String,
    list: Option<Vec<String>>,
    base: String,
    key: String,
    headers: Vec<(String, String)>,
}

fn grant_for(app: &App, req: &Request, cid: u64, provider: &str) -> Result<Grant, Response> {
    let agent = app.agents.get(cid).ok_or_else(|| Response::error(404, "unknown agent"))?;
    let (base, key) = provider_upstream(app, provider)?;
    Ok(Grant {
        cid,
        own: crate::agents::model_of(app, &agent),
        list: held_to(app, &agent),
        base,
        key,
        headers: anthropic_headers(req),
    })
}

/// The gateway's policy for one WebSocket call, as over HTTP.
fn admit(grant: &Grant, provider: &str, body: &mut Value) -> Result<(), String> {
    match &grant.list {
        Some(list) => check_model(body, provider, &grant.own, list),
        None => Ok(()),
    }
}

fn limits() -> ws::Limits {
    ws::Limits { max_frame: 8 << 20, max_message: 8 << 20, ..Default::default() }
}

fn text(value: Value) -> Message {
    Message::Text(value.to_string())
}

fn responses_socket(app: &App, req: &Request, cid: u64) -> Result<Response, Response> {
    let grant = grant_for(app, req, cid, "openai")?;
    Ok(ws::accept(req, Origins::None, None, limits(), move |socket| relay_responses(socket, grant)))
}

fn messages_socket(app: &App, req: &Request, cid: u64) -> Result<Response, Response> {
    let grant = grant_for(app, req, cid, "anthropic")?;
    Ok(ws::accept(req, Origins::None, Some(MESSAGES_PROTOCOL), limits(), move |socket| relay_messages(socket, grant)))
}

/// A client frame on the Responses socket: a JSON `response.create`, and nothing else.
fn check_create(frame: &str) -> Result<Value, &'static str> {
    let frame: Value = serde_json::from_str(frame).map_err(|_| "frames must be JSON")?;
    if frame.get("type").and_then(Value::as_str) != Some("response.create") {
        return Err("the only frame a client sends is response.create");
    }
    Ok(frame)
}

/// A client frame on the Messages bridge: `{"type":"messages.create","id","request"}`.
fn check_messages_create(frame: &str) -> Result<(Value, Value), &'static str> {
    let frame: Value = serde_json::from_str(frame).map_err(|_| "frames must be JSON")?;
    if frame.get("type").and_then(Value::as_str) != Some("messages.create") {
        return Err("the only frame a client sends is messages.create");
    }
    let request = match frame.get("request") {
        Some(request) if request.is_object() => request.clone(),
        _ => return Err("messages.create needs a request object"),
    };
    Ok((frame.get("id").cloned().unwrap_or(Value::Null), request))
}

/// One SSE event as a bridge frame: `{"id","event","data"}`; data that is not JSON goes as a string.
fn sse_frame(id: &Value, event: &str, data: &str) -> Value {
    let data = serde_json::from_str::<Value>(data).unwrap_or_else(|_| Value::String(data.to_string()));
    json!({ "id": id, "event": event, "data": data })
}

/// Meters a `response.completed` event's usage, as the HTTP path meters a response.
fn meter_responses(cid: u64, model: &str, frame: &str) {
    let Ok(event) = serde_json::from_str::<Value>(frame) else { return };
    if event.get("type").and_then(Value::as_str) != Some("response.completed") {
        return;
    }
    if let Some(tokens) = tokens_in(&event) {
        defer_usage(cid, model.to_string(), tokens);
    }
}

/// The OpenAI Responses socket: each `response.create` is checked and sent upstream; every
/// upstream frame goes back unchanged. One thread reads both sockets in turns.
fn relay_responses(mut client: Socket, grant: Grant) {
    let target = Target { base: ws_base(&grant.base), bearer: Some(grant.key.clone()), ..Default::default() };
    let mut upstream = match ws::connect(&target, "/v1/responses", None, Duration::from_secs(15), limits()) {
        Ok(socket) => socket,
        Err(_) => {
            client.close(1011, "the upstream is unavailable");
            return;
        }
    };
    let started = Instant::now();
    let mut model = String::new();
    loop {
        if started.elapsed() >= LIFETIME {
            client.close(1001, "reconnect");
            upstream.close(1001, "");
            return;
        }
        match client.recv(TICK) {
            Err(_) => {
                upstream.close(1000, "");
                return;
            }
            Ok(None) => {}
            Ok(Some(Message::Binary(_))) => {
                client.close(1008, "frames must be text");
                upstream.close(1000, "");
                return;
            }
            Ok(Some(Message::Text(frame))) => {
                let mut body = match check_create(&frame) {
                    Ok(body) => body,
                    Err(reason) => {
                        client.close(1008, reason);
                        upstream.close(1000, "");
                        return;
                    }
                };
                if let Err(message) = admit(&grant, "openai", &mut body) {
                    if client.send(&text(json!({ "type": "error", "error": { "message": message } }))).is_err() {
                        upstream.close(1000, "");
                        return;
                    }
                    continue;
                }
                model = body.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
                if upstream.send(&Message::Text(body.to_string())).is_err() {
                    client.close(1011, "the upstream connection failed");
                    return;
                }
            }
        }
        match upstream.recv(TICK) {
            Err(_) => {
                client.close(1011, "the upstream closed");
                return;
            }
            Ok(None) => {}
            Ok(Some(message)) => {
                if let Message::Text(frame) = &message {
                    meter_responses(grant.cid, &model, frame);
                }
                if client.send(&message).is_err() {
                    upstream.close(1000, "");
                    return;
                }
            }
        }
    }
}

/// The Anthropic bridge: each `messages.create` runs one Messages stream, its events sent as
/// frames. Calls run one at a time; up to [`MAX_QUEUED`] wait behind the one running.
fn relay_messages(mut client: Socket, grant: Grant) {
    let started = Instant::now();
    let mut queue: VecDeque<(Value, Value)> = VecDeque::new();
    loop {
        if started.elapsed() >= LIFETIME {
            client.close(1001, "reconnect");
            return;
        }
        // Waits for a frame only when idle; with calls queued, reads one frame per turn.
        let wait = if queue.is_empty() { TICK } else { Duration::from_millis(1) };
        match client.recv(wait) {
            Err(_) => return,
            Ok(None) => {}
            Ok(Some(Message::Binary(_))) => {
                client.close(1008, "frames must be text");
                return;
            }
            Ok(Some(Message::Text(frame))) => match check_messages_create(&frame) {
                Err(reason) => {
                    client.close(1008, reason);
                    return;
                }
                Ok((id, request)) if queue.len() < MAX_QUEUED => queue.push_back((id, request)),
                Ok((id, _)) => {
                    let refused = json!({
                        "id": id,
                        "event": "error",
                        "data": { "message": format!("too many calls queued; at most {MAX_QUEUED} wait") },
                    });
                    if client.send(&text(refused)).is_err() {
                        return;
                    }
                }
            },
        }
        if let Some((id, request)) = queue.pop_front() {
            if bridge_call(&mut client, &grant, &id, request).is_err() {
                return;
            }
        }
    }
}

/// One Messages call over the socket: the policy, key and upstream as over HTTP, every SSE
/// event sent as a frame, then `done`. A failure is one error frame. `Err` only when the client
/// is gone.
fn bridge_call(client: &mut Socket, grant: &Grant, id: &Value, mut request: Value) -> io::Result<()> {
    request["stream"] = json!(true);
    match stream_messages(client, grant, id, &mut request) {
        Ok(()) => client.send(&text(json!({ "id": id, "event": "done" }))),
        Err(message) => client.send(&text(json!({ "id": id, "event": "error", "data": { "message": message } }))),
    }
}

fn stream_messages(client: &mut Socket, grant: &Grant, id: &Value, request: &mut Value) -> Result<(), String> {
    admit(grant, "anthropic", request)?;
    let model = request.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
    let resp = upstream_post("anthropic", &grant.base, &grant.key, "/v1/messages", &grant.headers, request)?;
    let status = resp.status().as_u16();
    let mut reader = resp.into_body().into_reader();
    if !(200..300).contains(&status) {
        let mut body = String::new();
        reader.read_to_string(&mut body).map_err(|e| e.to_string())?;
        return Err(format!("upstream answered {status}: {body}"));
    }
    let tail = read_sse(BufReader::new(reader), |kind, data| client.send(&text_frame(id, kind, data)))
        .map_err(|e| e.to_string())?;
    if let Some(tokens) = tokens_in_sse(&tail) {
        defer_usage(grant.cid, model, tokens);
    }
    Ok(())
}

/// The bridge frame of one SSE event, as a socket message.
fn text_frame(id: &Value, event: &str, data: &str) -> Message {
    text(sse_frame(id, event, data))
}

/// The models agent `cid` may use on `provider`, in that provider's list shape.
fn list_models(app: &App, cid: u64, provider: &str) -> Result<Response, Response> {
    let agent = app.agents.get(cid).ok_or_else(|| Response::error(404, "unknown agent"))?;
    let policy = app.settings.get().model_policy;
    let list = match tier_of(&agent.role) {
        Tier::Orchestrator => policy.orchestrator,
        Tier::Worker => policy.worker,
    };
    let prefix = format!("{provider}/");
    let ids: Vec<&str> = list
        .iter()
        .map(|m| canonical(m))
        .filter_map(|m| m.strip_prefix(prefix.as_str()))
        .collect();
    let body = if provider == "anthropic" {
        let data: Vec<Value> = ids.iter().map(|id| json!({ "type": "model", "id": id, "display_name": id })).collect();
        json!({ "data": data, "has_more": false, "first_id": ids.first(), "last_id": ids.last() })
    } else {
        let data: Vec<Value> = ids.iter().map(|id| json!({ "id": id, "object": "model", "owned_by": provider })).collect();
        json!({ "object": "list", "data": data })
    };
    Ok(Response::json(200, &body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(models: &[&str]) -> Vec<String> {
        models.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn a_listed_model_passes_untouched_and_effort_suffixes_do_not_matter() {
        let list = allowed(&["anthropic/claude-haiku-5-5", "anthropic/claude-opus-5-5@low"]);
        assert_eq!(decide_model("anthropic", "claude-haiku-5-5", "anthropic/claude-haiku-5-5", &list), Decision::Allow);
        assert_eq!(decide_model("anthropic", "claude-opus-5-5", "anthropic/claude-haiku-5-5", &list), Decision::Allow);
    }

    #[test]
    fn an_unlisted_model_is_rewritten_to_the_agents_own_on_the_same_provider() {
        let list = allowed(&["anthropic/claude-haiku-5-5", "openai/gpt-6-luna"]);
        assert_eq!(
            decide_model("anthropic", "claude-fable-5-1", "anthropic/claude-haiku-5-5@low", &list),
            Decision::Rewrite("claude-haiku-5-5".to_string())
        );
        // A missing model is an unlisted one: the agent's own model answers.
        assert_eq!(
            decide_model("openai", "", "openai/gpt-6-luna", &list),
            Decision::Rewrite("gpt-6-luna".to_string())
        );
    }

    #[test]
    fn an_unlisted_model_with_no_same_provider_fallback_is_refused_naming_the_allowed() {
        let list = allowed(&["anthropic/claude-haiku-5-5", "openai/gpt-6-luna"]);
        let Decision::Refuse(message) = decide_model("openai", "gpt-9", "anthropic/claude-haiku-5-5", &list) else {
            panic!("expected a refusal");
        };
        assert!(message.contains("openai/gpt-6-luna"), "{message}");
        assert!(message.contains("anthropic/claude-haiku-5-5"), "{message}");
    }

    #[test]
    fn a_body_naming_an_unlisted_model_is_rewritten_or_refused_in_place() {
        let list = allowed(&["openai/gpt-6-luna"]);
        let mut body = json!({ "model": "gpt-9" });
        check_model(&mut body, "openai", "openai/gpt-6-luna", &list).unwrap();
        assert_eq!(body["model"], "gpt-6-luna");
        let mut body = json!({ "model": "gpt-9" });
        assert!(check_model(&mut body, "anthropic", "openai/gpt-6-luna", &list).is_err());
        assert_eq!(body["model"], "gpt-9");
    }

    #[test]
    fn upstream_urls_follow_the_base_and_its_own_v1() {
        assert_eq!(upstream_url("https://api.anthropic.com", "/v1/messages"), "https://api.anthropic.com/v1/messages");
        assert_eq!(upstream_url("https://api.anthropic.com/", "/v1/messages"), "https://api.anthropic.com/v1/messages");
        assert_eq!(upstream_url("https://api.openai.com/v1", "/v1/responses"), "https://api.openai.com/v1/responses");
        assert_eq!(
            upstream_url("https://api.openai.com/v1/", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(ws_base("https://api.openai.com/v1/"), "https://api.openai.com");
    }

    #[test]
    fn an_anthropic_stream_tail_counts_input_from_start_and_final_output_from_the_last_delta() {
        let tail = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"Hi\"}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":15}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";
        assert_eq!(tokens_in_sse(tail), Some(Tokens { input: 25, output: 15 }));
    }

    #[test]
    fn buffered_bodies_carry_usage_at_the_top_level() {
        let openai = json!({ "id": "c", "choices": [], "usage": { "prompt_tokens": 12, "completion_tokens": 7, "total_tokens": 19 } });
        assert_eq!(tokens_in(&openai), Some(Tokens { input: 12, output: 7 }));
        let anthropic = json!({ "type": "message", "usage": { "input_tokens": 30, "output_tokens": 4 } });
        assert_eq!(tokens_in(&anthropic), Some(Tokens { input: 30, output: 4 }));
        assert_eq!(tokens_in(&json!({ "id": "x" })), None);
    }

    #[test]
    fn a_plain_get_to_a_socket_path_is_still_the_405_it_was() {
        let plain = Request {
            method: "GET".into(),
            path: "/internal/v1/agents/1/gateway/openai/v1/responses".into(),
            ..Default::default()
        };
        assert!(!is_upgrade(&plain));
        let upgrade = Request { headers: vec![("upgrade".into(), "websocket".into())], ..plain.clone() };
        assert!(is_upgrade(&upgrade));
        let refused = not_upgrade();
        assert_eq!(refused.status, 405);
        assert!(refused.headers.iter().any(|(name, value)| name.eq_ignore_ascii_case("allow") && value == "POST"));
    }

    #[test]
    fn a_client_frame_that_is_not_the_frame_its_socket_takes_is_refused() {
        assert!(check_create(r#"{"type":"response.create","model":"gpt-6-luna","input":"hi"}"#).is_ok());
        assert_eq!(
            check_create(r#"{"type":"session.update"}"#).unwrap_err(),
            "the only frame a client sends is response.create"
        );
        assert!(check_create("not json").is_err());
        assert!(check_messages_create(r#"{"type":"response.create","request":{}}"#).is_err());
        assert!(check_messages_create(r#"{"type":"messages.create","id":"a"}"#).is_err());
        let (id, request) = check_messages_create(r#"{"type":"messages.create","id":"a","request":{"model":"m"}}"#).unwrap();
        assert_eq!((id, request), (json!("a"), json!({ "model": "m" })));
    }

    #[test]
    fn one_sse_event_becomes_one_bridge_frame() {
        let frame = sse_frame(&json!("c1"), "content_block_delta", r#"{"type":"content_block_delta","index":0}"#);
        assert_eq!(
            frame,
            json!({ "id": "c1", "event": "content_block_delta", "data": { "type": "content_block_delta", "index": 0 } })
        );
        assert_eq!(sse_frame(&json!(7), "ping", "not json")["data"], json!("not json"));
    }
}
