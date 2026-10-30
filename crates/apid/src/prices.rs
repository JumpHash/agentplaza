//! Model prices: what one million tokens of each model costs, so the plaza can show what its
//! agents spend. The catalog is fetched from two public lists and kept in plaza.db ("prices"),
//! so a hub without internet still has the last prices it saw:
//!
//! - OpenRouter's model list (`https://openrouter.ai/api/v1/models`): every OpenRouter model id
//!   (`provider/model`), with prompt, completion, cache-read and cache-write prices per token.
//! - LiteLLM's price table (`model_prices_and_context_window.json` in BerriAI/litellm): the
//!   providers' own model ids (Anthropic, OpenAI, DeepSeek, …), with per-token input, output,
//!   cache-read (`cache_read_input_token_cost`) and cache-write (`cache_creation_input_token_cost`).
//!
//! The owner's manual prices (`settings.prices`, US dollars per million tokens) win over both.
//! Lookup order for a plaza model id such as `anthropic/claude-opus-5-5`: manual price, then the
//! built-in vendor prices ([`VENDOR`], Meta's Muse models), then the OpenRouter id as is, then
//! LiteLLM by the full id, then vendor prices and LiteLLM by the part after the first `/`.
//! The last step retains an upstream's price when a named transport instance prefixes its id,
//! such as `local-glm/opencode/glm-5.3`. Ids compare case-insensitively.
//!
//! GET  /v1/prices          → {fetched_at_ms, sources: [{name, url, models, ok, error?}], models: [{model, price}]}
//! GET  /v1/prices/{model}  → {model, price} (404 when nothing prices it); `{model}` is percent-encoded
//! POST /v1/prices/refresh  → the same as GET /v1/prices, after fetching both lists again
//!
//! The catalog refreshes when apid starts (if older than a day) and once a day after.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

use apz_http::{Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::App;

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";
pub const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
/// How old the catalog may get before it is fetched again.
pub const MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;
/// A catalog with a failed list is fetched again after this long instead of [`MAX_AGE_MS`].
const RETRY_MS: u64 = 60 * 60 * 1000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// The lists are a few megabytes; ureq's default 10 MB reader limit leaves too little room.
const BODY_LIMIT: u64 = 64 * 1024 * 1024;
/// LiteLLM providers whose bare model ids also get a `<provider>/<id>` alias.
const ALIAS_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "deepseek",
    "gemini",
    "xai",
    "mistral",
];

/// One model's prices in US dollars per million tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    /// Reading cached input; `None` when the source does not say (then `input` is used).
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Writing input to the cache; `None` when the source does not say (then `input` is used).
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// Where a price came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Manual,
    /// A provider's own published price list, built into apid (see [`VENDOR`]).
    Vendor,
    Openrouter,
    Litellm,
}

