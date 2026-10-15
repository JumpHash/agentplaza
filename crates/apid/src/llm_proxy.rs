//! The model gateway for coding engines inside agents' VMs. Claude Code, Codex, OpenCode and
//! Gemini CLI point their base URLs at `plaza.gateway`; the hub relays each request to
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

use apz_gateway::Caller;
use apz_http::client::Target;
use apz_http::ws::{self, Message, Origins, Socket};
use apz_http::{Request, Response, Router};
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::internal::authorized;
use crate::payments::{charge_model_call, paying_from};
use crate::prices::{Tokens, USD_PER_CREDIT};
use crate::settings::{ModelPolicy, Tier, tier_of};
use crate::state::App;
use crate::usage::Charge;
use crate::vault::VaultError;

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
        .post(
            "/internal/v1/agents/{cid}/gateway/anthropic/v1/messages",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "anthropic", "/v1/messages")
                })
            },
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/anthropic/v1/messages",
            |app, req, p| {
                if !is_upgrade(req) {
                    return not_upgrade();
                }
                authorized(app, req, p.get("cid"), |cid| messages_socket(app, req, cid))
            },
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/anthropic/v1/models",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    list_models(app, cid, "anthropic")
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/openai/v1/responses",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "openai", "/v1/responses")
                })
            },
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/openai/v1/responses",
            |app, req, p| {
                if !is_upgrade(req) {
                    return not_upgrade();
                }
                authorized(app, req, p.get("cid"), |cid| {
                    responses_socket(app, req, cid)
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/openai/v1/chat/completions",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "openai", "/v1/chat/completions")
                })
            },
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/openai/v1/models",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    list_models(app, cid, "openai")
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/openrouter/v1/chat/completions",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "openrouter", "/v1/chat/completions")
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/openrouter/v1/messages",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "openrouter", "/v1/messages")
                })
            },
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/openrouter/v1/models",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    list_models(app, cid, "openrouter")
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/opencode/v1/chat/completions",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "opencode", "/v1/chat/completions")
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/opencode/v1/responses",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "opencode", "/v1/responses")
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/opencode/v1/messages",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    relay(app, req, cid, "opencode", "/v1/messages")
                })
            },
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/opencode/v1/models",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    list_models(app, cid, "opencode")
                })
            },
        )
        .post(
            "/internal/v1/agents/{cid}/gateway/gemini/v1beta/models/{call}",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    let (model, method) = gemini_call(p.get("call")).ok_or_else(|| {
                        Response::error(
                            404,
                            "the gateway relays generateContent, streamGenerateContent and countTokens",
                        )
                    })?;
                    relay_gemini(app, req, cid, model, method)
                })
            },
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/gemini/v1beta/models",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| list_gemini_models(app, cid))
            },
        )
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
    Decision::Refuse(format!(
        "`{requested}` is not allowed for this agent; allowed: {}",
        names.join(", ")
    ))
}

/// Applies the policy to a request body: an allowed model passes, an unlisted one is rewritten
/// to the agent's own model when it is on the same provider, and refused otherwise.
fn check_model(body: &mut Value, provider: &str, own: &str, list: &[String]) -> Result<(), String> {
    let requested = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match decide_model(provider, requested, own, list) {
        Decision::Allow => Ok(()),
        Decision::Rewrite(model) => {
            body["model"] = json!(model);
            Ok(())
        }
        Decision::Refuse(message) => Err(message),
    }
}

/// The tier list an agent's model calls are held to: the orchestrator's list, or `None` when they
/// are not held to one. Workers are not held to a list (they may run any model), nor are dev models.
fn held_list(dev_models: bool, tier: Tier, policy: &ModelPolicy) -> Option<Vec<String>> {
    if dev_models || tier != Tier::Orchestrator {
        return None;
    }
    Some(policy.orchestrator.clone())
}

/// [`held_list`] for `agent`, from the settings as they are now.
fn held_to(app: &App, agent: &Agent) -> Option<Vec<String>> {
    held_list(
        app.settings.dev_models(),
        tier_of(&agent.role),
        &app.settings.get().model_policy,
    )
}

/// The upstream URL for `path` under `base`. A base that already ends in `/v1` takes the
/// path without its own `/v1`.
pub fn upstream_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = if base.ends_with("/v1") {
        path.strip_prefix("/v1").unwrap_or(path)
    } else {
        path
    };
    format!("{base}{path}")
}

/// The host part of a provider base, for a WebSocket target (which takes no path).
fn ws_base(base: &str) -> String {
    let base = base.trim_end_matches('/');
    base.strip_suffix("/v1").unwrap_or(base).to_string()
}

/// One call's metered usage: its tokens, and the cost in USD the provider reported for it
/// (OpenRouter's `usage.cost`), `None` when the provider does not say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metered {
    pub tokens: Tokens,
    pub cost_usd: Option<f64>,
}

/// The counts read from a response's usage objects. A later count replaces an earlier one.
#[derive(Default, Clone, Copy)]
struct Counts {
    /// `input_tokens` (Anthropic, Responses) or `prompt_tokens` (chat, DeepSeek), as sent.
    input: Option<u64>,
    output: Option<u64>,
    /// Cache hits that the provider's input count already includes (OpenAI, DeepSeek).
    cached_in_input: Option<u64>,
    /// DeepSeek's cache misses: its input outside the cache.
    cache_miss: Option<u64>,
    /// Anthropic's cache reads: outside its input count.
    cache_read: Option<u64>,
    /// Anthropic's cache writes.
    cache_write: Option<u64>,
    cost: Option<f64>,
}

fn count(usage: &Value, path: &str) -> Option<u64> {
    usage.pointer(path).and_then(Value::as_u64)
}

/// Folds the usage objects of one JSON event (top level, `message` or `response`) into `acc`;
/// a later count replaces an earlier one.
fn merge(acc: &mut Counts, event: &Value) {
    for usage in [
        event.get("usage"),
        event.pointer("/message/usage"),
        event.pointer("/response/usage"),
    ]
    .into_iter()
    .flatten()
    {
        let input = count(usage, "/input_tokens").or_else(|| count(usage, "/prompt_tokens"));
        let output = count(usage, "/output_tokens").or_else(|| count(usage, "/completion_tokens"));
        let cached = count(usage, "/prompt_tokens_details/cached_tokens")
            .or_else(|| count(usage, "/input_tokens_details/cached_tokens"))
            .or_else(|| count(usage, "/prompt_cache_hit_tokens"));
        acc.input = input.or(acc.input);
        acc.output = output.or(acc.output);
        acc.cached_in_input = cached.or(acc.cached_in_input);
        acc.cache_miss = count(usage, "/prompt_cache_miss_tokens").or(acc.cache_miss);
        acc.cache_read = count(usage, "/cache_read_input_tokens").or(acc.cache_read);
        acc.cache_write = count(usage, "/cache_creation_input_tokens").or(acc.cache_write);
        acc.cost = usage.get("cost").and_then(Value::as_f64).or(acc.cost);
    }
}

/// Normalizes the counts: OpenAI-style input includes its cache hits and Anthropic's does not,
/// so the input here is the tokens billed at the full input price (DeepSeek's miss count when sent).
fn finish(acc: Counts) -> Option<Metered> {
    if acc.input.is_none() && acc.output.is_none() && acc.cost.is_none() {
        return None;
    }
    let cached = acc.cached_in_input.unwrap_or(0);
    let input = match (acc.cache_miss, acc.input) {
        (Some(miss), _) => miss,
        (None, Some(input)) => input.saturating_sub(cached),
        (None, None) => 0,
    };
    let tokens = Tokens {
        input,
        output: acc.output.unwrap_or(0),
        cache_read: cached + acc.cache_read.unwrap_or(0),
        cache_write: acc.cache_write.unwrap_or(0),
    };
    Some(Metered {
        tokens,
        cost_usd: acc.cost,
    })
}

/// The usage of a buffered Anthropic or OpenAI response body.
pub fn tokens_in(body: &Value) -> Option<Metered> {
    let mut acc = Counts::default();
    merge(&mut acc, body);
    finish(acc)
}

