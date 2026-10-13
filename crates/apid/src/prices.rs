//! Model prices: what one million tokens of each model costs, so the plaza can show what its
//! agents spend. The catalog is fetched from two public lists and kept in Postgres ("prices"),
//! so a hub without internet still has the last prices it saw:
//!
//! - OpenRouter's model list (`https://openrouter.ai/api/v1/models`): every OpenRouter model id
//!   (`provider/model`), with prompt, completion, cache-read and cache-write prices per token.
//! - LiteLLM's price table (`model_prices_and_context_window.json` in BerriAI/litellm): the
//!   providers' own model ids (Anthropic, OpenAI, DeepSeek, …), with per-token input, output,
//!   cache-read (`cache_read_input_token_cost`) and cache-write (`cache_creation_input_token_cost`).
//!
//! The owner's manual prices (`settings.prices`, credits per million tokens; 1 credit = $0.001) win over both.
//! Lookup order for a plaza model id such as `anthropic/claude-opus-5-5`: manual price, then the
//! OpenRouter id as is, then LiteLLM by the full id, then LiteLLM by the part after the first
//! `/`; ids compare case-insensitively.
//!
//! GET  /v1/prices          → {fetched_at_ms, sources: [{name, url, models, ok, error?}], models: [{model, price}]}
//! GET  /v1/prices/{model}  → {model, price} (404 when nothing prices it); `{model}` is percent-encoded
//! POST /v1/prices/refresh  → the same as GET /v1/prices, after fetching both lists again
//!
//! The catalog refreshes when apid starts (if older than a day) and once a day after.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

