//! The model gateway for coding engines inside agents' VMs. Claude Code, Codex and
//! OpenCode point their base URLs at `plaza.gateway`; the hub relays each request to
//! `/internal/v1/agents/{cid}/gateway/...`. apid holds the real provider keys, applies
//! the owner's model policy, streams the answer back and meters what it can.

use std::io::{BufRead, BufReader, Read};

use apz_http::{Request, Response, Router};
use serde_json::{json, Value};

use crate::internal::authorized;
use crate::settings::{tier_of, Tier};
use crate::state::App;

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/internal/v1/agents/{cid}/gateway/anthropic/v1/messages", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| relay(app, req, cid, "anthropic", "/v1/messages"))
        })
        .get("/internal/v1/agents/{cid}/gateway/anthropic/v1/models", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| list_models(app, cid, "anthropic"))
        })
        .post("/internal/v1/agents/{cid}/gateway/openai/v1/responses", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| relay(app, req, cid, "openai", "/v1/responses"))
        })
        .post("/internal/v1/agents/{cid}/gateway/openai/v1/chat/completions", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| relay(app, req, cid, "openai", "/v1/chat/completions"))
        })
        .get("/internal/v1/agents/{cid}/gateway/openai/v1/models", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| list_models(app, cid, "openai"))
        })
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

/// The upstream URL for `path` under `base`. A base that already ends in `/v1` takes the
/// path without its own `/v1`.
pub fn upstream_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = if base.ends_with("/v1") { path.strip_prefix("/v1").unwrap_or(path) } else { path };
    format!("{base}{path}")
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

/// Adds one call's tokens to the agent's usage and records it in the transcript.
/// Usage read from streams that ended, waiting to be recorded: the stream writer has no
/// `App`, so the scheduler's loop drains this every couple of seconds ([`drain_usage`]).
fn pending_usage() -> &'static std::sync::Mutex<Vec<(u64, String, Tokens)>> {
    static PENDING: std::sync::OnceLock<std::sync::Mutex<Vec<(u64, String, Tokens)>>> = std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// Records the usage of streams that have ended.
pub fn drain_usage(app: &App) {
    let done: Vec<_> = std::mem::take(&mut *pending_usage().lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for (cid, model, tokens) in done {
        record_usage(app, cid, &model, tokens);
    }
}

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

/// Relays one model call for agent `cid` to `provider`'s `path`, streaming when asked.
fn relay(app: &App, req: &Request, cid: u64, provider: &str, path: &str) -> Result<Response, Response> {
    let agent = app.agents.get(cid).ok_or_else(|| Response::error(404, "unknown agent"))?;
    let mut body: Value = req.json().map_err(|e| Response::error(400, &e.to_string()))?;
    if !app.settings.dev_models() {
        let policy = app.settings.get().model_policy;
        let list = match tier_of(&agent.role) {
            Tier::Orchestrator => policy.orchestrator,
            Tier::Worker => policy.worker,
        };
        let requested = body.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
        match decide_model(provider, &requested, &crate::agents::model_of(app, &agent), &list) {
            Decision::Allow => {}
            Decision::Rewrite(model) => body["model"] = json!(model),
            Decision::Refuse(message) => return Err(Response::json(403, &json!({ "error": message }))),
        }
    }
    let model = body.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
    let (base, key) = app
        .providers
        .upstream(provider)
        .ok_or_else(|| Response::error(503, &format!("no `{provider}` provider is configured")))?;
    let key = key.ok_or_else(|| Response::error(503, &format!("the `{provider}` provider has no key")))?;

    let client = http_agent();
    let url = upstream_url(&base, path);
    let mut call = client.post(url.as_str());
    if provider == "anthropic" {
        call = call.header("x-api-key", key.as_str());
        for name in ["anthropic-version", "anthropic-beta"] {
            if let Some(value) = req.header(name) {
                call = call.header(name, value);
            }
        }
    } else {
        call = call.header("authorization", format!("Bearer {key}"));
    }
    let resp = call.send_json(&body).map_err(|e| Response::error(502, &format!("upstream: {e}")))?;

    let status = resp.status().as_u16();
    let ok = (200..300).contains(&status);
    let streaming = body.get("stream").and_then(Value::as_bool) == Some(true);
    let mut reader = resp.into_body().into_reader();
    if streaming && ok {
        let model_for_meter = model.clone();
        return Ok(Response::sse(move |sse| {
            let mut event = String::new();
            let mut data = String::new();
            // The stream's tail carries its usage; it is metered once the stream ends.
            let mut tail = String::new();
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if line.is_empty() {
                    if !data.is_empty() {
                        let kind = if event.is_empty() { "message".to_string() } else { event.clone() };
                        sse.event(&kind, &data)?;
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
                    tail.drain(..tail.len() - 32 * 1024);
                }
            }
            if let Some(tokens) = tokens_in_sse(&tail) {
                pending_usage().lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((cid, model_for_meter, tokens));
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
    fn upstream_urls_follow_the_base_and_its_own_v1() {
        assert_eq!(upstream_url("https://api.anthropic.com", "/v1/messages"), "https://api.anthropic.com/v1/messages");
        assert_eq!(upstream_url("https://api.anthropic.com/", "/v1/messages"), "https://api.anthropic.com/v1/messages");
        assert_eq!(upstream_url("https://api.openai.com/v1", "/v1/responses"), "https://api.openai.com/v1/responses");
        assert_eq!(
            upstream_url("https://api.openai.com/v1/", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
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
}