/// The usage of a whole stream from its SSE text. Anthropic's `message_start` gives the
/// input and its `message_delta` the final output, so the last count of each wins.
pub fn tokens_in_sse(text: &str) -> Option<Metered> {
    let mut acc = Counts::default();
    for data in text.lines().filter_map(|line| line.strip_prefix("data:")) {
        if let Ok(event) = serde_json::from_str::<Value>(data.trim()) {
            merge(&mut acc, &event);
        }
    }
    finish(acc)
}

/// Usage read from streams and sockets, waiting to be recorded: the stream writer has no
/// `App`, so the scheduler's loop drains this every couple of seconds ([`drain_usage`]). Each
/// entry carries the charge the call started under (its task and work item), read when the call
/// started: the usage is recorded only after the call ends, by which time the agent may have moved on.
type PendingUsage = Vec<(u64, Charge, String, Metered)>;

fn pending_usage() -> &'static std::sync::Mutex<PendingUsage> {
    static PENDING: std::sync::OnceLock<std::sync::Mutex<PendingUsage>> =
        std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

fn defer_usage(cid: u64, charge: Charge, model: String, metered: Metered) {
    pending_usage()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((cid, charge, model, metered));
}

/// The usage of one Gemini `usageMetadata` object. Gemini's prompt count includes the cached part
/// (reported apart as `cachedContentTokenCount`), and its thinking tokens are billed as output.
/// Gemini reports no cost, so the price table estimates it.
fn gemini_metered(usage: &Value) -> Option<Metered> {
    let prompt = count(usage, "/promptTokenCount");
    let candidates = count(usage, "/candidatesTokenCount");
    if prompt.is_none() && candidates.is_none() {
        return None;
    }
    let cached = count(usage, "/cachedContentTokenCount").unwrap_or(0);
    Some(Metered {
        tokens: Tokens {
            input: prompt.unwrap_or(0).saturating_sub(cached),
            output: candidates.unwrap_or(0) + count(usage, "/thoughtsTokenCount").unwrap_or(0),
            cache_read: cached,
            cache_write: 0,
        },
        cost_usd: None,
    })
}

/// The usage of a Gemini response: a JSON object, or the array of chunks a stream sent without
/// `alt=sse`. Each `usageMetadata` counts the call so far, so the last one wins.
fn tokens_in_gemini(body: &Value) -> Option<Metered> {
    match body {
        Value::Array(chunks) => chunks
            .iter()
            .rev()
            .find_map(|chunk| chunk.get("usageMetadata").and_then(gemini_metered)),
        other => other.get("usageMetadata").and_then(gemini_metered),
    }
}

/// The usage of a Gemini stream from its SSE text: the last event that carries `usageMetadata`.
fn tokens_in_gemini_sse(text: &str) -> Option<Metered> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .filter_map(|event| event.get("usageMetadata").and_then(gemini_metered))
        .last()
}

/// The model and method a Gemini path names (`{model}:{method}` after `/v1beta/models/`), or `None`
/// for a method the gateway does not relay. The model must be a plain id: it goes into the upstream path.
fn gemini_call(call: &str) -> Option<(&str, &'static str)> {
    let (model, method) = call.rsplit_once(':')?;
    let method = ["generateContent", "streamGenerateContent", "countTokens"]
        .into_iter()
        .find(|known| *known == method)?;
    let plain = !model.is_empty()
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    plain.then_some((model, method))
}

/// Records the usage of streams that have ended.
pub fn drain_usage(app: &App) {
    let done: Vec<_> = std::mem::take(
        &mut *pending_usage()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for (cid, charge, model, metered) in done {
        record_usage(app, cid, charge, &model, metered);
    }
}

/// Adds one call's usage to the agent's usage, records it in the transcript and meters it into
/// the usage ledger, charged to the task and work item the call started under (`charge`, read when
/// it started). The transcript entry stays under no work: a work's transcript is the agent's own
/// text, and the feed would show these calls as that.
fn record_usage(app: &App, cid: u64, charge: Charge, model: &str, metered: Metered) {
    let t = metered.tokens;
    let usage = serde_json::from_value(json!({
        "input_tokens": t.input,
        "output_tokens": t.output,
        "cache_read_tokens": t.cache_read,
        "cache_write_tokens": t.cache_write,
    }))
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
            "usage": {
                "input_tokens": t.input,
                "output_tokens": t.output,
                "cache_read_tokens": t.cache_read,
                "cache_write_tokens": t.cache_write,
            },
            "cost_usd": metered.cost_usd,
        }),
    );
    bill_call(app, cid, charge.task.as_deref(), model, t, metered.cost_usd);
    crate::usage::meter_charged(app, cid, model, t, metered.cost_usd, charge, "gateway");
}

/// The task agent `cid` is working on, if any: the one its usage is charged to.
fn current_task(app: &App, cid: u64) -> Option<String> {
    app.work.current(cid).and_then(|item| item.origin.task)
}

/// The charge for a model call agent `cid` starts now: the task it works on and the work item it runs
/// under (0 for none). Read when the call starts, since its usage is recorded only after the call ends.
fn current_charge(app: &App, cid: u64) -> Charge {
    match app.work.current(cid) {
        Some(item) => Charge {
            task: item.origin.task,
            work: item.id,
        },
        None => Charge::default(),
    }
}

/// The 402 message when agent `cid` cannot pay for a model call, or `None` when it can (or when
/// charging is off).
fn refusal(charging: bool, task: Option<&str>, escrow: i128, wallet: i128) -> Option<String> {
    if !charging || paying_from(task, escrow, wallet).1 > 0 {
        return None;
    }
    Some(match task {
        Some(_) => format!(
            "cannot pay for this model call: wallet balance {wallet} credits, the task's escrow has {escrow}; top up or finish the task"
        ),
        None => format!(
            "cannot pay for this model call: wallet balance {wallet} credits; top up or finish the task"
        ),
    })
}

/// The pre-call check: the refusal for agent `cid`'s model call, read from the treasury now.
fn payment_refusal(app: &App, cid: u64) -> Option<String> {
    let charging = app.settings.get().payments.charge_models;
    let task = current_task(app, cid);
    let treasury = &app.plaza.treasury;
    let escrow = task
        .as_deref()
        .map_or(0, |task| treasury.escrow_balance(task));
    refusal(charging, task.as_deref(), escrow, treasury.wallet(cid))
}

/// The credits one call costs: its reported USD cost, else `estimate` (USD), over
/// [`USD_PER_CREDIT`], rounded up to a whole credit. Zero when there is nothing to charge.
fn credits_for(cost_usd: Option<f64>, estimate: impl FnOnce() -> f64) -> i64 {
    let usd = cost_usd.unwrap_or_else(estimate);
    if !usd.is_finite() || usd <= 0.0 {
        return 0;
    }
    // The tolerance keeps float noise (50.00000000000001 credits) from rounding a whole credit up.
    (usd / USD_PER_CREDIT - 1e-9).ceil() as i64
}

/// Bills agent `cid`'s model call after it ran, through the shared charge
/// ([`charge_model_call`]). A shortfall is noted in the transcript.
fn bill_call(
    app: &App,
    cid: u64,
    task: Option<&str>,
    model: &str,
    tokens: Tokens,
    cost_usd: Option<f64>,
) {
    let charging = app.settings.get().payments.charge_models;
    let credits = credits_for(cost_usd, || {
        let settings = app.settings.get();
        app.prices
            .lookup(model, &settings.prices)
            .map_or(0.0, |p| crate::prices::estimate(&p.price, &tokens))
    });
    let spend = |task: &str, credits: i64| app.work.add_spend(task, credits);
    let charge = charge_model_call(
        &app.plaza.treasury,
        charging,
        spend,
        cid,
        task,
        credits,
        &format!("model {model}"),
    );
    if let Some(settled) = charge.filter(|settled| settled.charged < settled.wanted) {
        let available = settled.available.clamp(0, i128::from(i64::MAX)) as i64;
        app.transcripts.record(
            &app.bus,
            cid,
            0,
            "model_charge_shortfall",
            json!({
                "model": model,
                "task": task,
                "wanted": settled.wanted,
                "charged": settled.charged,
                "available": available,
                "error": settled.error,
            }),
        );
    }
}