use apz_http::{Router, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::App;

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";
pub const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
/// How old the catalog may get before it is fetched again.
/// What one plaza credit is worth (`settings::Price` is in credits per million tokens).
pub const USD_PER_CREDIT: f64 = 0.001;
pub const MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;
/// A catalog with a failed list is fetched again after this long instead of [`MAX_AGE_MS`].
const RETRY_MS: u64 = 60 * 60 * 1000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// The lists are a few megabytes; ureq's default 10 MB reader limit leaves too little room.
const BODY_LIMIT: u64 = 64 * 1024 * 1024;
/// LiteLLM providers whose bare model ids also get a `<provider>/<id>` alias.
const ALIAS_PROVIDERS: &[&str] = &["anthropic", "openai", "deepseek", "gemini", "xai", "mistral"];

/// One model's prices in US dollars per million tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
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
    Openrouter,
    Litellm,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Priced {
    #[serde(flatten)]
    pub price: ModelPrice,
    pub source: Source,
}

/// What one fetch of one list did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceStatus {
    pub name: String,
    pub url: String,
    pub models: usize,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The fetched lists, keyed by lower-cased model id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogState {
    pub fetched_at_ms: u64,
    pub openrouter: BTreeMap<String, ModelPrice>,
    pub litellm: BTreeMap<String, ModelPrice>,
    pub sources: Vec<SourceStatus>,
}

/// Token counts of one call, as the gateway and the proxy meter them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The price of `model`: `manual` (settings.prices: credits per million tokens, converted at
    /// [`USD_PER_CREDIT`]) first, then the
    /// fetched lists in the order the module docs give.
    pub fn lookup(&self, model: &str, manual: &BTreeMap<String, crate::settings::Price>) -> Option<Priced> {
        let id = strip_effort(model.trim()).to_lowercase();
        if let Some((_, price)) = manual.iter().find(|(key, _)| key.trim().to_lowercase() == id) {
            return Some(Priced {
                price: ModelPrice {
                    input: price.input * USD_PER_CREDIT,
                    output: price.output * USD_PER_CREDIT,
                    cache_read: None,
                    cache_write: None,
                },
                source: Source::Manual,
            });
        }
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(price) = state.openrouter.get(&id) {
            return Some(Priced { price: *price, source: Source::Openrouter });
        }
        let by_full_id = state.litellm.get(&id);
        let by_tail = id.split_once('/').and_then(|(_, rest)| state.litellm.get(rest));
        by_full_id.or(by_tail).map(|price| Priced { price: *price, source: Source::Litellm })
    }

    /// Fetches both lists (blocking, ~10 s timeout each) and replaces the catalog with what came
    /// back; a list that fails keeps its previous entries and reports the error in `sources`.
    /// `fetched_at_ms` moves on every call, failed or not.
    pub fn refresh(&self) -> CatalogState {
        let _turn = self.fetching.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = self.state();
        let (openrouter, openrouter_status) =
            fetch_list("openrouter", OPENROUTER_URL, parse_openrouter, previous.openrouter);
        let (litellm, litellm_status) = fetch_list("litellm", LITELLM_URL, parse_litellm, previous.litellm);
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

    pub fn state(&self) -> CatalogState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn snapshot(&self) -> String {
        serde_json::to_string(&self.state()).unwrap_or_default()
    }

    pub fn restore(&self, json: &str) -> Result<(), String> {
        let state: CatalogState = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.state.write().map_err(|e| e.to_string())? = state;
        Ok(())
    }
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
    let list: List = serde_json::from_str(body).map_err(|e| format!("OpenRouter's model list: {e}"))?;
    let mut out = BTreeMap::new();
    for model in list.data {
        let id = model.id.trim().to_lowercase();
        let price = |key: &str| per_million(model.pricing.get(key));
        let (Some(input), Some(output)) = (price("prompt"), price("completion")) else { continue };
        if id.is_empty() {
            continue;
        }
        let cache_read = price("input_cache_read");
        let cache_write = price("input_cache_write");
        out.insert(id, ModelPrice { input, output, cache_read, cache_write });
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
        let (Some(input), Some(output)) = (field("input_cost_per_token"), field("output_cost_per_token")) else {
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
        if !key.contains('/') {
            if let Some(provider) = entry.get("litellm_provider").and_then(Value::as_str) {
                let provider = provider.to_lowercase();
                if ALIAS_PROVIDERS.contains(&provider.as_str()) {
                    aliases.push((format!("{provider}/{key}"), price));
                }
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
                && effort.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
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
    let mut response = agent.run(request).map_err(|e| format!("did not answer: {e}"))?;
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
            price: Priced { price: *price, source: Source::Openrouter },
        })
        .collect();
    models.extend(
        state
            .litellm
            .iter()
            .filter(|(model, _)| !state.openrouter.contains_key(*model))
            .map(|(model, price)| Entry {
                model: model.clone(),
                price: Priced { price: *price, source: Source::Litellm },
            }),
    );
    Listing { fetched_at_ms: state.fetched_at_ms, sources: state.sources, models }
}

/// What GET /v1/prices/{model} answers.
#[derive(Serialize)]
struct OneModel {
    model: String,
    price: Priced,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/prices", |app, _, _| Response::json(200, &listing(app.prices.state())))
        .get("/v1/prices/{model}", |app, _, p| {
            let model = percent_decode_path(p.get("model"));
            let manual = app.settings.get().prices;
            match app.prices.lookup(&model, &manual) {
                Some(price) => Response::json(200, &OneModel { model, price }),
                None => Response::error(404, &format!("no price for {model}")),
            }
        })
        .post("/v1/prices/refresh", |app, _, _| {
            app.prices.refresh();
            Response::json(200, &listing(app.prices.state()))
        })
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
    fn bad_bodies_are_errors() {
        assert!(parse_openrouter("not json").is_err());
        assert!(parse_litellm("[1, 2]").is_err());
    }

    #[test]
    fn lookup_order_manual_then_openrouter_then_litellm() {
        let catalog = catalog();
        let none = BTreeMap::new();

        let opus = catalog.lookup("anthropic/claude-opus-5-5@high", &none).unwrap();
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

    #[test]
    fn manual_prices_win_and_have_no_cache_prices() {
        let catalog = catalog();
        let mut manual = BTreeMap::new();
        manual.insert("Anthropic/Claude-Opus-5-5".to_string(), crate::settings::Price { input: 1000.0, output: 2000.0 });
        let priced = catalog.lookup("anthropic/claude-opus-5-5@high", &manual).unwrap();
        assert_eq!(priced.source, Source::Manual);
        assert!(close(priced.price.input, 1.0) && close(priced.price.output, 2.0));
        assert_eq!(priced.price.cache_read, None);
        assert_eq!(priced.price.cache_write, None);
    }

    #[test]
    fn strips_only_a_plain_effort_suffix() {
        assert_eq!(strip_effort("openai/gpt-6@high"), "openai/gpt-6");
        assert_eq!(strip_effort("openai/gpt-6"), "openai/gpt-6");
        assert_eq!(strip_effort("@high"), "@high");
        assert_eq!(strip_effort("a@b/c"), "a@b/c");
    }

    #[test]
    fn estimate_bills_cache_at_its_price_or_the_input_price() {
        let price = ModelPrice { input: 3.0, output: 15.0, cache_read: Some(0.3), cache_write: None };
        let tokens = Tokens { input: 1_000_000, output: 100_000, cache_read: 2_000_000, cache_write: 500_000 };
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

    #[test]
    fn listing_puts_openrouter_first_without_duplicates() {
        let state = catalog().state();
        let listing = listing(state);
        let models: Vec<&str> = listing.models.iter().map(|e| e.model.as_str()).collect();
        assert_eq!(models.iter().filter(|m| **m == "anthropic/claude-opus-5-5").count(), 1);
        assert_eq!(listing.models[0].price.source, Source::Openrouter);
        let gpt = listing.models.iter().find(|e| e.model == "openai/gpt-6").unwrap();
        assert_eq!(gpt.price.source, Source::Litellm);
    }
}