/// Providers' own published prices, for models the public lists do not carry yet. Keys are plaza
/// ids (`provider/model`, no effort). Meta's Muse models (dev.meta.ai/docs/pricing-rate-limits):
/// the contributor variants cost less because Meta trains on what they send and return.
pub const VENDOR: &[(&str, ModelPrice)] = &[
    (
        "meta/muse-wonder-1.0",
        ModelPrice {
            input: 4.0,
            output: 20.0,
            cache_read: Some(0.2),
            cache_write: None,
        },
    ),
    (
        "meta/muse-spark-1.3",
        ModelPrice {
            input: 1.25,
            output: 4.25,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "meta/muse-spark-1.2",
        ModelPrice {
            input: 1.25,
            output: 4.25,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "meta/muse-spark-1.1",
        ModelPrice {
            input: 1.25,
            output: 4.25,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "meta/muse-spark-1.3-contributor",
        ModelPrice {
            input: 0.10,
            output: 0.20,
            cache_read: Some(0.002),
            cache_write: None,
        },
    ),
    (
        "meta/muse-spark-1.2-contributor",
        ModelPrice {
            input: 0.10,
            output: 0.20,
            cache_read: Some(0.002),
            cache_write: None,
        },
    ),
    // OpenCode Zen (opencode.ai/v2/docs/console/models): pay-as-you-go list prices per 1M tokens,
    // base tier only (the first tier of tiered models). Mistral Large 4 is the displayed 50%-off
    // price ($0.68 / $2.09 / $0.07), not the struck-through list price.
    (
        "opencode/big-pickle",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/space-bunny-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/cat-jam-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/longcat-2.5-preview-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/step-5-preview-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/exo-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/mimo-v2.6-flash-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/mimo-v2.5-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/ling-3.1-flash-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/ling-3.0-flash-fin-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/nemotron-3-ultra-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/nemotron-3.5-lightning-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/muse-spark-1.3-contributor-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: None,
        },
    ),
    (
        "opencode/jev-1.13-free",
        ModelPrice {
            input: 0.0,
            output: 0.0,
            cache_read: None,
            cache_write: None,
        },
    ),
    (
        "opencode/jev-1.13",
        ModelPrice {
            input: 0.042,
            output: 0.0,
            cache_read: None,
            cache_write: None,
        },
    ),
    (
        "opencode/minimax-m3",
        ModelPrice {
            input: 0.30,
            output: 1.20,
            cache_read: Some(0.06),
            cache_write: None,
        },
    ),
    (
        "opencode/minimax-m2.7",
        ModelPrice {
            input: 0.30,
            output: 1.20,
            cache_read: Some(0.06),
            cache_write: None,
        },
    ),
    (
        "opencode/minimax-m2.5",
        ModelPrice {
            input: 0.30,
            output: 1.20,
            cache_read: Some(0.06),
            cache_write: None,
        },
    ),
    (
        "opencode/glm-5.3-flash",
        ModelPrice {
            input: 0.15,
            output: 0.50,
            cache_read: Some(0.03),
            cache_write: None,
        },
    ),
    (
        "opencode/glm-5.3",
        ModelPrice {
            input: 1.40,
            output: 4.40,
            cache_read: Some(0.26),
            cache_write: None,
        },
    ),
    (
        "opencode/glm-5.2",
        ModelPrice {
            input: 1.40,
            output: 4.40,
            cache_read: Some(0.26),
            cache_write: None,
        },
    ),
    (
        "opencode/glm-5.1",
        ModelPrice {
            input: 1.40,
            output: 4.40,
            cache_read: Some(0.26),
            cache_write: None,
        },
    ),
    (
        "opencode/glm-5",
        ModelPrice {
            input: 1.00,
            output: 3.20,
            cache_read: Some(0.20),
            cache_write: None,
        },
    ),
    (
        "opencode/kimi-k2.7-code",
        ModelPrice {
            input: 0.95,
            output: 4.00,
            cache_read: Some(0.19),
            cache_write: None,
        },
    ),
    (
        "opencode/kimi-k3",
        ModelPrice {
            input: 3.00,
            output: 15.00,
            cache_read: Some(0.30),
            cache_write: None,
        },
    ),
    (
        "opencode/kimi-k2.6",
        ModelPrice {
            input: 0.95,
            output: 4.00,
            cache_read: Some(0.16),
            cache_write: None,
        },
    ),
    (
        "opencode/kimi-k2.5",
        ModelPrice {
            input: 0.60,
            output: 3.00,
            cache_read: Some(0.10),
            cache_write: None,
        },
    ),
    (
        "opencode/mistral-large-4",
        ModelPrice {
            input: 0.68,
            output: 2.09,
            cache_read: Some(0.07),
            cache_write: None,
        },
    ),
    (
        "opencode/qwen3.8-max",
        ModelPrice {
            input: 2.00,
            output: 6.00,
            cache_read: Some(0.25),
            cache_write: Some(2.50),
        },
    ),
    (
        "opencode/qwen3.8-flash",
        ModelPrice {
            input: 0.15,
            output: 0.47,
            cache_read: Some(0.016),
            cache_write: Some(0.20),
        },
    ),
    (
        "opencode/qwen3.7-max",
        ModelPrice {
            input: 2.50,
            output: 7.50,
            cache_read: Some(0.50),
            cache_write: Some(3.125),
        },
    ),
    (
        "opencode/qwen3.7-plus",
        ModelPrice {
            input: 0.40,
            output: 1.60,
            cache_read: Some(0.04),
            cache_write: Some(0.50),
        },
    ),
    (
        "opencode/qwen3.6-plus",
        ModelPrice {
            input: 0.50,
            output: 3.00,
            cache_read: Some(0.05),
            cache_write: Some(0.625),
        },
    ),
    (
        "opencode/qwen3.5-plus",
        ModelPrice {
            input: 0.20,
            output: 1.20,
            cache_read: Some(0.02),
            cache_write: Some(0.25),
        },
    ),
    (
        "opencode/deepseek-v4.1-flash",
        ModelPrice {
            input: 0.30,
            output: 1.20,
            cache_read: Some(0.006),
            cache_write: None,
        },
    ),
    (
        "opencode/deepseek-v4-pro",
        ModelPrice {
            input: 1.74,
            output: 3.48,
            cache_read: Some(0.145),
            cache_write: None,
        },
    ),
    (
        "opencode/deepseek-v4-flash",
        ModelPrice {
            input: 0.14,
            output: 0.28,
            cache_read: Some(0.028),
            cache_write: None,
        },
    ),
    (
        "opencode/deepseek-v4-flash-vision-exp",
        ModelPrice {
            input: 0.14,
            output: 0.28,
            cache_read: Some(0.028),
            cache_write: None,
        },
    ),
    (
        "opencode/claude-fable-5-1",
        ModelPrice {
            input: 10.0,
            output: 50.0,
            cache_read: Some(0.25),
            cache_write: Some(12.50),
        },
    ),
    (
        "opencode/claude-fable-5",
        ModelPrice {
            input: 10.0,
            output: 50.0,
            cache_read: Some(1.00),
            cache_write: Some(12.50),
        },
    ),
    (
        "opencode/claude-opus-5-5",
        ModelPrice {
            input: 4.0,
            output: 20.0,
            cache_read: Some(0.20),
            cache_write: Some(5.00),
        },
    ),
    (
        "opencode/claude-opus-5",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_read: Some(0.50),
            cache_write: Some(6.25),
        },
    ),
    (
        "opencode/claude-opus-4-8",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_read: Some(0.50),
            cache_write: Some(6.25),
        },
    ),
    (
        "opencode/claude-opus-4-7",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_read: Some(0.50),
            cache_write: Some(6.25),
        },
    ),
    (
        "opencode/claude-opus-4-6",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_read: Some(0.50),
            cache_write: Some(6.25),
        },
    ),
    (
        "opencode/claude-opus-4-5",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_read: Some(0.50),
            cache_write: Some(6.25),
        },
    ),
    (
        "opencode/claude-sonnet-5-5",
        ModelPrice {
            input: 2.00,
            output: 10.00,
            cache_read: Some(0.20),
            cache_write: Some(2.50),
        },
    ),
    (
        "opencode/claude-sonnet-5",
        ModelPrice {
            input: 2.00,
            output: 10.00,
            cache_read: Some(0.20),
            cache_write: Some(2.50),
        },
    ),
    (
        "opencode/claude-sonnet-4-6",
        ModelPrice {
            input: 3.00,
            output: 15.00,
            cache_read: Some(0.30),
            cache_write: Some(3.75),
        },
    ),
    (
        "opencode/claude-sonnet-4-5",
        ModelPrice {
            input: 3.00,
            output: 15.00,
            cache_read: Some(0.30),
            cache_write: Some(3.75),
        },
    ),
    (
        "opencode/claude-haiku-5-5",
        ModelPrice {
            input: 0.10,
            output: 0.50,
            cache_read: Some(0.01),
            cache_write: Some(0.125),
        },
    ),
    (
        "opencode/claude-haiku-4-5",
        ModelPrice {
            input: 1.00,
            output: 5.00,
            cache_read: Some(0.10),
            cache_write: Some(1.25),
        },
    ),
    (
        "opencode/gemini-3.8-flash",
        ModelPrice {
            input: 1.50,
            output: 7.50,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "opencode/gemini-3.7-flash",
        ModelPrice {
            input: 1.50,
            output: 7.50,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "opencode/gemini-3.6-flash",
        ModelPrice {
            input: 1.50,
            output: 7.50,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "opencode/gemini-3.5-flash",
        ModelPrice {
            input: 1.50,
            output: 9.00,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "opencode/gemini-3.5-flash-lite",
        ModelPrice {
            input: 0.30,
            output: 2.50,
            cache_read: Some(0.03),
            cache_write: None,
        },
    ),
    (
        "opencode/gemini-3.1-pro",
        ModelPrice {
            input: 2.00,
            output: 12.00,
            cache_read: Some(0.20),
            cache_write: None,
        },
    ),
    (
        "opencode/gemini-3-flash",
        ModelPrice {
            input: 0.50,
            output: 3.00,
            cache_read: Some(0.05),
            cache_write: None,
        },
    ),
    (
        "opencode/grok-4.7",
        ModelPrice {
            input: 2.00,
            output: 6.00,
            cache_read: Some(0.50),
            cache_write: None,
        },
    ),
    (
        "opencode/grok-4.6",
        ModelPrice {
            input: 2.00,
            output: 6.00,
            cache_read: Some(0.50),
            cache_write: None,
        },
    ),
    (
        "opencode/grok-4.5",
        ModelPrice {
            input: 2.00,
            output: 6.00,
            cache_read: Some(0.30),
            cache_write: None,
        },
    ),
    (
        "opencode/grok-build-0.1",
        ModelPrice {
            input: 1.00,
            output: 2.00,
            cache_read: Some(0.20),
            cache_write: None,
        },
    ),
    (
        "opencode/muse-spark-1.3",
        ModelPrice {
            input: 1.25,
            output: 4.25,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "opencode/muse-spark-1.2",
        ModelPrice {
            input: 1.25,
            output: 4.25,
            cache_read: Some(0.15),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-6-astra",
        ModelPrice {
            input: 10.0,
            output: 50.0,
            cache_read: Some(1.00),
            cache_write: Some(12.50),
        },
    ),
    (
        "opencode/gpt-6-sol",
        ModelPrice {
            input: 2.00,
            output: 10.00,
            cache_read: Some(0.20),
            cache_write: Some(2.50),
        },
    ),
    (
        "opencode/gpt-6.1-sol",
        ModelPrice {
            input: 2.00,
            output: 10.00,
            cache_read: Some(0.20),
            cache_write: Some(2.50),
        },
    ),
    (
        "opencode/gpt-6-luna",
        ModelPrice {
            input: 0.10,
            output: 0.50,
            cache_read: Some(0.01),
            cache_write: Some(0.125),
        },
    ),
    (
        "opencode/gpt-5.6-sol",
        ModelPrice {
            input: 4.00,
            output: 20.00,
            cache_read: Some(0.40),
            cache_write: Some(5.00),
        },
    ),
    (
        "opencode/gpt-5.6-terra",
        ModelPrice {
            input: 2.00,
            output: 12.00,
            cache_read: Some(0.20),
            cache_write: Some(2.50),
        },
    ),
    (
        "opencode/gpt-5.6-luna",
        ModelPrice {
            input: 0.20,
            output: 1.20,
            cache_read: Some(0.02),
            cache_write: Some(0.25),
        },
    ),
    (
        "opencode/gpt-5.5",
        ModelPrice {
            input: 5.00,
            output: 30.00,
            cache_read: Some(0.50),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.5-pro",
        ModelPrice {
            input: 30.00,
            output: 180.00,
            cache_read: Some(30.00),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.4",
        ModelPrice {
            input: 2.50,
            output: 15.00,
            cache_read: Some(0.25),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.4-pro",
        ModelPrice {
            input: 30.00,
            output: 180.00,
            cache_read: Some(30.00),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.4-mini",
        ModelPrice {
            input: 0.75,
            output: 4.50,
            cache_read: Some(0.075),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.4-nano",
        ModelPrice {
            input: 0.20,
            output: 1.25,
            cache_read: Some(0.02),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.3-codex-spark",
        ModelPrice {
            input: 1.75,
            output: 14.00,
            cache_read: Some(0.175),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.3-codex",
        ModelPrice {
            input: 1.75,
            output: 14.00,
            cache_read: Some(0.175),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.2",
        ModelPrice {
            input: 1.75,
            output: 14.00,
            cache_read: Some(0.175),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.2-codex",
        ModelPrice {
            input: 1.75,
            output: 14.00,
            cache_read: Some(0.175),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.1",
        ModelPrice {
            input: 1.07,
            output: 8.50,
            cache_read: Some(0.107),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.1-codex",
        ModelPrice {
            input: 1.07,
            output: 8.50,
            cache_read: Some(0.107),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.1-codex-max",
        ModelPrice {
            input: 1.25,
            output: 10.00,
            cache_read: Some(0.125),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5.1-codex-mini",
        ModelPrice {
            input: 0.25,
            output: 2.00,
            cache_read: Some(0.025),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5",
        ModelPrice {
            input: 1.07,
            output: 8.50,
            cache_read: Some(0.107),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5-codex",
        ModelPrice {
            input: 1.07,
            output: 8.50,
            cache_read: Some(0.107),
            cache_write: None,
        },
    ),
    (
        "opencode/gpt-5-nano",
        ModelPrice {
            input: 0.05,
            output: 0.40,
            cache_read: Some(0.005),
            cache_write: None,
        },
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Priced {
    #[serde(flatten)]
    pub price: ModelPrice,
    pub source: Source,
}

/// What one fetch of one list did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct SourceStatus {
    pub name: String,
    pub url: String,
    pub models: usize,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The fetched lists, keyed by lower-cased model id: one row of `price_catalog`, its models and sources as children.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "price_catalog")]
pub struct CatalogState {
    pub fetched_at_ms: u64,
    #[table(map)]
    pub openrouter: BTreeMap<String, ModelPrice>,
    #[table(map)]
    pub litellm: BTreeMap<String, ModelPrice>,
    #[table(child)]
    pub sources: Vec<SourceStatus>,
}

/// Token counts of one call, as the gateway and the proxy meter them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Tokens {
    /// Input tokens billed at the full input price (cache reads and writes excluded).
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Default)]
pub struct Catalog {
    state: RwLock<CatalogState>,
    /// Held while a refresh fetches, so two refreshes never run at once.
    fetching: Mutex<()>,
}

impl Catalog {
    /// The price of `model`: `manual` (settings.prices: dollars per million tokens) first, then the
    /// fetched lists in the order the module docs give.
    pub fn lookup(
        &self,
        model: &str,
        manual: &BTreeMap<String, crate::settings::Price>,
    ) -> Option<Priced> {
        let id = strip_effort(model.trim()).to_lowercase();
        if let Some((_, price)) = manual
            .iter()
            .find(|(key, _)| key.trim().to_lowercase() == id)
        {
            return Some(Priced {
                price: ModelPrice {
                    input: price.input,
                    output: price.output,
                    cache_read: None,
                    cache_write: None,
                },
                source: Source::Manual,
            });
        }
        if let Some((_, price)) = VENDOR.iter().find(|(key, _)| *key == id) {
            return Some(Priced {
                price: *price,
                source: Source::Vendor,
            });
        }
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        // `openrouter/<vendor>/<model>`: the rest is OpenRouter's own id. Never falls through to LiteLLM.
        if let Some(rest) = id.strip_prefix("openrouter/") {
            return state.openrouter.get(rest).map(|price| Priced {
                price: *price,
                source: Source::Openrouter,
            });
        }
        if let Some(price) = state.openrouter.get(&id) {
            return Some(Priced {
                price: *price,
                source: Source::Openrouter,
            });
        }
        if let Some(price) = state.litellm.get(&id) {
            return Some(Priced {
                price: *price,
                source: Source::Litellm,
            });
        }
        let (_, upstream) = id.split_once('/')?;
        if let Some((_, price)) = VENDOR.iter().find(|(key, _)| *key == upstream) {
            return Some(Priced {
                price: *price,
                source: Source::Vendor,
            });
        }
        state.litellm.get(upstream).map(|price| Priced {
            price: *price,
            source: Source::Litellm,
        })
    }

    /// Fetches both lists (blocking, ~10 s timeout each) and replaces the catalog with what came
    /// back; a list that fails keeps its previous entries and reports the error in `sources`.
    /// `fetched_at_ms` moves on every call, failed or not.
    pub fn refresh(&self) -> CatalogState {
        let _turn = self.fetching.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = self.state();
        let (openrouter, openrouter_status) = fetch_list(
            "openrouter",
            OPENROUTER_URL,
            parse_openrouter,
            previous.openrouter,
        );
        let (litellm, litellm_status) =
            fetch_list("litellm", LITELLM_URL, parse_litellm, previous.litellm);
        let next = CatalogState {
            fetched_at_ms: apz_plaza::now_ms(),
            openrouter,
            litellm,
            sources: vec![openrouter_status, litellm_status],
        };
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = next.clone();
        next
    }

    /// Refreshes when the catalog is older than [`MAX_AGE_MS`], never fetched, or holds a failed
    /// list and is older than an hour (so a dead network is retried hourly, not hammered).
    pub fn refresh_if_stale(&self, now_ms: u64) {
        let state = self.state();
        let never = state.fetched_at_ms == 0;
        let age = now_ms.saturating_sub(state.fetched_at_ms);
        let failed = state.sources.iter().any(|source| !source.ok);
        if never || age > MAX_AGE_MS || (failed && age > RETRY_MS) {
            self.refresh();
        }
    }

    /// When the lists were fetched and how each fetch went, without copying the lists.
    pub fn fetched(&self) -> (u64, Vec<SourceStatus>) {
        self.state
            .read()
            .map(|s| (s.fetched_at_ms, s.sources.clone()))
            .unwrap_or_default()
    }

    pub fn state(&self) -> CatalogState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// The catalog as its rows (the saver writes what changed).
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        apz_store::RowSet::one(def(), &*state)
    }

    /// Replaces the catalog with what the store holds (at start); an empty store keeps the empty catalog.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        if let Some(stored) = apz_store::table::load_one::<CatalogState>(conn, def())? {
            *self.state.write().unwrap_or_else(PoisonError::into_inner) = stored;
        }
        Ok(())
    }
}

/// The catalog's tables (`price_catalog`).
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(CatalogState)
}

/// The estimated cost of `tokens` at `price`, in US dollars.
pub fn estimate(price: &ModelPrice, tokens: &Tokens) -> f64 {
    let cache_read = price.cache_read.unwrap_or(price.input);
    let cache_write = price.cache_write.unwrap_or(price.input);
    let cost = tokens.input as f64 * price.input
        + tokens.output as f64 * price.output
        + tokens.cache_read as f64 * cache_read
        + tokens.cache_write as f64 * cache_write;
    cost / 1e6
}

/// Parses OpenRouter's `GET /api/v1/models` body (`{data: [{id, pricing: {prompt, completion,
/// input_cache_read?, input_cache_write?}}]}`, prices are per-token decimal strings).
pub fn parse_openrouter(body: &str) -> Result<BTreeMap<String, ModelPrice>, String> {
    #[derive(Deserialize)]
    struct List {
        data: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        id: String,
        #[serde(default)]
        pricing: BTreeMap<String, Value>,
    }
    let list: List =
        serde_json::from_str(body).map_err(|e| format!("OpenRouter's model list: {e}"))?;
    let mut out = BTreeMap::new();
    for model in list.data {
        let id = model.id.trim().to_lowercase();
        let price = |key: &str| per_million(model.pricing.get(key));
        let (Some(input), Some(output)) = (price("prompt"), price("completion")) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let cache_read = price("input_cache_read");
        let cache_write = price("input_cache_write");
        out.insert(
            id,
            ModelPrice {
                input,
                output,
                cache_read,
                cache_write,
            },
        );
    }
    Ok(out)
}

/// Parses LiteLLM's price table (`{"<model id>": {input_cost_per_token, output_cost_per_token,
/// cache_read_input_token_cost?, cache_creation_input_token_cost?, mode, ...}}`); only
/// `mode: "chat"` (or `"responses"`) entries with both base prices are kept; the `sample_spec`
/// key is skipped.
pub fn parse_litellm(body: &str) -> Result<BTreeMap<String, ModelPrice>, String> {
    let models: BTreeMap<String, Value> =
        serde_json::from_str(body).map_err(|e| format!("LiteLLM's price table: {e}"))?;
    let mut out = BTreeMap::new();
    let mut aliases = Vec::new();
    for (id, entry) in &models {
        if id == "sample_spec" {
            continue;
        }
        let mode = entry.get("mode").and_then(Value::as_str);
        if !matches!(mode, None | Some("chat") | Some("responses")) {
            continue;
        }
        let field = |key: &str| per_million(entry.get(key));
        let (Some(input), Some(output)) = (
            field("input_cost_per_token"),
            field("output_cost_per_token"),
        ) else {
            continue;
        };
        let key = id.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        let price = ModelPrice {
            input,
            output,
            cache_read: field("cache_read_input_token_cost"),
            cache_write: field("cache_creation_input_token_cost"),
        };
        if !key.contains('/')
            && let Some(provider) = entry.get("litellm_provider").and_then(Value::as_str)
        {
            let provider = provider.to_lowercase();
            if ALIAS_PROVIDERS.contains(&provider.as_str()) {
                aliases.push((format!("{provider}/{key}"), price));
            }
        }
        out.insert(key, price);
    }
    // Aliases go in after every direct id, so an alias never overwrites a listed model.
    for (alias, price) in aliases {
        out.entry(alias).or_insert(price);
    }
    Ok(out)
}

/// A per-token price (a JSON string or number) as dollars per million tokens. `None` when it is
/// absent, unparseable, negative or not finite (OpenRouter's "-1" marks a variable price).
fn per_million(value: Option<&Value>) -> Option<f64> {
    let per_token = match value? {
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    if !per_token.is_finite() || per_token < 0.0 {
        return None;
    }
    // Rounded to a billionth of a dollar per million tokens, so 0.000003 reads as 3.
    Some((per_token * 1e15).round() / 1e9)
}

/// The model id without an `@effort` suffix: `openai/gpt-6@high` gives `openai/gpt-6`.
fn strip_effort(model: &str) -> &str {
    match model.rsplit_once('@') {
        Some((base, effort))
            if !base.is_empty()
                && !effort.is_empty()
                && effort
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
        {
            base
        }
        _ => model,
    }
}

/// One list's fetch: the parsed list, or the previous one when the fetch or parse failed (or
/// came back empty, which would otherwise wipe a good list), with the reason in the status.
fn fetch_list(
    name: &str,
    url: &str,
    parse: fn(&str) -> Result<BTreeMap<String, ModelPrice>, String>,
    previous: BTreeMap<String, ModelPrice>,
) -> (BTreeMap<String, ModelPrice>, SourceStatus) {
    let fetched = fetch(url).and_then(|body| parse(&body)).and_then(|list| {
        if list.is_empty() {
            Err("the list has no priced models".to_string())
        } else {
            Ok(list)
        }
    });
    match fetched {
        Ok(list) => {
            let status = SourceStatus {
                name: name.to_string(),
                url: url.to_string(),
                models: list.len(),
                ok: true,
                error: None,
            };
            (list, status)
        }
        Err(error) => {
            let status = SourceStatus {
                name: name.to_string(),
                url: url.to_string(),
                models: previous.len(),
                ok: false,
                error: Some(error),
            };
            (previous, status)
        }
    }
}

/// GET `url` with the apid user agent and a 10 s timeout; the body as text, or why it failed.
fn fetch(url: &str) -> Result<String, String> {
    let request = ureq::http::Request::builder()
        .method("GET")
        .uri(url)
        .header("user-agent", "agentplaza")
        .body(Vec::new())
        .map_err(|e| format!("bad request: {e}"))?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .run(request)
        .map_err(|e| format!("did not answer: {e}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("answered {status}"));
    }
    response
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()
        .map_err(|e| format!("reading the reply: {e}"))
}

/// A path segment with its `%XX` escapes decoded (`%2F` gives `/`); a malformed escape stays.
fn percent_decode_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What GET /v1/prices lists: OpenRouter's ids first, then the LiteLLM ids not already listed.
#[derive(Serialize)]
struct Listing {
    fetched_at_ms: u64,
    sources: Vec<SourceStatus>,
    models: Vec<Entry>,
}

#[derive(Serialize)]
struct Entry {
    model: String,
    price: Priced,
}

fn listing(state: CatalogState) -> Listing {
    let mut models: Vec<Entry> = state
        .openrouter
        .iter()
        .map(|(model, price)| Entry {
            model: model.clone(),
            price: Priced {
                price: *price,
                source: Source::Openrouter,
            },
        })
        .collect();
    models.extend(VENDOR.iter().map(|(model, price)| Entry {
        model: model.to_string(),
        price: Priced {
            price: *price,
            source: Source::Vendor,
        },
    }));
    models.extend(
        state
            .litellm
            .iter()
            .filter(|(model, _)| {
                !state.openrouter.contains_key(*model)
                    && !VENDOR.iter().any(|(v, _)| *v == model.as_str())
            })
            .map(|(model, price)| Entry {
                model: model.clone(),
                price: Priced {
                    price: *price,
                    source: Source::Litellm,
                },
            }),
    );
    Listing {
        fetched_at_ms: state.fetched_at_ms,
        sources: state.sources,
        models,
    }
}

/// What GET /v1/prices/{model} answers.
#[derive(Serialize)]
struct OneModel {
    model: String,
    price: Priced,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/prices", |app, _, _| {
            Response::json(200, &listing(app.prices.state()))
        })
        .doc(RouteDoc::new("List the model prices"))
        .get("/v1/prices/{model}", |app, _, p| {
            let model = percent_decode_path(p.get("model"));
            let manual = app.settings.get().prices;
            match app.prices.lookup(&model, &manual) {
                Some(price) => Response::json(200, &OneModel { model, price }),
                None => Response::error(404, &format!("no price for {model}")),
            }
        })
        .doc(RouteDoc::new("Show one model's price").errors(&[404]))
        .post("/v1/prices/refresh", |app, _, _| {
            app.prices.refresh();
            Response::json(200, &listing(app.prices.state()))
        })
        .doc(RouteDoc::new("Refresh the model prices from the public lists").outbound())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENROUTER: &str = r#"{"data": [
        {"id": "Anthropic/Claude-Opus-5-5", "pricing": {"prompt": "0.000015", "completion": "0.000075",
            "input_cache_read": "0.0000015", "input_cache_write": "0.00001875"}},
        {"id": "openrouter/auto", "pricing": {"prompt": "-1", "completion": "-1"}},
        {"id": "deepseek/deepseek-v4", "pricing": {"prompt": "0.00000027", "completion": "0.0000011"}}
    ]}"#;

    const LITELLM: &str = r#"{
        "sample_spec": {"input_cost_per_token": 0.0, "output_cost_per_token": 0.0, "mode": "chat"},
        "claude-opus-5-5": {"input_cost_per_token": 0.000015, "output_cost_per_token": 0.000075,
            "cache_read_input_token_cost": 0.0000015, "cache_creation_input_token_cost": 0.00001875,
            "mode": "chat", "litellm_provider": "anthropic"},
        "gpt-6": {"input_cost_per_token": 0.000002, "output_cost_per_token": 0.00001,
            "mode": "responses", "litellm_provider": "openai"},
        "text-embedding-3": {"input_cost_per_token": 0.00000002, "output_cost_per_token": 0.0,
            "mode": "embedding", "litellm_provider": "openai"},
        "deepseek/deepseek-v4": {"input_cost_per_token": 0.0000003, "output_cost_per_token": 0.0000012,
            "litellm_provider": "deepseek"},
        "mystery": {"input_cost_per_token": 0.000001, "litellm_provider": "xyz"},
        "grok": {"input_cost_per_token": 0.000003, "output_cost_per_token": 0.000015,
            "mode": "chat", "litellm_provider": "xai"},
        "xai/grok": {"input_cost_per_token": 0.000009, "output_cost_per_token": 0.00003,
            "mode": "chat", "litellm_provider": "xai"}
    }"#;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn catalog() -> Catalog {
        let catalog = Catalog::default();
        *catalog.state.write().unwrap() = CatalogState {
            fetched_at_ms: 1,
            openrouter: parse_openrouter(OPENROUTER).unwrap(),
            litellm: parse_litellm(LITELLM).unwrap(),
            sources: Vec::new(),
        };
        catalog
    }

    #[test]
    fn openrouter_prices_are_per_million_and_skip_variable_routers() {
        let list = parse_openrouter(OPENROUTER).unwrap();
        assert_eq!(list.len(), 2);
        assert!(!list.contains_key("openrouter/auto"));
        let opus = list["anthropic/claude-opus-5-5"];
        assert!(close(opus.input, 15.0) && close(opus.output, 75.0));
        assert!(close(opus.cache_read.unwrap(), 1.5));
        assert!(close(opus.cache_write.unwrap(), 18.75));
        let deepseek = list["deepseek/deepseek-v4"];
        assert!(close(deepseek.input, 0.27) && close(deepseek.output, 1.1));
        assert_eq!(deepseek.cache_read, None);
        assert_eq!(deepseek.cache_write, None);
    }

    #[test]
    fn litellm_keeps_chat_entries_and_adds_provider_aliases() {
        let list = parse_litellm(LITELLM).unwrap();
        let mut keys: Vec<&str> = list.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "anthropic/claude-opus-5-5",
                "claude-opus-5-5",
                "deepseek/deepseek-v4",
                "gpt-6",
                "grok",
                "openai/gpt-6",
                "xai/grok",
            ]
        );
        let gpt = list["gpt-6"];
        assert!(close(gpt.input, 2.0) && close(gpt.output, 10.0));
        assert_eq!(gpt.cache_read, None);
        assert_eq!(list["anthropic/claude-opus-5-5"], list["claude-opus-5-5"]);
        assert!(close(list["claude-opus-5-5"].cache_write.unwrap(), 18.75));
        // The listed "xai/grok" wins over the alias made from "grok", whatever the order.
        assert!(close(list["xai/grok"].input, 9.0));
        assert!(close(list["grok"].input, 3.0));
    }

    #[test]
    fn catalog_lookup_resolves_ids_by_source() {
        // Openrouter ids resolve from the openrouter map.
        {
            let catalog = catalog();
            let none = BTreeMap::new();
            let price = ModelPrice {
                input: 1.5,
                output: 3.0,
                cache_read: None,
                cache_write: None,
            };
            {
                let mut state = catalog.state.write().unwrap();
                state.openrouter.insert("anthropic/claude-x".into(), price);
                state.openrouter.insert("vendor/model:free".into(), price);
            }

            let claude = catalog
                .lookup("openrouter/anthropic/claude-x", &none)
                .unwrap();
            assert_eq!(claude.source, Source::Openrouter);
            assert!(close(claude.price.input, 1.5) && close(claude.price.output, 3.0));

            let free = catalog
                .lookup("openrouter/vendor/model:free", &none)
                .unwrap();
            assert_eq!(free.source, Source::Openrouter);
            assert!(close(free.price.input, 1.5));
        }

        // Openrouter ids never fall through to litellm.
        {
            let catalog = catalog();
            let none = BTreeMap::new();
            let price = ModelPrice {
                input: 9.0,
                output: 9.0,
                cache_read: None,
                cache_write: None,
            };
            catalog
                .state
                .write()
                .unwrap()
                .litellm
                .insert("unknown/x".into(), price);

            assert!(catalog.lookup("unknown/x", &none).is_some());
            assert_eq!(catalog.lookup("openrouter/unknown/x", &none), None);
        }

        // Opencode ids resolve from the vendor table.
        {
            let catalog = catalog();
            let none = BTreeMap::new();

            let sonnet = catalog.lookup("opencode/claude-sonnet-5-5", &none).unwrap();
            assert_eq!(sonnet.source, Source::Vendor);
            assert!(close(sonnet.price.input, 2.0) && close(sonnet.price.output, 10.0));

            assert_eq!(catalog.lookup("opencode/no-such-model", &none), None);
        }

        // Lookup order manual then openrouter then litellm.
        {
            let catalog = catalog();
            let none = BTreeMap::new();

            let opus = catalog
                .lookup("anthropic/claude-opus-5-5@high", &none)
                .unwrap();
            assert_eq!(opus.source, Source::Openrouter);
            assert!(close(opus.price.input, 15.0));

            let upper = catalog.lookup("Anthropic/Claude-Opus-5-5", &none).unwrap();
            assert_eq!(upper.source, Source::Openrouter);

            let gpt = catalog.lookup("openai/gpt-6", &none).unwrap();
            assert_eq!(gpt.source, Source::Litellm);
            assert!(close(gpt.price.input, 2.0));

            // Not in OpenRouter, and LiteLLM knows the part after the first '/'.
            let tail = catalog.lookup("bedrock/claude-opus-5-5", &none).unwrap();
            assert_eq!(tail.source, Source::Litellm);
            assert!(close(tail.price.output, 75.0));

            assert_eq!(catalog.lookup("openai/unknown-model", &none), None);
        }

        // Manual prices win and have no cache prices. They are dollars per million tokens.
        {
            let catalog = catalog();
            let mut manual = BTreeMap::new();
            manual.insert(
                "Anthropic/Claude-Opus-5-5".to_string(),
                crate::settings::Price {
                    input: 1.0,
                    output: 2.0,
                },
            );
            let priced = catalog
                .lookup("anthropic/claude-opus-5-5@high", &manual)
                .unwrap();
            assert_eq!(priced.source, Source::Manual);
            assert!(close(priced.price.input, 1.0) && close(priced.price.output, 2.0));
            assert_eq!(priced.price.cache_read, None);
            assert_eq!(priced.price.cache_write, None);
        }
    }

    #[test]
    fn the_compatible_providers_are_priced_from_the_lists_they_appear_in() {
        // The fetched list prices the OpenAI-compatible presets (PLAN 11.3) by their bare model ids, so
        // `<preset>/<model>` resolves through the part after the first `/`. A local server is in no list: unpriced.
        const COMPATIBLE: &str = r#"{
            "qwen-plus": {"input_cost_per_token": 0.0000004, "output_cost_per_token": 0.0000012,
                "mode": "chat", "litellm_provider": "dashscope"},
            "kimi-k3": {"input_cost_per_token": 0.0000006, "output_cost_per_token": 0.0000025,
                "mode": "chat", "litellm_provider": "moonshot"},
            "glm-5.3": {"input_cost_per_token": 0.0000006, "output_cost_per_token": 0.0000022,
                "mode": "chat", "litellm_provider": "zai"},
            "mistral-large-latest": {"input_cost_per_token": 0.000002, "output_cost_per_token": 0.000006,
                "mode": "chat", "litellm_provider": "mistral"},
            "grok-4.7": {"input_cost_per_token": 0.000003, "output_cost_per_token": 0.000015,
                "mode": "chat", "litellm_provider": "xai"}
        }"#;
        let catalog = Catalog::default();
        *catalog.state.write().unwrap() = CatalogState {
            fetched_at_ms: 1,
            openrouter: Default::default(),
            litellm: parse_litellm(COMPATIBLE).unwrap(),
            sources: Vec::new(),
        };
        let none = BTreeMap::new();
        for id in [
            "qwen/qwen-plus",
            "moonshot/kimi-k3",
            "zai/glm-5.3",
            "mistral/mistral-large-latest",
            "xai/grok-4.7",
        ] {
            let priced = catalog
                .lookup(id, &none)
                .unwrap_or_else(|| panic!("{id} is priced"));
            assert!(
                priced.price.input > 0.0 && priced.price.output > 0.0,
                "{id}"
            );
        }
        assert!(catalog.lookup("vllm/llama-3", &none).is_none());
        assert!(catalog.lookup("llamacpp/local-model", &none).is_none());
    }

    #[test]
    fn a_named_local_transport_keeps_its_upstream_vendor_price() {
        let catalog = Catalog::default();
        let mut manual = BTreeMap::new();
        let upstream = catalog.lookup("opencode/glm-5.3", &manual).unwrap();
        let routed = catalog
            .lookup("local-glm/opencode/glm-5.3@high", &manual)
            .unwrap();
        assert_eq!(routed, upstream);
        assert!(routed.price.input > 0.0 && routed.price.output > 0.0);
        assert!(
            catalog
                .lookup("local-glm/opencode/unknown", &manual)
                .is_none()
        );
        // A price specifically configured for the instance still wins.
        manual.insert(
            "local-glm/opencode/glm-5.3".into(),
            crate::settings::Price {
                input: 2.0,
                output: 5.0,
            },
        );
        let custom = catalog
            .lookup("local-glm/opencode/glm-5.3@high", &manual)
            .unwrap();
        assert_eq!(custom.source, Source::Manual);
        assert_eq!(custom.price.input, 2.0);
        assert_eq!(custom.price.output, 5.0);
    }

    #[test]
    fn bad_bodies_are_errors() {
        assert!(parse_openrouter("not json").is_err());
        assert!(parse_litellm("[1, 2]").is_err());
    }

    #[test]
    fn meta_muse_prices_are_built_in_and_contributor_variants_are_priced() {
        let catalog = Catalog::default();
        let none = BTreeMap::new();
        let spark = catalog.lookup("meta/muse-spark-1.3@high", &none).unwrap();
        assert_eq!(spark.source, Source::Vendor);
        assert!(close(spark.price.input, 1.25) && close(spark.price.output, 4.25));
        assert!(close(spark.price.cache_read.unwrap(), 0.15));
        let wonder = catalog.lookup("meta/muse-wonder-1.0", &none).unwrap();
        assert!(
            close(wonder.price.input, 4.0)
                && close(wonder.price.output, 20.0)
                && close(wonder.price.cache_read.unwrap(), 0.2)
        );
        let contributor = catalog
            .lookup("meta/muse-spark-1.3-contributor", &none)
            .unwrap();
        assert!(close(contributor.price.input, 0.10) && close(contributor.price.output, 0.20));
        assert!(close(contributor.price.cache_read.unwrap(), 0.002));
        // Manual prices still win over the built-in ones.
        let mut manual = BTreeMap::new();
        manual.insert(
            "meta/muse-wonder-1.0".to_string(),
            crate::settings::Price {
                input: 1.0,
                output: 2.0,
            },
        );
        assert_eq!(
            catalog
                .lookup("meta/muse-wonder-1.0", &manual)
                .unwrap()
                .source,
            Source::Manual
        );
        // Every built-in price is listed, as the vendor's.
        let listed = listing(Catalog::default().state());
        assert_eq!(
            listed
                .models
                .iter()
                .filter(|e| e.price.source == Source::Vendor)
                .count(),
            VENDOR.len()
        );
    }

    #[test]
    fn estimate_bills_cache_at_its_price_or_the_input_price() {
        let price = ModelPrice {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.3),
            cache_write: None,
        };
        let tokens = Tokens {
            input: 1_000_000,
            output: 100_000,
            cache_read: 2_000_000,
            cache_write: 500_000,
        };
        // 3 + 1.5 + 0.6 + 1.5
        assert!(close(estimate(&price, &tokens), 6.6));
        assert_eq!(estimate(&price, &Tokens::default()), 0.0);
    }

    #[test]
    fn decodes_model_path_segments() {
        assert_eq!(percent_decode_path("openai%2Fgpt-6"), "openai/gpt-6");
        assert_eq!(percent_decode_path("a+b"), "a+b");
        assert_eq!(percent_decode_path("bad%zz%4"), "bad%zz%4");
    }

    /// The catalog written to its tables and loaded into a fresh catalog, as a restart does.
    #[test]
    fn the_catalog_comes_back_whole_from_its_tables() {
        let before = catalog();
        before.state.write().unwrap().sources = vec![SourceStatus {
            name: "litellm".into(),
            url: LITELLM_URL.into(),
            models: 2,
            ok: false,
            error: Some("timed out".into()),
        }];
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &before.rows().unwrap()).unwrap();
        let after = Catalog::default();
        after.load(&conn).unwrap();
        assert_eq!(
            serde_json::to_value(after.state()).unwrap(),
            serde_json::to_value(before.state()).unwrap()
        );
    }
}