/// An upstream client that returns error statuses as responses, so their bodies pass through.
fn http_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .http_status_as_error(false)
            .build(),
    )
}

/// The Anthropic headers a client sends that the upstream must get too.
fn anthropic_headers(req: &Request) -> Vec<(String, String)> {
    ["anthropic-version", "anthropic-beta"]
        .into_iter()
        .filter_map(|name| {
            req.header(name)
                .map(|value| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// The provider's upstream base and key for agent `cid`'s call, as the HTTP path takes them. The key
/// is the one the vault holds for the agent's node (see `providers::Store::upstream`).
fn provider_upstream(app: &App, provider: &str, cid: u64) -> Result<(String, String), Response> {
    let caller = Caller::Agent {
        cid,
        node: crate::vault::node_for_cid(&app.nodes.list(), cid),
    };
    upstream_for(&app.providers, provider, &caller)
        .map_err(|(status, message)| Response::error(status, &message))
}

/// [`provider_upstream`] over the providers store: `Err` is the status and message that refuse the call.
/// A sealed, uninitialised or tampered vault refuses it with 503 (`vault sealed`, or `vault tampered: …`);
/// a key only other nodes hold refuses it with 403. No key is taken from the environment instead.
fn upstream_for(
    providers: &crate::providers::Store,
    provider: &str,
    caller: &Caller,
) -> Result<(String, String), (u16, String)> {
    let (base, key) = providers
        .upstream(provider, caller)
        .map_err(|err| match err {
            VaultError::Tampered(_) => (503, err.message()),
            VaultError::OutOfScope => (403, err.message()),
            _ => (503, "vault sealed".to_string()),
        })?
        .ok_or_else(|| (503, format!("no `{provider}` provider is configured")))?;
    let key = key.ok_or_else(|| (503, format!("the `{provider}` provider has no key")))?;
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
    } else if provider == "gemini" {
        // Gemini takes its key in its own header, not as a bearer token.
        call = call.header("x-goog-api-key", key);
    } else {
        call = call.header("authorization", format!("Bearer {key}"));
        // The Messages path of OpenRouter and OpenCode takes the key as `x-api-key` too.
        if is_aggregator(provider) && path.ends_with("/messages") {
            call = call.header("x-api-key", key);
        }
    }
    for (name, value) in headers {
        call = call.header(name.as_str(), value.as_str());
    }
    call.send_json(body).map_err(|e| format!("upstream: {e}"))
}

/// OpenRouter and OpenCode, whose model ids are `provider/model` (the first `/` splits the provider off).
fn is_aggregator(provider: &str) -> bool {
    matches!(provider, "openrouter" | "opencode")
}

/// The headers a relayed call sends upstream beyond its key: the Anthropic version and beta headers
/// for Anthropic and for the Messages path of OpenRouter and OpenCode, and those two providers' extra headers.
fn upstream_headers(app: &App, req: &Request, provider: &str, path: &str) -> Vec<(String, String)> {
    let mut headers =
        if provider == "anthropic" || (is_aggregator(provider) && path.ends_with("/messages")) {
            anthropic_headers(req)
        } else {
            Vec::new()
        };
    if is_aggregator(provider) {
        headers.extend(app.providers.extra_headers(provider));
    }
    headers
}

/// Why OpenCode cannot serve a call to `path` for `model` (provider-relative, as the agent names it), or
/// `None` when it can. The Messages endpoint takes `claude-` models and a few Qwen ones; the Responses
/// endpoint takes `gpt-`, `grok-` and `muse-spark-`; chat completions takes the rest, bar `claude-`,
/// `gemini-` and `jev-`.
pub fn opencode_endpoint_error(path: &str, model: &str) -> Option<String> {
    let model = canonical(model);
    let responses =
        model.starts_with("gpt-") || model.starts_with("grok-") || model.starts_with("muse-spark-");
    if path.ends_with("messages") {
        let qwen = [
            "qwen3.8-flash",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.6-plus",
            "qwen3.5-plus",
        ];
        if model.starts_with("claude-") || qwen.contains(&model) {
            return None;
        }
        let (name, endpoint) = if responses {
            ("responses", "/v1/responses")
        } else {
            ("chat completions", "/v1/chat/completions")
        };
        return Some(format!(
            "OpenCode serves `{model}` on its {name} endpoint ({endpoint}), not Messages, so Claude Code cannot call it"
        ));
    }
    if path.ends_with("responses") {
        if responses {
            return None;
        }
        let endpoint = if model.starts_with("claude-") {
            "/v1/messages"
        } else {
            "/v1/chat/completions"
        };
        return Some(format!(
            "the responses endpoint serves only gpt-, grok- and muse-spark- models; `{model}` belongs on {endpoint}"
        ));
    }
    if path.ends_with("chat/completions") {
        if model.starts_with("claude-") {
            return Some(format!(
                "`{model}` is served on /v1/messages, not chat completions"
            ));
        }
        if model.starts_with("gemini-") || model.starts_with("jev-") {
            return Some(format!(
                "OpenCode does not serve gemini- or jev- models on chat completions; `{model}` is not relayed here"
            ));
        }
    }
    None
}

/// Relays one model call for agent `cid` to `provider`'s `path`, streaming when asked.
fn relay(
    app: &App,
    req: &Request,
    cid: u64,
    provider: &str,
    path: &str,
) -> Result<Response, Response> {
    // Held by STOP ALL like every other model call (`internal::model_call`), not only the sockets.
    if app.control.paused() {
        return Err(Response::error(
            409,
            "the plaza is held (STOP ALL); waiting for the owner to resume",
        ));
    }
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| Response::error(404, "unknown agent"))?;
    let charge = current_charge(app, cid);
    let mut body: Value = req
        .json()
        .map_err(|e| Response::error(400, &e.to_string()))?;
    // Workers may call any model; the orchestrator is held to its list.
    if let Some(list) = held_to(app, &agent) {
        let own = crate::agents::model_of(app, &agent);
        check_model(&mut body, provider, &own, &list)
            .map_err(|message| Response::json(403, &json!({ "error": message })))?;
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if provider == "opencode" {
        if let Some(message) = opencode_endpoint_error(path, &model) {
            return Err(Response::json(400, &json!({ "error": message })));
        }
    }
    // OpenRouter and OpenCode models are metered as `provider/model`, as the policy names them.
    let model = if is_aggregator(provider) {
        format!("{provider}/{model}")
    } else {
        model
    };
    if let Some(message) = payment_refusal(app, cid) {
        return Err(Response::json(402, &json!({ "error": message })));
    }
    let (base, key) = provider_upstream(app, provider, cid)?;
    let headers = upstream_headers(app, req, provider, path);
    let resp = upstream_post(provider, &base, &key, path, &headers, &body)
        .map_err(|e| Response::error(502, &e))?;

    let status = resp.status().as_u16();
    let ok = (200..300).contains(&status);
    let streaming = body.get("stream").and_then(Value::as_bool) == Some(true);
    let mut reader = resp.into_body().into_reader();
    if streaming && ok {
        return Ok(Response::sse(move |sse| {
            // The stream's tail carries its usage; it is metered once the stream ends.
            let tail = read_sse(BufReader::new(reader), |kind, data| sse.event(kind, data))?;
            if let Some(tokens) = tokens_in_sse(&tail) {
                defer_usage(cid, charge, model, tokens);
            }
            Ok(())
        }));
    }

    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| Response::error(502, &format!("upstream: {e}")))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Response::error(502, "the upstream body is not JSON"))?;
    if ok {
        if let Some(tokens) = tokens_in(&value) {
            record_usage(app, cid, charge, &model, tokens);
        }
    }
    Ok(Response::json(status, &value))
}

/// The refusal for a Gemini body that names a model other than the path's `model`, or `None` when it names
/// none or the same one. A body names its model at the top level or, for countTokens, inside
/// `generateContentRequest`; Gemini spells it `models/{id}` there, so that prefix is dropped before the compare.
fn gemini_body_mismatch(body: &Value, model: &str) -> Option<String> {
    [
        body.get("model"),
        body.pointer("/generateContentRequest/model"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(|named| named.strip_prefix("models/").unwrap_or(named))
    .find(|named| *named != model)
    .map(|other| {
        format!("`{other}` is not allowed for this agent; the call names `{model}` in its path")
    })
}

/// Relays one Gemini API call for agent `cid`: `model` and `method` as the path names them. The agent's
/// key is the placeholder the CLI holds; the provider's key goes upstream in Gemini's own header, and the
/// agent's key never does. The model is metered bare, as [`relay`] meters the other providers.
fn relay_gemini(
    app: &App,
    req: &Request,
    cid: u64,
    model: &str,
    method: &str,
) -> Result<Response, Response> {
    if app.control.paused() {
        return Err(Response::error(
            409,
            "the plaza is held (STOP ALL); waiting for the owner to resume",
        ));
    }
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| Response::error(404, "unknown agent"))?;
    let charge = current_charge(app, cid);
    let body: Value = req
        .json()
        .map_err(|e| Response::error(400, &e.to_string()))?;
    // The model is in the path, so the policy checks a body that names only it. A body that names
    // another model would be sent under this path's model and metered as well; it is refused.
    if let Some(message) = gemini_body_mismatch(&body, model) {
        return Err(Response::json(403, &json!({ "error": message })));
    }
    let mut named = json!({ "model": model });
    if let Some(list) = held_to(app, &agent) {
        let own = crate::agents::model_of(app, &agent);
        check_model(&mut named, "gemini", &own, &list)
            .map_err(|message| Response::json(403, &json!({ "error": message })))?;
    }
    let model = named
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(message) = payment_refusal(app, cid) {
        return Err(Response::json(402, &json!({ "error": message })));
    }
    let (base, key) = provider_upstream(app, "gemini", cid)?;
    // Only `alt=sse` is forwarded: the query string is otherwise the client's to leave out.
    let event_stream = method == "streamGenerateContent" && req.query_param("alt") == Some("sse");
    let mut path = format!("/v1beta/models/{model}:{method}");
    if event_stream {
        path.push_str("?alt=sse");
    }
    let resp = upstream_post("gemini", &base, &key, &path, &[], &body)
        .map_err(|e| Response::error(502, &e))?;

    let status = resp.status().as_u16();
    let ok = (200..300).contains(&status);
    let mut reader = resp.into_body().into_reader();
    if event_stream && ok {
        return Ok(Response::sse(move |sse| {
            // The stream's last event carries the usage; it is metered once the stream ends.
            let tail = read_sse(BufReader::new(reader), |kind, data| sse.event(kind, data))?;
            if let Some(tokens) = tokens_in_gemini_sse(&tail) {
                defer_usage(cid, charge, model, tokens);
            }
            Ok(())
        }));
    }

    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| Response::error(502, &format!("upstream: {e}")))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Response::error(502, "the upstream body is not JSON"))?;
    if ok {
        if let Some(tokens) = tokens_in_gemini(&value) {
            record_usage(app, cid, charge, &model, tokens);
        }
    }
    Ok(Response::json(status, &value))
}

/// Reads an SSE body, handing each event (its name, `message` when unnamed, and its data) to
/// `on_event`. Returns the stream's tail, for metering.
fn read_sse(
    reader: impl BufRead,
    mut on_event: impl FnMut(&str, &str) -> io::Result<()>,
) -> io::Result<String> {
    let mut event = String::new();
    let mut data = String::new();
    let mut tail = String::new();
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.is_empty() {
            if !data.is_empty() {
                let kind = if event.is_empty() {
                    "message"
                } else {
                    event.as_str()
                };
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

/// What a WebSocket's model calls need: the agent, the hub (to read the hold and the agent's
/// policy afresh for every call, not once when the socket opened), and the provider's upstream.
struct Grant {
    cid: u64,
    app: std::sync::Weak<App>,
    base: String,
    key: String,
    headers: Vec<(String, String)>,
}

fn grant_for(app: &App, req: &Request, cid: u64, provider: &str) -> Result<Grant, Response> {
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| Response::error(404, "unknown agent"))?;
    let (base, key) = provider_upstream(app, provider, cid)?;
    let _ = agent;
    Ok(Grant {
        cid,
        app: app.handle(),
        base,
        key,
        headers: anthropic_headers(req),
    })
}

/// The gateway's policy for one WebSocket call, as over HTTP, read live: STOP ALL holds the call,
/// the agent's current model and the list it is held to apply (a change since the socket opened
/// counts), and an agent that cannot pay is refused with the same message as over HTTP's 402.
/// The charge the call starts under is returned: its usage is metered under it.
fn admit(grant: &Grant, provider: &str, body: &mut Value) -> Result<Charge, String> {
    let app = grant.app.upgrade().ok_or("the hub is shutting down")?;
    if app.control.paused() {
        return Err("the plaza is held (STOP ALL); waiting for the owner to resume".into());
    }
    let agent = app.agents.get(grant.cid).ok_or("unknown agent")?;
    if let Some(list) = held_to(&app, &agent) {
        check_model(
            body,
            provider,
            &crate::agents::model_of(&app, &agent),
            &list,
        )?;
    }
    if let Some(message) = payment_refusal(&app, grant.cid) {
        return Err(message);
    }
    Ok(current_charge(&app, grant.cid))
}

fn limits() -> ws::Limits {
    ws::Limits {
        max_frame: 8 << 20,
        max_message: 8 << 20,
        ..Default::default()
    }
}

fn text(value: Value) -> Message {
    Message::Text(value.to_string())
}

fn responses_socket(app: &App, req: &Request, cid: u64) -> Result<Response, Response> {
    let grant = grant_for(app, req, cid, "openai")?;
    Ok(ws::accept(
        req,
        Origins::None,
        None,
        limits(),
        move |socket| relay_responses(socket, grant),
    ))
}

fn messages_socket(app: &App, req: &Request, cid: u64) -> Result<Response, Response> {
    let grant = grant_for(app, req, cid, "anthropic")?;
    Ok(ws::accept(
        req,
        Origins::None,
        Some(MESSAGES_PROTOCOL),
        limits(),
        move |socket| relay_messages(socket, grant),
    ))
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
    let data =
        serde_json::from_str::<Value>(data).unwrap_or_else(|_| Value::String(data.to_string()));
    json!({ "id": id, "event": event, "data": data })
}

/// Meters a `response.completed` event's usage, as the HTTP path meters a response.
fn meter_responses(cid: u64, charge: &Charge, model: &str, frame: &str) {
    let Ok(event) = serde_json::from_str::<Value>(frame) else {
        return;
    };
    if event.get("type").and_then(Value::as_str) != Some("response.completed") {
        return;
    }
    if let Some(tokens) = tokens_in(&event) {
        defer_usage(cid, charge.clone(), model.to_string(), tokens);
    }
}

/// The OpenAI Responses socket: each `response.create` is checked and sent upstream; every
/// upstream frame goes back unchanged. One thread reads both sockets in turns.
fn relay_responses(mut client: Socket, grant: Grant) {
    let target = Target {
        base: ws_base(&grant.base),
        bearer: Some(grant.key.clone()),
        ..Default::default()
    };
    let mut upstream = match ws::connect(
        &target,
        "/v1/responses",
        None,
        Duration::from_secs(15),
        limits(),
    ) {
        Ok(socket) => socket,
        Err(_) => {
            client.close(1011, "the upstream is unavailable");
            return;
        }
    };
    let started = Instant::now();
    let mut model = String::new();
    // The charge of the call in flight, read when its `response.create` was admitted.
    let mut charge = Charge::default();
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
                match admit(&grant, "openai", &mut body) {
                    Ok(admitted) => charge = admitted,
                    Err(message) => {
                        if client
                            .send(&text(
                                json!({ "type": "error", "error": { "message": message } }),
                            ))
                            .is_err()
                        {
                            upstream.close(1000, "");
                            return;
                        }
                        continue;
                    }
                }
                model = body
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
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
                    meter_responses(grant.cid, &charge, &model, frame);
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
        let wait = if queue.is_empty() {
            TICK
        } else {
            Duration::from_millis(1)
        };
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
fn bridge_call(
    client: &mut Socket,
    grant: &Grant,
    id: &Value,
    mut request: Value,
) -> io::Result<()> {
    request["stream"] = json!(true);
    match stream_messages(client, grant, id, &mut request) {
        Ok(()) => client.send(&text(json!({ "id": id, "event": "done" }))),
        Err(message) => client.send(&text(
            json!({ "id": id, "event": "error", "data": { "message": message } }),
        )),
    }
}

fn stream_messages(
    client: &mut Socket,
    grant: &Grant,
    id: &Value,
    request: &mut Value,
) -> Result<(), String> {
    let charge = admit(grant, "anthropic", request)?;
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let resp = upstream_post(
        "anthropic",
        &grant.base,
        &grant.key,
        "/v1/messages",
        &grant.headers,
        request,
    )?;
    let status = resp.status().as_u16();
    let mut reader = resp.into_body().into_reader();
    if !(200..300).contains(&status) {
        let mut body = String::new();
        reader
            .read_to_string(&mut body)
            .map_err(|e| e.to_string())?;
        return Err(format!("upstream answered {status}: {body}"));
    }
    let tail = read_sse(BufReader::new(reader), |kind, data| {
        client.send(&text_frame(id, kind, data))
    })
    .map_err(|e| e.to_string())?;
    if let Some(tokens) = tokens_in_sse(&tail) {
        defer_usage(grant.cid, charge, model, tokens);
    }
    Ok(())
}

/// The bridge frame of one SSE event, as a socket message.
fn text_frame(id: &Value, event: &str, data: &str) -> Message {
    text(sse_frame(id, event, data))
}

/// The model ids agent `cid` may use on `provider`: its tier's list, without the provider's prefix.
fn allowed_ids(app: &App, cid: u64, provider: &str) -> Result<Vec<String>, Response> {
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| Response::error(404, "unknown agent"))?;
    let policy = app.settings.get().model_policy;
    let list = match tier_of(&agent.role) {
        Tier::Orchestrator => policy.orchestrator,
        Tier::Worker => policy.worker,
    };
    let prefix = format!("{provider}/");
    Ok(list
        .iter()
        .map(|m| canonical(m))
        .filter_map(|m| m.strip_prefix(prefix.as_str()))
        .map(str::to_string)
        .collect())
}

/// The models agent `cid` may use on `provider`, in that provider's list shape.
fn list_models(app: &App, cid: u64, provider: &str) -> Result<Response, Response> {
    let owned = allowed_ids(app, cid, provider)?;
    let ids: Vec<&str> = owned.iter().map(String::as_str).collect();
    let body = if provider == "anthropic" {
        let data: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "type": "model", "id": id, "display_name": id }))
            .collect();
        json!({ "data": data, "has_more": false, "first_id": ids.first(), "last_id": ids.last() })
    } else {
        let data: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "id": id, "object": "model", "owned_by": provider }))
            .collect();
        json!({ "object": "list", "data": data })
    };
    Ok(Response::json(200, &body))
}

/// The Gemini models agent `cid` may use, in Gemini's list shape (`models/{id}` names).
fn list_gemini_models(app: &App, cid: u64) -> Result<Response, Response> {
    let models: Vec<Value> = allowed_ids(app, cid, "gemini")?
        .iter()
        .map(|id| json!({ "name": format!("models/{id}") }))
        .collect();
    Ok(Response::json(200, &json!({ "models": models })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_vault_refuses_the_upstream_call_with_503_and_no_key() {
        use crate::vault::{NewSecret, Scope};
        use secrecy::SecretString;
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("apid-llm-sealed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = apz_plaza::now_ms();
        let vault = Arc::new(crate::vault::Store::open(
            &dir,
            &dir.join("vault.key"),
            None,
        ));
        vault.boot(now).unwrap();
        vault
            .create(
                NewSecret {
                    name: "ANTHROPIC_API_KEY".into(),
                    kind: "api_key".into(),
                    label: "test".into(),
                    value: SecretString::from("PLAZA-VALUE-4417".to_string()),
                    scope: Scope::Plaza,
                    bindings: vec!["provider:anthropic".into()],
                },
                "owner",
                now,
            )
            .unwrap();
        let providers = crate::providers::Store::load(&dir, Arc::clone(&vault)).unwrap();
        let caller = Caller::Agent {
            cid: 7,
            node: Some("n3".into()),
        };
        // Unsealed, the entry is served.
        assert_eq!(
            upstream_for(&providers, "anthropic", &caller)
                .map(|(_, key)| key)
                .as_deref(),
            Ok("PLAZA-VALUE-4417")
        );
        vault.seal("owner", now).unwrap();
        assert_eq!(
            upstream_for(&providers, "anthropic", &caller),
            Err((503, "vault sealed".to_string()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gemini_path_names_a_plain_model_and_one_of_three_methods() {
        assert_eq!(
            gemini_call("gemini-3-flash:generateContent"),
            Some(("gemini-3-flash", "generateContent"))
        );
        assert_eq!(
            gemini_call("gemini-2.5-pro:streamGenerateContent"),
            Some(("gemini-2.5-pro", "streamGenerateContent"))
        );
        assert_eq!(
            gemini_call("gemini-2.5-flash:countTokens"),
            Some(("gemini-2.5-flash", "countTokens"))
        );
        // Other methods, a missing method, and any model that could reach another upstream path.
        assert_eq!(gemini_call("gemini-2.5-flash:embedContent"), None);
        assert_eq!(gemini_call("gemini-2.5-flash"), None);
        assert_eq!(gemini_call(":generateContent"), None);
        assert_eq!(gemini_call("models/x:generateContent"), None);
        assert_eq!(gemini_call("../x:generateContent"), None);
    }

    #[test]
    fn gemini_usage_is_read_from_usage_metadata_with_cached_and_thinking_tokens() {
        let body = json!({
            "candidates": [],
            "usageMetadata": {
                "promptTokenCount": 1200,
                "cachedContentTokenCount": 200,
                "candidatesTokenCount": 50,
                "thoughtsTokenCount": 30,
            }
        });
        let metered = tokens_in_gemini(&body).expect("usage");
        assert_eq!(metered.tokens.input, 1000);
        assert_eq!(metered.tokens.output, 80);
        assert_eq!(metered.tokens.cache_read, 200);
        assert_eq!(metered.cost_usd, None);
        assert!(tokens_in_gemini(&json!({ "candidates": [] })).is_none());
    }

    #[test]
    fn a_gemini_stream_is_metered_by_its_last_usage_metadata() {
        let sse = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":1}}

data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":7}}

"#;
        let metered = tokens_in_gemini_sse(sse).expect("usage");
        assert_eq!((metered.tokens.input, metered.tokens.output), (10, 7));
        assert!(tokens_in_gemini_sse("data: {\"candidates\":[]}\n\n").is_none());
    }

    #[test]
    fn a_buffered_gemini_stream_takes_its_usage_from_the_last_chunk_that_has_it() {
        let chunks = json!([
            { "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 2 } },
            { "candidates": [] }
        ]);
        let metered = tokens_in_gemini(&chunks).expect("usage");
        assert_eq!((metered.tokens.input, metered.tokens.output), (5, 2));
    }

    #[test]
    fn a_deferred_call_keeps_the_work_it_started_under() {
        // The queue is shared by the process, so the test takes back only its own agent's entries.
        let cid = 9_100_001;
        let tokens = Tokens {
            input: 3,
            output: 4,
            cache_read: 0,
            cache_write: 0,
        };
        let started = Charge {
            task: Some("task-7".into()),
            work: 42,
        };
        defer_usage(
            cid,
            started.clone(),
            "anthropic/x".into(),
            Metered {
                tokens,
                cost_usd: None,
            },
        );
        let mut pending = pending_usage()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *pending)
            .into_iter()
            .partition(|entry| entry.0 == cid);
        *pending = rest;
        drop(pending);
        assert_eq!(mine.len(), 1);
        let (_, charge, model, metered) = &mine[0];
        // The task is the one the call started under, not whatever the agent works on when it drains.
        assert_eq!(charge, &started);
        assert_eq!(model, "anthropic/x");
        assert_eq!(metered.tokens, tokens);
    }

    #[test]
    fn a_gemini_body_naming_another_model_is_refused() {
        // countTokens names its model inside generateContentRequest; any body may name one at the top level.
        let count = json!({ "generateContentRequest": { "model": "models/gemini-2.5-pro" } });
        assert!(gemini_body_mismatch(&count, "gemini-2.5-flash").is_some());
        let top = json!({ "model": "gemini-2.5-pro", "contents": [] });
        assert!(gemini_body_mismatch(&top, "gemini-2.5-flash").is_some());
        // The same model, with or without the `models/` prefix, and bodies that name none, pass.
        let same = json!({ "generateContentRequest": { "model": "models/gemini-2.5-flash" } });
        assert_eq!(gemini_body_mismatch(&same, "gemini-2.5-flash"), None);
        let bare = json!({ "model": "gemini-2.5-flash" });
        assert_eq!(gemini_body_mismatch(&bare, "gemini-2.5-flash"), None);
        assert_eq!(
            gemini_body_mismatch(&json!({ "contents": [] }), "gemini-2.5-flash"),
            None
        );
        let message = gemini_body_mismatch(&count, "gemini-2.5-flash").unwrap();
        assert!(message.contains("gemini-2.5-pro") && message.contains("gemini-2.5-flash"));
    }

    #[test]
    fn gemini_calls_are_held_to_the_gemini_list() {
        let list = vec!["gemini/gemini-3-flash".to_string()];
        assert_eq!(
            decide_model("gemini", "gemini-3-flash", "anthropic/claude-x", &list),
            Decision::Allow
        );
        assert_eq!(
            decide_model("gemini", "gemini-3-pro", "gemini/gemini-3-flash", &list),
            Decision::Rewrite("gemini-3-flash".to_string())
        );
        assert!(matches!(
            decide_model("gemini", "gemini-3-pro", "anthropic/claude-x", &list),
            Decision::Refuse(_)
        ));
    }

    /// A booted vault in `dir` with one Anthropic key entry in `scope`.
    fn anthropic_key_in(
        dir: &std::path::Path,
        scope: crate::vault::Scope,
    ) -> std::sync::Arc<crate::vault::Store> {
        use crate::vault::NewSecret;
        use secrecy::SecretString;
        let now = apz_plaza::now_ms();
        let vault = crate::vault::Store::open(dir, &dir.join("vault.key"), None);
        vault.boot(now).unwrap();
        vault
            .create(
                NewSecret {
                    name: "ANTHROPIC_API_KEY".into(),
                    kind: "api_key".into(),
                    label: "test".into(),
                    value: SecretString::from("PLAZA-VALUE-4417".to_string()),
                    scope,
                    bindings: vec!["provider:anthropic".into()],
                },
                "owner",
                now,
            )
            .unwrap();
        std::sync::Arc::new(vault)
    }

    #[test]
    fn a_tampered_vault_refuses_the_upstream_call_with_503_naming_the_tamper() {
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("apid-llm-tampered-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vault = anthropic_key_in(&dir, crate::vault::Scope::Plaza);
        drop(vault);
        let path = dir.join("vault.json");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replacen("provider:anthropic", "provider:anthropix", 1),
        )
        .unwrap();
        // Boot refuses the changed file; the vault stays up to say why.
        let vault = crate::vault::Store::open(&dir, &dir.join("vault.key"), None);
        let _ = vault.boot(apz_plaza::now_ms());
        let providers = crate::providers::Store::load(&dir, Arc::new(vault)).unwrap();
        let caller = Caller::Agent {
            cid: 7,
            node: Some("n3".into()),
        };
        let refusal = upstream_for(&providers, "anthropic", &caller).err();
        assert_eq!(refusal.as_ref().map(|(status, _)| *status), Some(503));
        let message = refusal.unwrap().1;
        assert!(
            message.starts_with("vault tampered: ") && message.contains("index MAC"),
            "{message}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_agent_off_the_nodes_holding_the_key_is_refused_403_by_the_upstream_call() {
        use crate::vault::Scope;
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("apid-llm-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vault = anthropic_key_in(
            &dir,
            Scope::Nodes {
                nodes: vec!["n3".into()],
            },
        );
        let providers = crate::providers::Store::load(&dir, Arc::clone(&vault)).unwrap();
        let elsewhere = Caller::Agent {
            cid: 7,
            node: Some("n4".into()),
        };
        assert_eq!(
            upstream_for(&providers, "anthropic", &elsewhere),
            Err((403, "that secret is not scoped to this node".to_string()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn allowed(models: &[&str]) -> Vec<String> {
        models.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn a_listed_model_passes_untouched_and_effort_suffixes_do_not_matter() {
        let list = allowed(&[
            "anthropic/claude-haiku-5-5",
            "anthropic/claude-opus-5-5@low",
        ]);
        assert_eq!(
            decide_model(
                "anthropic",
                "claude-haiku-5-5",
                "anthropic/claude-haiku-5-5",
                &list
            ),
            Decision::Allow
        );
        assert_eq!(
            decide_model(
                "anthropic",
                "claude-opus-5-5",
                "anthropic/claude-haiku-5-5",
                &list
            ),
            Decision::Allow
        );
    }

    #[test]
    fn workers_are_not_held_to_a_model_list_and_the_orchestrator_is() {
        let policy = ModelPolicy::default();
        // Workers may run any model: no list is handed to the relay for them, dev models neither.
        assert_eq!(held_list(false, Tier::Worker, &policy), None);
        assert_eq!(held_list(true, Tier::Orchestrator, &policy), None);
        // The orchestrator is held to its own list, fail closed.
        assert_eq!(
            held_list(false, Tier::Orchestrator, &policy),
            Some(policy.orchestrator.clone())
        );
        let list = held_list(false, Tier::Orchestrator, &policy).unwrap();
        assert_eq!(
            decide_model(
                "anthropic",
                "claude-fable-5-1",
                "anthropic/claude-opus-5-5",
                &list
            ),
            Decision::Allow
        );
        assert!(matches!(
            decide_model(
                "deepseek",
                "deepseek-flash",
                "anthropic/claude-opus-5-5",
                &list
            ),
            Decision::Refuse(_)
        ));
    }

    #[test]
    fn an_unlisted_model_is_rewritten_to_the_agents_own_on_the_same_provider() {
        let list = allowed(&["anthropic/claude-haiku-5-5", "openai/gpt-6-luna"]);
        assert_eq!(
            decide_model(
                "anthropic",
                "claude-fable-5-1",
                "anthropic/claude-haiku-5-5@low",
                &list
            ),
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
        let Decision::Refuse(message) =
            decide_model("openai", "gpt-9", "anthropic/claude-haiku-5-5", &list)
        else {
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
        assert_eq!(
            upstream_url("https://api.anthropic.com", "/v1/messages"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            upstream_url("https://api.anthropic.com/", "/v1/messages"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            upstream_url("https://api.openai.com/v1", "/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            upstream_url("https://api.openai.com/v1/", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            ws_base("https://api.openai.com/v1/"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn an_openrouter_model_is_policed_as_its_provider_prefixed_id() {
        let own = "openrouter/anthropic/claude-y";
        assert_eq!(
            decide_model(
                "openrouter",
                "anthropic/claude-x",
                own,
                &allowed(&["openrouter/anthropic/claude-x"])
            ),
            Decision::Allow
        );
        assert_eq!(
            decide_model(
                "openrouter",
                "anthropic/claude-x",
                own,
                &allowed(&["openrouter/anthropic/claude-z"])
            ),
            Decision::Rewrite("anthropic/claude-y".into())
        );
        assert!(matches!(
            decide_model(
                "openrouter",
                "anthropic/claude-x",
                "anthropic/claude-y",
                &allowed(&["openrouter/anthropic/claude-z"])
            ),
            Decision::Refuse(_)
        ));
    }

    #[test]
    fn opencode_calls_go_to_the_endpoint_their_model_is_served_on() {
        assert_eq!(opencode_endpoint_error("/v1/messages", "claude-x"), None);
        assert!(opencode_endpoint_error("/v1/messages", "gpt-5.6-terra").is_some());
        assert!(opencode_endpoint_error("/v1/messages", "deepseek-v4-pro").is_some());
        assert_eq!(
            opencode_endpoint_error("/v1/responses", "gpt-5.6-terra"),
            None
        );
        assert!(opencode_endpoint_error("/v1/responses", "claude-x").is_some());
        assert_eq!(
            opencode_endpoint_error("/v1/chat/completions", "deepseek-v4-pro"),
            None
        );
        assert!(opencode_endpoint_error("/v1/chat/completions", "claude-x").is_some());
        assert!(opencode_endpoint_error("/v1/chat/completions", "gemini-3-flash").is_some());
    }

    /// Reads one HTTP request: its head, and a body as long as its Content-Length says.
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        use std::io::Read;
        let mut seen = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&seen).to_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text[..end]
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if seen.len() >= end + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&seen).to_string()
    }

    fn metered(
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost_usd: Option<f64>,
    ) -> Metered {
        Metered {
            tokens: Tokens {
                input,
                output,
                cache_read,
                cache_write,
            },
            cost_usd,
        }
    }

    #[test]
    fn an_anthropic_stream_meters_its_cache_reads_and_writes_outside_its_input() {
        let tail = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":300,\"cache_creation_input_tokens\":40,\"output_tokens\":1}}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":15}}\n\n";
        // Anthropic's input_tokens already excludes its cache reads and writes.
        assert_eq!(tokens_in_sse(tail), Some(metered(25, 15, 300, 40, None)));
    }

    #[test]
    fn openai_chat_stream_usage_chunk_meters_cached_tokens() {
        let tail = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":2000,\"completion_tokens\":9,\"prompt_tokens_details\":{\"cached_tokens\":1920}}}\n\n\
data: [DONE]\n\n";
        assert_eq!(tokens_in_sse(tail), Some(metered(80, 9, 1920, 0, None)));
    }

    #[test]
    fn openrouter_reports_the_cost_of_a_call_in_usd() {
        let body =
            json!({ "usage": { "prompt_tokens": 10, "completion_tokens": 2, "cost": 0.00042 } });
        assert_eq!(tokens_in(&body), Some(metered(10, 2, 0, 0, Some(0.00042))));
        let tail = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"cost\":0.5}}\n\n";
        assert_eq!(tokens_in_sse(tail), Some(metered(10, 2, 0, 0, Some(0.5))));
    }

    #[test]
    fn a_plain_get_to_a_socket_path_is_still_the_405_it_was() {
        let plain = Request {
            method: "GET".into(),
            path: "/internal/v1/agents/1/gateway/openai/v1/responses".into(),
            ..Default::default()
        };
        assert!(!is_upgrade(&plain));
        let upgrade = Request {
            headers: vec![("upgrade".into(), "websocket".into())],
            ..plain.clone()
        };
        assert!(is_upgrade(&upgrade));
        let refused = not_upgrade();
        assert_eq!(refused.status, 405);
        assert!(
            refused
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("allow") && value == "POST")
        );
    }

    #[test]
    fn a_client_frame_that_is_not_the_frame_its_socket_takes_is_refused() {
        assert!(
            check_create(r#"{"type":"response.create","model":"gpt-6-luna","input":"hi"}"#).is_ok()
        );
        assert_eq!(
            check_create(r#"{"type":"session.update"}"#).unwrap_err(),
            "the only frame a client sends is response.create"
        );
        assert!(check_create("not json").is_err());
        assert!(check_messages_create(r#"{"type":"response.create","request":{}}"#).is_err());
        assert!(check_messages_create(r#"{"type":"messages.create","id":"a"}"#).is_err());
        let (id, request) =
            check_messages_create(r#"{"type":"messages.create","id":"a","request":{"model":"m"}}"#)
                .unwrap();
        assert_eq!((id, request), (json!("a"), json!({ "model": "m" })));
    }

    #[test]
    fn one_sse_event_becomes_one_bridge_frame() {
        let frame = sse_frame(
            &json!("c1"),
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0}"#,
        );
        assert_eq!(
            frame,
            json!({ "id": "c1", "event": "content_block_delta", "data": { "type": "content_block_delta", "index": 0 } })
        );
        assert_eq!(
            sse_frame(&json!(7), "ping", "not json")["data"],
            json!("not json")
        );
    }

    use apz_plaza::treasury::Treasury;

    const PAYER: u64 = 7;

    /// A treasury where agent `PAYER` has `wallet` credits (paid from the task "job", whose
    /// escrow keeps 1000 credits), built as the sanctions tests build one.
    fn funded(wallet: i64) -> Treasury {
        let treasury = Treasury::new(apz_plaza::events::EventBus::new());
        if wallet > 0 {
            // The treasury's margin rule: pay at most a fifth of the task's real cost.
            treasury
                .fund_task("job", 6 * wallet + 1000)
                .expect("the task is funded");
            treasury
                .charge_spend(Some("job"), 5 * wallet, "real cost")
                .expect("the cost is spent");
            treasury
                .pay_agent("job", PAYER, wallet)
                .expect("the agent is paid");
        }
        treasury
    }

    #[test]
    fn a_broke_agent_is_refused_and_a_funded_one_passes() {
        let message = refusal(true, None, 0, 0).expect("no wallet, no task: refused");
        assert!(message.starts_with("cannot pay for this model call"));
        assert!(
            refusal(true, Some("job"), 0, 0)
                .expect("an empty escrow and no wallet: refused")
                .contains("escrow has 0")
        );
        assert_eq!(refusal(true, None, 0, 25), None);
        assert_eq!(
            refusal(true, Some("job"), 40, 0),
            None,
            "the task's escrow pays"
        );
        assert_eq!(
            refusal(true, Some("job"), 0, 25),
            None,
            "an empty escrow falls back to the wallet"
        );
    }

    #[test]
    fn charging_off_never_refuses_and_charges_nothing() {
        assert_eq!(refusal(false, None, 0, 0), None);
        let treasury = funded(100);
        assert_eq!(
            charge_model_call(&treasury, false, |_, _| {}, PAYER, None, 50, "m"),
            None
        );
        assert_eq!(treasury.wallet(PAYER), 100);
    }

    #[test]
    fn the_settings_default_charges_models() {
        assert!(crate::settings::Settings::default().payments.charge_models);
    }

    #[test]
    fn credits_are_the_cost_over_usd_per_credit_rounded_up() {
        assert_eq!(
            credits_for(Some(0.05), || 1.0),
            50,
            "reported cost wins over the estimate"
        );
        assert_eq!(
            credits_for(Some(0.0011), || 0.0),
            2,
            "a fraction of a credit rounds up"
        );
        assert_eq!(
            credits_for(None, || 0.003),
            3,
            "the estimate is used without a reported cost"
        );
        assert_eq!(
            credits_for(Some(0.0), || 1.0),
            0,
            "a zero cost is not charged"
        );
        assert_eq!(
            credits_for(None, || 0.0),
            0,
            "an unpriced call is not charged"
        );
    }

    #[test]
    fn a_call_is_charged_its_credits_from_the_wallet() {
        let treasury = funded(100);
        let settled = crate::payments::settle(&treasury, PAYER, None, 50, "model call");
        assert_eq!((settled.wanted, settled.charged), (50, 50));
        assert_eq!(settled.error, None);
        assert_eq!(treasury.wallet(PAYER), 50);
    }

    #[test]
    fn a_charge_larger_than_the_balance_is_capped_and_never_negative() {
        let treasury = funded(30);
        let settled = charge_model_call(&treasury, true, |_, _| {}, PAYER, None, 50, "model call")
            .expect("short, so noted");
        assert_eq!(
            (settled.wanted, settled.charged, settled.available),
            (50, 30, 30)
        );
        assert_eq!(treasury.wallet(PAYER), 0);
        // A broke agent is charged nothing, and the call still goes through.
        let broke = funded(0);
        let settled = charge_model_call(&broke, true, |_, _| {}, PAYER, None, 5, "model call")
            .expect("short, so noted");
        assert_eq!((settled.charged, settled.available), (0, 0));
        assert_eq!(broke.wallet(PAYER), 0);
    }

    #[test]
    fn a_task_escrow_pays_first_and_is_capped_there() {
        let treasury = funded(30);
        // The task keeps 1000 in escrow: a 1200 charge is capped there, the wallet is untouched.
        let settled = crate::payments::settle(&treasury, PAYER, Some("job"), 1200, "model call");
        assert_eq!((settled.charged, settled.available), (1000, 1000));
        assert_eq!(treasury.escrow_balance("job"), 0);
        assert_eq!(treasury.wallet(PAYER), 30);
        // With the escrow empty, the wallet pays.
        let settled = crate::payments::settle(&treasury, PAYER, Some("job"), 10, "model call");
        assert_eq!((settled.charged, settled.available), (10, 30));
        assert_eq!(treasury.wallet(PAYER), 20);
    }

    /// What a mock upstream saw of one call: the request line and the whole head, lowercased.
    struct Recorded {
        line: String,
        head: String,
    }

    /// A local upstream on 127.0.0.1 that reads one request, records it and answers 200 with `json`. The
    /// base is the server's root; the receiver yields what it saw once the request has arrived.
    fn spawn_mock(json: &'static str) -> (String, std::sync::mpsc::Receiver<Recorded>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let head = read_request(&mut stream).to_lowercase();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()
            );
            std::io::Write::write_all(&mut stream, reply.as_bytes()).unwrap();
            let line = head.lines().next().unwrap_or_default().to_string();
            tx.send(Recorded { line, head }).unwrap();
        });
        (root, rx)
    }

    /// The attribution headers the OpenRouter preset sends with every call.
    fn openrouter_attribution() -> Vec<(String, String)> {
        vec![
            (
                "HTTP-Referer".to_string(),
                "https://agentplaza.io".to_string(),
            ),
            ("X-OpenRouter-Title".to_string(), "AgentPlaza".to_string()),
        ]
    }

    /// POSTs a call for `model` to `path` through `upstream_post`, with a provider base of a fresh mock's root
    /// plus `prefix`. Returns what the mock saw; the call must have been answered 200.
    fn post_to_mock(
        provider: &str,
        prefix: &str,
        key: &str,
        path: &str,
        headers: &[(String, String)],
        model: &str,
    ) -> Recorded {
        let (root, rx) = spawn_mock(r#"{"id":"c","choices":[]}"#);
        let body = json!({ "model": model, "max_tokens": 1, "messages": [] });
        let resp = upstream_post(
            provider,
            &format!("{root}{prefix}"),
            key,
            path,
            headers,
            &body,
        )
        .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        rx.recv().unwrap()
    }

    #[test]
    fn each_upstream_route_sends_its_own_auth_and_attribution_headers() {
        let version = || ("anthropic-version".to_string(), "2023-06-01".to_string());
        let with_version = |mut headers: Vec<(String, String)>| {
            headers.push(version());
            headers
        };
        // (provider, base prefix, path, headers, model, x-api-key sent, attribution sent)
        let cases = [
            (
                "openrouter",
                "/api/v1",
                "/v1/chat/completions",
                openrouter_attribution(),
                "openai/gpt-x",
                false,
                true,
            ),
            (
                "openrouter",
                "/api/v1",
                "/v1/messages",
                with_version(openrouter_attribution()),
                "anthropic/claude-sonnet-5-5",
                true,
                true,
            ),
            (
                "opencode",
                "/inference/v1",
                "/v1/chat/completions",
                vec![],
                "deepseek-v4-pro",
                false,
                false,
            ),
            (
                "opencode",
                "/inference/v1",
                "/v1/responses",
                vec![],
                "gpt-5.6-terra",
                false,
                false,
            ),
            (
                "opencode",
                "/inference/v1",
                "/v1/messages",
                vec![version()],
                "claude-sonnet-5-5",
                true,
                false,
            ),
        ];
        for (provider, prefix, path, headers, model, x_api_key, attribution) in cases {
            let key = format!("test-key-{provider}");
            let seen = post_to_mock(provider, prefix, &key, path, &headers, model);
            assert_eq!(
                seen.line,
                format!("post {prefix}{} http/1.1", path.trim_start_matches("/v1")),
                "{path}"
            );
            assert!(
                seen.head
                    .contains(&format!("authorization: bearer {key}\r\n")),
                "{path}: {}",
                seen.head
            );
            if x_api_key {
                assert!(
                    seen.head.contains(&format!("x-api-key: {key}\r\n")),
                    "{path}: {}",
                    seen.head
                );
            } else {
                assert!(!seen.head.contains("x-api-key:"), "{path}: {}", seen.head);
            }
            if attribution {
                assert!(
                    seen.head
                        .contains("http-referer: https://agentplaza.io\r\n"),
                    "{path}: {}",
                    seen.head
                );
                assert!(
                    seen.head.contains("x-openrouter-title: agentplaza\r\n"),
                    "{path}: {}",
                    seen.head
                );
            } else {
                assert!(
                    !seen.head.contains("http-referer:"),
                    "{path}: {}",
                    seen.head
                );
            }
        }
    }

    #[test]
    fn opencode_refusals_name_the_endpoint_the_model_is_served_on() {
        // (endpoint called, model, the endpoint the refusal names, an endpoint it must not name)
        let cases = [
            (
                "/v1/messages",
                "deepseek-v4-pro",
                "/v1/chat/completions",
                Some("/v1/responses"),
            ),
            ("/v1/responses", "claude-sonnet-5-5", "/v1/messages", None),
            ("/v1/messages", "gpt-5.6-terra", "/v1/responses", None),
        ];
        for (called, model, names, not) in cases {
            let message = opencode_endpoint_error(called, model).expect("refused");
            assert!(message.contains(names), "{message}");
            assert!(message.contains(model), "{message}");
            if let Some(other) = not {
                assert!(!message.contains(other), "{message}");
            }
        }
    }
    #[test]
    fn buffered_bodies_meter_each_providers_usage_shape() {
        // (body, the usage it meters): input excludes the cached tokens in every shape.
        let cases = [
            (
                json!({ "id": "c", "choices": [], "usage": { "prompt_tokens": 12, "completion_tokens": 7, "total_tokens": 19 } }),
                metered(12, 7, 0, 0, None),
            ),
            (
                json!({ "type": "message", "usage": { "input_tokens": 30, "output_tokens": 4 } }),
                metered(30, 4, 0, 0, None),
            ),
            (
                json!({
                    "choices": [],
                    "usage": {
                        "prompt_tokens": 100,
                        "completion_tokens": 7,
                        "prompt_tokens_details": { "cached_tokens": 80 },
                    },
                }),
                metered(20, 7, 80, 0, None),
            ),
            (
                json!({
                    "type": "response.completed",
                    "response": { "usage": { "input_tokens": 500, "input_tokens_details": { "cached_tokens": 420 }, "output_tokens": 9 } },
                }),
                metered(80, 9, 420, 0, None),
            ),
            (
                json!({ "usage": { "prompt_tokens": 100, "completion_tokens": 5, "prompt_cache_hit_tokens": 70, "prompt_cache_miss_tokens": 30 } }),
                metered(30, 5, 70, 0, None),
            ),
            // Without a miss count, the input is the prompt less its cache hits.
            (
                json!({ "usage": { "prompt_tokens": 100, "completion_tokens": 5, "prompt_cache_hit_tokens": 70 } }),
                metered(30, 5, 70, 0, None),
            ),
            // Cached tokens above the prompt do not underflow the input.
            (
                json!({ "usage": { "prompt_tokens": 5, "prompt_tokens_details": { "cached_tokens": 9 } } }),
                metered(0, 0, 9, 0, None),
            ),
        ];
        for (body, want) in cases {
            assert_eq!(tokens_in(&body), Some(want), "{body}");
        }
        assert_eq!(tokens_in(&json!({ "id": "x" })), None);
    }
}
