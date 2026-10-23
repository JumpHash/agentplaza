//! The model gateway: one blocking client that talks to every model API.
//!
//! [`Gateway::complete`] takes a provider-neutral [`ChatRequest`] whose model
//! is `"<provider-id>/<model-id>"`, translates it to that provider's API,
//! streams the answer's fragments to a sink as [`Delta`]s and returns the
//! authoritative [`ChatResponse`]. Each provider kind is one module:
//!
//! * `anthropic` — Messages API, direct and on Vertex AI;
//! * `openai` — Chat Completions: OpenAI, Azure OpenAI, every compatible server;
//! * `responses` — the OpenAI Responses API;
//! * `decisions` — decision models (OpenAI Decisions, System One): not chats,
//!   see [`Gateway::decide`];
//! * `gemini` — Gemini API and Gemini on Vertex AI;
//! * `bedrock` — Bedrock Converse (with a SigV4 signer);
//! * `mock` — deterministic and offline.
//!
//! Credentials come from the environment, a [`SecretSource`] or a command,
//! and never appear in an error.

#![forbid(unsafe_code)]

mod anthropic;
mod bedrock;
mod config;
mod decisions;
mod error;
mod gemini;
mod http;
mod mock;
mod openai;
mod responses;
mod sigv4;
#[cfg(test)]
mod testutil;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::Command;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub use config::{Auth, Preset, ProviderConfig, ProviderKind, lists_training_model, presets};
pub use decisions::{
    Answer, ChoiceOption, ChoiceProbability, DecisionRequest, DecisionResponse, Level,
    LevelProbability, Question, Usage,
};
pub use error::{GatewayError, SecretReason};

use crate::error::truncate;
use crate::http::Http;

/// How long a token printed by an [`Auth::Command`] is reused.
const COMMAND_TOKEN_TTL: Duration = Duration::from_secs(50 * 60);

/// `ToolUse.meta.error` when a streamed tool input was not valid JSON: the
/// harness must answer with an error result instead of running the tool.
pub const INVALID_JSON: &str = "INVALID_JSON";

/// The gateway's complaint about a tool call's input, if it has one (see
/// [`INVALID_JSON`]).
pub fn tool_input_error(meta: Option<&Value>) -> Option<&str> {
    let meta = meta?;
    (meta.get("kind")?.as_str()? == "gateway").then(|| meta.get("error")?.as_str())?
}

/// Whose call a credential is read for. The hub passes it to the [`SecretSource`], which decides
/// what the caller may read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The hub's own background work.
    Hub,
    /// The owner, from the console (a provider test or a model list). `node` is the node the owner chose
    /// (`None`: the hub's own machine).
    Owner { node: Option<String> },
    /// An agent's model call. `node` is the node whose CID range holds the agent, computed by the
    /// hub (`None` when a revoked node's range holds it: nothing may be read).
    Agent { cid: u64, node: Option<String> },
}

/// A credential read: the provider it belongs to, and whose call it is.
pub struct SecretRead<'a> {
    pub provider: &'a str,
    pub caller: &'a Caller,
}

/// Named credentials. `name` is the provider's secret name (`Auth::Secret`, or the env-style name
/// of `Auth::Env`); `read` says which provider and whose call it is. `apid` reads the vault.
pub trait SecretSource: Send + Sync {
    fn get(&self, name: &str, read: &SecretRead<'_>) -> Option<String>;

    /// The secret, or why there is none. The default answers [`SecretSource::get`] and says `Missing`. A
    /// source that can say more (the vault: sealed, tampered, held elsewhere) overrides this instead, so
    /// the gateway makes one lookup per call, and that lookup is the only one audited.
    fn fetch(&self, name: &str, read: &SecretRead<'_>) -> Result<String, SecretReason> {
        self.get(name, read)
            .filter(|s| !s.is_empty())
            .ok_or(SecretReason::Missing)
    }
}

impl SecretSource for BTreeMap<String, String> {
    fn get(&self, name: &str, _read: &SecretRead<'_>) -> Option<String> {
        BTreeMap::get(self, name).cloned()
    }
}

impl SecretSource for HashMap<String, String> {
    fn get(&self, name: &str, _read: &SecretRead<'_>) -> Option<String> {
        HashMap::get(self, name).cloned()
    }
}

impl<F: Fn(&str, &SecretRead<'_>) -> Option<String> + Send + Sync> SecretSource for F {
    fn get(&self, name: &str, read: &SecretRead<'_>) -> Option<String> {
        self(name, read)
    }
}

/// A model a provider offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Context window in tokens, when the API says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
}

impl ModelInfo {
    pub fn new(id: impl Into<String>) -> Self {
        ModelInfo {
            id: id.into(),
            name: None,
            context: None,
        }
    }
}

/// The outcome of [`Gateway::test`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestReport {
    pub ok: bool,
    /// `provider/model` that answered (or was asked).
    pub model: String,
    pub latency_ms: u64,
    /// The model's reply text.
    #[serde(default)]
    pub reply: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

type EnvFn = dyn Fn(&str) -> Option<String> + Send + Sync;

/// Builds a [`Gateway`].
pub struct GatewayBuilder {
    providers: Vec<ProviderConfig>,
    secrets: Box<dyn SecretSource>,
    env: Box<EnvFn>,
    retries: u32,
    retry_delay: Duration,
}

impl GatewayBuilder {
    pub fn providers(mut self, providers: Vec<ProviderConfig>) -> Self {
        self.providers = providers;
        self
    }

    pub fn secrets(mut self, secrets: impl SecretSource + 'static) -> Self {
        self.secrets = Box::new(secrets);
        self
    }

    /// Replaces the process environment as the source of `{"env": …}`
    /// credentials and of the AWS / Google variables.
    pub fn env(mut self, env: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.env = Box::new(env);
        self
    }

    /// Retries after a retryable failure (default 2) and the first backoff
    /// (default 1 s, doubling; a `retry-after` header wins).
    pub fn retry(mut self, retries: u32, first_delay: Duration) -> Self {
        self.retries = retries;
        self.retry_delay = first_delay;
        self
    }

    pub fn build(self) -> Gateway {
        let providers = self
            .providers
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        Gateway {
            inner: Arc::new(Inner {
                providers: RwLock::new(providers),
                secrets: self.secrets,
                env: self.env,
                http: Http::new(self.retries, self.retry_delay),
                listing: Http::with_timeout(0, self.retry_delay, LISTING_DEADLINE),
                tokens: Mutex::new(HashMap::new()),
                no_stream_options: Mutex::new(HashSet::new()),
            }),
        }
    }
}

/// The model gateway. Cheap to clone; clones share providers, the connection
/// pool and cached tokens.
#[derive(Clone)]
pub struct Gateway {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    providers: RwLock<BTreeMap<String, ProviderConfig>>,
    secrets: Box<dyn SecretSource>,
    env: Box<EnvFn>,
    pub(crate) http: Http,
    /// The client for model listings: no retries and one deadline for the whole listing, so a slow provider never
    /// holds a listing for longer (the hub refreshes listings in the background and serves the last good one).
    pub(crate) listing: Http,
    /// Command → (token, fetched at).
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    /// Providers that rejected `stream_options`.
    pub(crate) no_stream_options: Mutex<HashSet<String>>,
}

/// How long one model listing may take, from the first byte sent to the last read.
pub const LISTING_DEADLINE: Duration = Duration::from_secs(10);

impl Gateway {
    pub fn builder() -> GatewayBuilder {
        GatewayBuilder {
            providers: Vec::new(),
            secrets: Box::new(|_: &str, _: &crate::SecretRead<'_>| None),
            env: Box::new(|name: &str| std::env::var(name).ok().filter(|v| !v.is_empty())),
            retries: 2,
            retry_delay: Duration::from_secs(1),
        }
    }

    /// A gateway over `providers`, reading secrets from `secrets`.
    pub fn new(providers: Vec<ProviderConfig>, secrets: impl SecretSource + 'static) -> Self {
        Gateway::builder()
            .providers(providers)
            .secrets(secrets)
            .build()
    }

    /// The configured providers, by id.
    pub fn providers(&self) -> Vec<ProviderConfig> {
        self.inner
            .providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Replaces the configured providers (cached tokens are kept).
    pub fn set_providers(&self, providers: Vec<ProviderConfig>) {
        let map = providers.into_iter().map(|p| (p.id.clone(), p)).collect();
        *self
            .inner
            .providers
            .write()
            .unwrap_or_else(|e| e.into_inner()) = map;
    }

    /// The provider `id` when it may take calls: known and enabled.
    fn callable(&self, id: &str) -> Result<ProviderConfig, GatewayError> {
        let p = self.provider(id)?;
        if p.enabled {
            Ok(p)
        } else {
            Err(GatewayError::Disabled(p.id))
        }
    }

    pub fn provider(&self, id: &str) -> Result<ProviderConfig, GatewayError> {
        let providers = self
            .inner
            .providers
            .read()
            .unwrap_or_else(|e| e.into_inner());
        providers
            .get(id)
            .cloned()
            .ok_or_else(|| GatewayError::UnknownProvider(id.to_string()))
    }

    /// One model call. `req.model` is `"<provider-id>/<model-id>"`, optionally
    /// `@<effort>` (which overrides `req.effort`; see [`ModelRef`]); live
    /// fragments go to `sink`; the returned response's `model` is
    /// `provider-id/<the model the API says answered>`.
    pub fn complete(
        &self,
        req: &ChatRequest,
        sink: &mut dyn FnMut(Delta),
    ) -> Result<ChatResponse, GatewayError> {
        self.complete_for(req, sink, &Caller::Hub)
    }

    /// [`Gateway::complete`] for `caller`: the credential is read for that caller only.
    pub fn complete_for(
        &self,
        req: &ChatRequest,
        sink: &mut dyn FnMut(Delta),
        caller: &Caller,
    ) -> Result<ChatResponse, GatewayError> {
        let name = ModelRef::parse(&req.model)?;
        let pinned;
        let req = match &name.effort {
            Some(effort) if req.effort.as_ref() != Some(effort) => {
                pinned = ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    effort: Some(effort.clone()),
                    ..req.clone()
                };
                &pinned
            }
            _ => req,
        };
        let (provider_id, model) = (name.provider.as_str(), name.model.as_str());
        let p = self.callable(provider_id)?;
        let ctx = Ctx {
            gw: &self.inner,
            p: &p,
            caller,
            listing: false,
        };
        if req.has_images() && !p.kind.takes_images() {
            return Err(GatewayError::Unsupported(format!(
                "{provider_id} ({}) takes no images: its wire format has no image block, so the call is refused",
                p.kind.as_str()
            )));
        }
        let mut resp = match p.kind {
            ProviderKind::Anthropic | ProviderKind::VertexAnthropic => {
                anthropic::complete(&ctx, req, model, sink)
            }
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible | ProviderKind::AzureOpenAi => {
                openai::complete(&ctx, req, model, sink)
            }
            ProviderKind::OpenAiResponses => responses::complete(&ctx, req, model, sink),
            ProviderKind::Gemini | ProviderKind::VertexGemini => {
                gemini::complete(&ctx, req, model, sink)
            }
            ProviderKind::Bedrock => bedrock::complete(&ctx, req, model, sink),
            ProviderKind::Mock => Ok(mock::complete(req, model, sink)),
            ProviderKind::OpenAiDecisions
            | ProviderKind::SystemOne
            | ProviderKind::AzureDecisions => Err(GatewayError::Config(format!(
                "{provider_id} is a decision model; it answers questions, not chats"
            ))),
        }?;
        if resp.model.is_empty() {
            resp.model = model.to_string();
        }
        resp.model = format!("{provider_id}/{}", resp.model);
        Ok(resp)
    }

    /// The models a provider offers: its live listing (where the API has
    /// one) merged with the configured static `models`.
    pub fn list_models(&self, provider_id: &str) -> Result<Vec<ModelInfo>, GatewayError> {
        self.list_models_for(provider_id, &Caller::Hub)
    }

    /// [`Gateway::list_models`] for `caller`.
    pub fn list_models_for(
        &self,
        provider_id: &str,
        caller: &Caller,
    ) -> Result<Vec<ModelInfo>, GatewayError> {
        let p = self.provider(provider_id)?;
        // The live listing, fetched from the provider's API with the listing client (no retries, one deadline).
        // Caching is the caller's: the hub keeps the last good listing per provider (providers.rs).
        let ctx = Ctx {
            gw: &self.inner,
            p: &p,
            caller,
            listing: true,
        };
        let mut models = match p.kind {
            ProviderKind::Anthropic => anthropic::list_models(&ctx)?,
            ProviderKind::OpenAi
            | ProviderKind::OpenAiCompatible
            | ProviderKind::OpenAiResponses => openai::list_models(&ctx)?,
            ProviderKind::Gemini => gemini::list_models(&ctx)?,
            ProviderKind::Bedrock => bedrock::list_models(&ctx)?,
            ProviderKind::Mock => mock::MODELS.iter().map(|m| ModelInfo::new(*m)).collect(),
            ProviderKind::AzureOpenAi
            | ProviderKind::VertexAnthropic
            | ProviderKind::VertexGemini
            | ProviderKind::OpenAiDecisions
            | ProviderKind::SystemOne
            | ProviderKind::AzureDecisions => Vec::new(),
        };
        for id in &p.models {
            if !models.iter().any(|m| &m.id == id) {
                models.push(ModelInfo::new(id.clone()));
            }
        }
        Ok(models)
    }

    /// One decision call: the decision model `model` of `provider_id` answers
    /// `req`'s questions. A chat provider is refused; the request is validated
    /// before anything is sent.
    pub fn decide(
        &self,
        provider_id: &str,
        model: &str,
        req: &DecisionRequest,
    ) -> Result<DecisionResponse, GatewayError> {
        let p = self.callable(provider_id)?;
        if p.kind.group() != "decisions" {
            return Err(GatewayError::Config(format!(
                "{provider_id} is a chat model; it does not answer decisions"
            )));
        }
        req.validate()
            .map_err(|e| GatewayError::Config(format!("invalid decision request: {e}")))?;
        let ctx = Ctx {
            gw: &self.inner,
            p: &p,
            caller: &Caller::Hub,
            listing: false,
        };
        decisions::decide(&ctx, model, req)
    }

    /// One tiny call to check a provider works. `model` defaults to the
    /// first configured model, else the first listed one. Only a missing
    /// provider or model is an `Err`; a failed call is a report with `ok: false`.
    pub fn test(&self, provider_id: &str, model: Option<&str>) -> Result<TestReport, GatewayError> {
        self.test_for(provider_id, model, &Caller::Hub)
    }

    /// [`Gateway::test`] for `caller` (the owner's console test passes [`Caller::Owner`]).
    pub fn test_for(
        &self,
        provider_id: &str,
        model: Option<&str>,
        caller: &Caller,
    ) -> Result<TestReport, GatewayError> {
        let p = self.provider(provider_id)?;
        let model = match model.filter(|m| !m.is_empty()) {
            Some(m) => m.to_string(),
            None => match p.models.first() {
                Some(m) => m.clone(),
                None => self
                    .list_models_for(provider_id, caller)?
                    .into_iter()
                    .next()
                    .map(|m| m.id)
                    .ok_or_else(|| {
                        GatewayError::Config(format!("provider `{provider_id}` lists no models"))
                    })?,
            },
        };
        let req = ChatRequest {
            system_parts: Vec::new(),
            cache_last_message: false,
            model: format!("{provider_id}/{model}"),
            system: None,
            messages: vec![Message::user_text("Reply with the single word: ok")],
            tools: Vec::new(),
            max_tokens: 256,
            temperature: None,
            effort: Some("low".to_string()),
        };
        let started = Instant::now();
        let result = self.complete_for(&req, &mut |_| {}, caller);
        let latency_ms = started.elapsed().as_millis() as u64;
        Ok(match result {
            Ok(resp) => TestReport {
                ok: true,
                reply: resp.text(),
                model: resp.model,
                latency_ms,
                error: None,
            },
            Err(e) => TestReport {
                ok: false,
                model: req.model,
                latency_ms,
                reply: String::new(),
                error: Some(e.to_string()),
            },
        })
    }
}

/// Effort levels a model name's `@effort` suffix may pin.
pub const EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// A parsed model name: `provider/model` or `provider/model@effort`.
///
/// The model may itself contain `/` (`openrouter/anthropic/claude-x`) or a
/// non-effort `@` (Vertex's `claude-sonnet-4-5@20250929`): only a trailing
/// `@` + one of [`EFFORTS`] is an effort pin.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
    /// The pinned effort; it overrides the request's.
    pub effort: Option<String>,
}

impl ModelRef {
    pub fn parse(name: &str) -> Result<ModelRef, GatewayError> {
        let bad = || GatewayError::BadModelName(name.to_string());
        let (provider, rest) = name.trim().split_once('/').ok_or_else(bad)?;
        let (model, effort) = match rest.rsplit_once('@') {
            Some((model, effort)) if EFFORTS.contains(&effort.to_ascii_lowercase().as_str()) => {
                (model, Some(effort.to_ascii_lowercase()))
            }
            _ => (rest, None),
        };
        if provider.is_empty() || model.is_empty() {
            return Err(bad());
        }
        Ok(ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
            effort,
        })
    }

    /// `provider/model`, without the effort pin.
    pub fn base_name(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

impl std::fmt::Display for ModelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)?;
        match &self.effort {
            Some(effort) => write!(f, "@{effort}"),
            None => Ok(()),
        }
    }
}

impl std::str::FromStr for ModelRef {
    type Err = GatewayError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ModelRef::parse(s)
    }
}

/// Whether the model name `provider/model[@effort]` is a contributor model: one that a preset lists
/// in `options.trains_on_data`. Such a model trains its provider's future models on what it is sent
/// and returns, so the hub never picks one by itself (see `apid`'s settings).
pub fn trains_on_data(name: &str) -> bool {
    let Ok(model) = ModelRef::parse(name) else {
        return false;
    };
    presets()
        .iter()
        .any(|p| p.id == model.provider && lists_training_model(p.options.as_ref(), &model.model))
}

/// What an adapter needs for one call: the gateway and the provider.
pub(crate) struct Ctx<'a> {
    pub gw: &'a Inner,
    pub p: &'a ProviderConfig,
    /// Whose call this is: the credential is read for it.
    pub caller: &'a Caller,
    /// A model listing (the listing client), not a model call.
    pub listing: bool,
}

impl Ctx<'_> {
    pub fn env(&self, name: &str) -> Option<String> {
        (self.gw.env)(name)
    }

    pub fn http(&self) -> &Http {
        if self.listing {
            &self.gw.listing
        } else {
            &self.gw.http
        }
    }

    /// The named credential for this provider and caller: the source's value, or why there is none.
    /// One lookup per call.
    fn read_secret(&self, name: &str) -> Result<String, SecretReason> {
        let read = SecretRead {
            provider: &self.p.id,
            caller: self.caller,
        };
        self.gw.secrets.fetch(name, &read)
    }

    /// A named secret this provider needs, for this caller: the value, the source's refusal (sealed,
    /// out of scope), or `Auth` when the source holds no such secret. Never read from the process
    /// environment (the AWS key pair is read this way).
    pub(crate) fn required_secret(&self, name: &str) -> Result<String, GatewayError> {
        self.read_secret(name).map_err(|reason| {
            reason.error().unwrap_or_else(|| {
                GatewayError::Auth(format!(
                    "secret `{name}` is not set (provider `{}`)",
                    self.p.id
                ))
            })
        })
    }

    /// A named secret this provider may do without: `None` when the source holds none, but the source's
    /// refusal (sealed, out of scope) still fails the call, so a refused key is never silently dropped.
    pub(crate) fn optional_secret(&self, name: &str) -> Result<Option<String>, GatewayError> {
        match self.read_secret(name) {
            Ok(value) => Ok(Some(value)),
            Err(reason) => match reason.error() {
                None => Ok(None),
                Some(refusal) => Err(refusal),
            },
        }
    }

    /// The provider's credential; `None` for [`Auth::None`].
    pub fn credential(&self) -> Result<Option<String>, GatewayError> {
        let id = &self.p.id;
        match &self.p.auth {
            Auth::None => Ok(None),
            // The vault first (its entry for this provider), then the process environment.
            Auth::Env(name) => match self.read_secret(name) {
                Ok(value) => Ok(Some(value)),
                Err(reason) => match reason.error() {
                    // Only a name the vault does not hold may come from the process environment.
                    None => self.env(name).map(Some).ok_or_else(|| {
                        GatewayError::Auth(format!(
                            "environment variable {name} is not set (provider `{id}`)"
                        ))
                    }),
                    Some(refusal) => Err(refusal),
                },
            },
            Auth::Secret(name) => self.read_secret(name).map(Some).map_err(|reason| {
                reason.error().unwrap_or_else(|| {
                    GatewayError::Auth(format!("secret `{name}` is not set (provider `{id}`)"))
                })
            }),
            Auth::Command(cmd) => self.command_token(cmd).map(Some),
        }
    }

    /// The credential, which this provider kind cannot do without.
    pub fn required_credential(&self) -> Result<String, GatewayError> {
        self.credential()?.ok_or_else(|| {
            GatewayError::Auth(format!(
                "provider `{}` has no credential configured",
                self.p.id
            ))
        })
    }

    fn command_token(&self, cmd: &str) -> Result<String, GatewayError> {
        let mut tokens = self.gw.tokens.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((token, at)) = tokens.get(cmd)
            && at.elapsed() < COMMAND_TOKEN_TTL
        {
            return Ok(token.clone());
        }
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| GatewayError::Auth(format!("credential command could not run: {e}")))?;
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || token.is_empty() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let first = stderr.lines().next().unwrap_or("").trim();
            return Err(GatewayError::Auth(format!(
                "credential command failed ({}): {}",
                out.status,
                truncate(first, 200)
            )));
        }
        tokens.insert(cmd.to_string(), (token.clone(), Instant::now()));
        Ok(token)
    }

    /// `body` with the provider's `extra_body` merged in.
    pub fn finish_body(&self, mut body: Value) -> Value {
        merge_json(&mut body, &self.p.extra_body);
        body
    }

    /// A Vertex AI model endpoint: `options.project` (else
    /// `GOOGLE_CLOUD_PROJECT`), `options.location` (else `CLOUD_ML_REGION`,
    /// else `global`); `base_url` replaces the regional host.
    pub fn vertex_url(
        &self,
        publisher: &str,
        model: &str,
        method: &str,
    ) -> Result<String, GatewayError> {
        let project = self
            .p
            .options
            .as_ref()
            .and_then(|o| o.project.as_deref())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| self.env("GOOGLE_CLOUD_PROJECT"))
            .or_else(|| self.env("ANTHROPIC_VERTEX_PROJECT_ID"))
            .ok_or_else(|| {
                GatewayError::Config(format!("provider `{}` needs options.project", self.p.id))
            })?;
        let location = self
            .p
            .options
            .as_ref()
            .and_then(|o| o.location.as_deref())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| self.env("CLOUD_ML_REGION"))
            .unwrap_or_else(|| "global".to_string());
        let host = match self.p.base() {
            Some(base) => base.to_string(),
            None if location == "global" => "https://aiplatform.googleapis.com".to_string(),
            None => format!("https://{location}-aiplatform.googleapis.com"),
        };
        Ok(format!(
            "{host}/v1/projects/{project}/locations/{location}/publishers/{publisher}/models/{model}:{method}"
        ))
    }
}

/// Deep-merges object `extra` into `base`; a `null` member deletes.
fn merge_json(base: &mut Value, extra: &Value) {
    let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) else {
        return;
    };
    for (key, value) in extra {
        match (base.get_mut(key), value) {
            (_, Value::Null) => {
                base.remove(key);
            }
            (Some(slot @ Value::Object(_)), Value::Object(_)) => merge_json(slot, value),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// A tool call's streamed input: `{}` when empty; `{}` plus a gateway meta
/// note (see [`INVALID_JSON`]) when it is not a JSON object.
pub(crate) fn parse_tool_input(raw: &str) -> (Value, Option<Value>) {
    if raw.trim().is_empty() {
        return (json!({}), None);
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(v @ Value::Object(_)) => (v, None),
        _ => (
            json!({}),
            Some(json!({"kind": "gateway", "error": INVALID_JSON, "raw": truncate(raw, 2000)})),
        ),
    }
}

/// The opaque state in `block` that `tag`'s provider kind may replay.
pub(crate) fn replayable<'b>(block: &'b Block, tag: &str) -> Option<&'b Value> {
    match block {
        Block::Opaque { kind, raw } if kind == tag => Some(raw),
        _ => None,
    }
}

/// `ToolUse.meta` if it belongs to `tag`'s provider kind.
pub(crate) fn own_meta<'m>(meta: &'m Option<Value>, tag: &str) -> Option<&'m Value> {
    meta.as_ref()
        .filter(|m| m.get("kind").and_then(Value::as_str) == Some(tag))
}

/// The number of tool calls already in a conversation (for synthesized ids).
/// The `prompt_cache_key` a provider that takes one is sent: a digest of the system parts marked `cache`, which every
/// call of a project shares, so their calls share the key and its cache. `None` when no part is cached.
pub(crate) fn prompt_cache_key(req: &ChatRequest) -> Option<String> {
    let mut digest = Sha256::new();
    let mut cached = false;
    for part in req.system_parts.iter().filter(|part| part.cache) {
        digest.update(part.text.as_bytes());
        digest.update([0]);
        cached = true;
    }
    cached.then(|| {
        digest.finalize()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    })
}

pub(crate) fn tool_use_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, Block::ToolUse { .. }))
        .count()
}

/// Tool results first: providers want them ahead of any text in a message.
pub(crate) fn results_first(blocks: &[Block]) -> Vec<&Block> {
    let (mut results, rest): (Vec<_>, Vec<_>) = blocks
        .iter()
        .partition(|b| matches!(b, Block::ToolResult { .. }));
    results.extend(rest);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A secret source that holds nothing, and says why (`Missing` when it says nothing more).
    struct Refusing(Option<SecretReason>);

    impl SecretSource for Refusing {
        fn get(&self, _name: &str, _read: &SecretRead<'_>) -> Option<String> {
            None
        }

        fn fetch(&self, _name: &str, _read: &SecretRead<'_>) -> Result<String, SecretReason> {
            Err(self.0.clone().unwrap_or(SecretReason::Missing))
        }
    }

    /// A secret source that refuses with `reason`, counting each time the gateway asks it.
    struct Counting {
        asked: Arc<std::sync::atomic::AtomicUsize>,
        reason: SecretReason,
    }

    impl SecretSource for Counting {
        fn get(&self, _name: &str, _read: &SecretRead<'_>) -> Option<String> {
            None
        }

        fn fetch(&self, _name: &str, _read: &SecretRead<'_>) -> Result<String, SecretReason> {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(self.reason.clone())
        }
    }

    #[test]
    fn a_refused_secret_is_asked_for_once_and_the_call_says_why() {
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source = Counting {
            asked: Arc::clone(&asked),
            reason: SecretReason::Sealed,
        };
        let mut p = ProviderConfig::new("claude", ProviderKind::Anthropic);
        p.auth = Auth::Secret("CLAUDE_KEY".to_string());
        let gw = Gateway::builder()
            .providers(vec![p])
            .secrets(source)
            .build();
        let req = ChatRequest {
            system_parts: Vec::new(),
            cache_last_message: false,
            model: "claude/m".to_string(),
            ..crate::testutil::request("hi")
        };
        let err = gw.complete(&req, &mut |_| {}).unwrap_err();
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(err.to_string(), "vault sealed");
    }

    #[test]
    fn a_sealed_vault_does_not_fall_back_to_the_process_environment() {
        // The environment lookup answers a value; a sealed vault must still refuse, not use it.
        let mut p = ProviderConfig::new("claude", ProviderKind::Anthropic);
        p.auth = Auth::Env("GW_SEALED_FALLBACK_KEY_4417".to_string());
        let gw = Gateway::builder()
            .providers(vec![p])
            .secrets(Refusing(Some(SecretReason::Sealed)))
            .env(|_| Some("PROCESS-VALUE-4417".to_string()))
            .build();
        let req = ChatRequest {
            system_parts: Vec::new(),
            cache_last_message: false,
            model: "claude/m".to_string(),
            ..crate::testutil::request("hi")
        };
        let err = gw.complete(&req, &mut |_| {}).unwrap_err();
        assert_eq!(err.to_string(), "vault sealed");
    }

    /// A call to an Anthropic provider whose named key the source refuses.
    fn refused_call(source: Refusing) -> GatewayError {
        let mut p = ProviderConfig::new("claude", ProviderKind::Anthropic);
        p.auth = Auth::Secret("CLAUDE_KEY".to_string());
        let gw = Gateway::builder()
            .providers(vec![p])
            .secrets(source)
            .build();
        let req = ChatRequest {
            system_parts: Vec::new(),
            cache_last_message: false,
            model: "claude/m".to_string(),
            ..crate::testutil::request("hi")
        };
        gw.complete(&req, &mut |_| {}).unwrap_err()
    }

    #[test]
    fn a_refused_secret_fails_the_call_with_the_reason_it_gave() {
        // A sealed vault fails the call with its own text, not as an unset secret.
        {
            let err = refused_call(Refusing(Some(SecretReason::Sealed)));
            assert!(
                matches!(&err, GatewayError::Vault { status: 503, .. }),
                "{err:?}"
            );
            assert_eq!(err.to_string(), "vault sealed");
        }

        // A tampered vault and a secret held elsewhere each fail with their own status.
        {
            let tampered = refused_call(Refusing(Some(SecretReason::Tampered(
                "the index MAC does not match".into(),
            ))));
            assert!(
                matches!(&tampered, GatewayError::Vault { status: 503, .. }),
                "{tampered:?}"
            );
            assert_eq!(
                tampered.to_string(),
                "vault tampered: the index MAC does not match"
            );
            let elsewhere = refused_call(Refusing(Some(SecretReason::OutOfScope)));
            assert!(
                matches!(&elsewhere, GatewayError::Vault { status: 403, .. }),
                "{elsewhere:?}"
            );
            assert_eq!(
                elsewhere.to_string(),
                "that secret is not scoped to this node"
            );
        }

        // Without a reason the call still says the secret is not set.
        {
            let err = refused_call(Refusing(None));
            assert!(matches!(&err, GatewayError::Auth(_)), "{err:?}");
            assert!(
                err.to_string().contains("secret `CLAUDE_KEY` is not set"),
                "{err}"
            );
        }
    }

    #[test]
    fn model_refs_split_provider_model_and_effort() {
        // Model names keep their effort pin and reject malformed names.
        {
            let r = |s: &str| ModelRef::parse(s).unwrap();
            let pinned = r("anthropic/claude-opus-5-5@low");
            assert_eq!(
                (
                    pinned.provider.as_str(),
                    pinned.model.as_str(),
                    pinned.effort.as_deref()
                ),
                ("anthropic", "claude-opus-5-5", Some("low"))
            );
            assert_eq!(pinned.to_string(), "anthropic/claude-opus-5-5@low");
            assert_eq!(pinned.base_name(), "anthropic/claude-opus-5-5");
            assert_eq!(r("xai/grok-4.7@XHigh").to_string(), "xai/grok-4.7@xhigh");
            assert_eq!(r("openrouter/anthropic/claude").model, "anthropic/claude");
            // A Vertex version suffix is part of the model, not an effort.
            let vertex = r("vertex/claude-sonnet-4-5@20250929");
            assert_eq!(
                (vertex.model.as_str(), vertex.effort),
                ("claude-sonnet-4-5@20250929", None)
            );
            assert_eq!(
                r("vertex/claude-sonnet-4-5@20250929@max").effort.as_deref(),
                Some("max")
            );
            for bad in ["gpt-5", "openai/", "/gpt-5", "openai/@low"] {
                assert!(
                    matches!(ModelRef::parse(bad), Err(GatewayError::BadModelName(_))),
                    "{bad}"
                );
            }
        }

        // Nested model ids split on the first slash, and the pin survives the round trip.
        {
            let r = |s: &str| ModelRef::parse(s).unwrap();
            let plain = r("openrouter/anthropic/claude-x");
            assert_eq!(
                (
                    plain.provider.as_str(),
                    plain.model.as_str(),
                    plain.effort.as_deref()
                ),
                ("openrouter", "anthropic/claude-x", None)
            );
            let free = r("openrouter/meta-llama/llama-3.3-70b-instruct:free");
            assert_eq!(free.model, "meta-llama/llama-3.3-70b-instruct:free");
            let zen = r("opencode/claude-x@high");
            assert_eq!(
                (
                    zen.provider.as_str(),
                    zen.model.as_str(),
                    zen.effort.as_deref()
                ),
                ("opencode", "claude-x", Some("high"))
            );
            let pinned = r("openrouter/anthropic/claude-x@xhigh");
            assert_eq!(
                (pinned.model.as_str(), pinned.effort.as_deref()),
                ("anthropic/claude-x", Some("xhigh"))
            );
            // Not an effort: the `@` stays part of the model.
            let bogus = r("openrouter/vendor/model@bogus");
            assert_eq!(
                (bogus.model.as_str(), bogus.effort.is_none()),
                ("vendor/model@bogus", true)
            );
            // The base name round-trips the nested id, and the pin survives Display.
            assert_eq!(pinned.base_name(), "openrouter/anthropic/claude-x");
            assert_eq!(
                free.base_name(),
                "openrouter/meta-llama/llama-3.3-70b-instruct:free"
            );
            assert_eq!(
                ModelRef::parse(&pinned.base_name()).unwrap().base_name(),
                pinned.base_name()
            );
            assert_eq!(pinned.to_string(), "openrouter/anthropic/claude-x@xhigh");
        }
    }

    #[test]
    fn invalid_tool_json_is_flagged() {
        let (input, meta) = parse_tool_input(r#"{"path": "a"#);
        assert_eq!(input, json!({}));
        assert_eq!(tool_input_error(meta.as_ref()), Some(INVALID_JSON));
        assert_eq!(parse_tool_input(r#"{"a":1}"#), (json!({"a": 1}), None));
    }

    fn ok_stream() -> testutil::Reply {
        testutil::Reply::sse(testutil::sse_data(&[
            r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]))
    }

    #[test]
    fn retryable_statuses_are_retried_then_reported() {
        use testutil::{FakeServer, Reply, gateway, provider, request};
        let busy = || Reply::json(503, &json!({"error": "busy"})).header("retry-after", "0");
        let server = FakeServer::start(vec![
            busy(),
            busy(),
            busy(),
            Reply::json(429, &json!({})),
            ok_stream(),
        ]);
        let gw = gateway(vec![provider(
            "o",
            ProviderKind::OpenAiCompatible,
            &server,
            Auth::None,
        )]);
        let req = ChatRequest {
            system_parts: Vec::new(),
            cache_last_message: false,
            model: "o/m".into(),
            ..request("hi")
        };
        let err = gw.complete(&req, &mut |_| {}).unwrap_err();
        assert!(
            matches!(
                &err,
                GatewayError::Http {
                    status: 503,
                    retryable: true,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(err.is_retryable());
        assert_eq!(server.requests().len(), 3, "one call plus two retries");
        assert_eq!(gw.complete(&req, &mut |_| {}).unwrap().text(), "ok");
        assert_eq!(server.requests().len(), 5);
    }

    #[test]
    fn secrets_never_appear_in_errors() {
        use testutil::{FakeServer, Reply, gateway, provider, request};
        let leak = json!({"error": {"message": "Incorrect API key provided: sk-secret-value"}});
        let server = FakeServer::start(vec![Reply::json(401, &leak)]);
        let gw = gateway(vec![
            provider("o", ProviderKind::OpenAi, &server, Auth::Secret("s".into())),
            provider(
                "g",
                ProviderKind::Gemini,
                &server,
                Auth::Secret("missing".into()),
            ),
        ]);
        let err = gw
            .complete(
                &ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    model: "o/gpt-6-luna".into(),
                    ..request("hi")
                },
                &mut |_| {},
            )
            .unwrap_err();
        let shown = err.to_string();
        assert!(
            shown.contains("401")
                && shown.contains("[redacted]")
                && !shown.contains("sk-secret-value"),
            "{shown}"
        );
        assert!(!format!("{err:?}").contains("sk-secret-value"));
        assert_eq!(server.requests().len(), 1, "401 is not retried");
        let missing = gw
            .complete(
                &ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    model: "g/gemini-3-pro".into(),
                    ..request("hi")
                },
                &mut |_| {},
            )
            .unwrap_err();
        assert_eq!(
            missing,
            GatewayError::Auth("secret `missing` is not set (provider `g`)".into())
        );
        assert!(matches!(
            gw.complete(
                &ChatRequest {
                    system_parts: Vec::new(),
                    cache_last_message: false,
                    model: "nope/m".into(),
                    ..request("hi")
                },
                &mut |_| {}
            ),
            Err(GatewayError::UnknownProvider(_))
        ));
    }

    #[test]
    fn extra_body_deep_merges_and_null_deletes() {
        let mut body = json!({"a": {"x": 1, "y": 2}, "stream_options": {}, "k": 1});
        merge_json(
            &mut body,
            &json!({"a": {"y": 3}, "stream_options": null, "z": true}),
        );
        assert_eq!(body, json!({"a": {"x": 1, "y": 3}, "k": 1, "z": true}));
    }

    /// A conversation whose user turn is text and an image.
    fn with_image(model: &str) -> ChatRequest {
        ChatRequest {
            messages: vec![Message {
                role: apz_proto::chat::Role::User,
                content: vec![
                    Block::Text {
                        text: "look".into(),
                    },
                    Block::Image {
                        media_type: "image/png".into(),
                        data: "iVBORw0KGgo=".into(),
                    },
                ],
            }],
            model: model.into(),
            ..crate::testutil::request("unused")
        }
    }

    #[test]
    fn an_image_is_refused_by_a_wire_format_with_no_image_block_before_any_request_is_sent() {
        let server =
            crate::testutil::FakeServer::start(vec![crate::testutil::Reply::json(400, &json!({}))]);
        let p = crate::testutil::provider(
            "g",
            ProviderKind::Gemini,
            &server,
            crate::config::Auth::Env("GEMINI_API_KEY".into()),
        );
        let err = crate::testutil::gateway(vec![p])
            .complete(&with_image("g/gemini-test"), &mut |_| {})
            .unwrap_err();
        assert!(
            matches!(&err, GatewayError::Unsupported(why) if why.contains("takes no images")),
            "{err}"
        );
        assert!(
            server.requests().is_empty(),
            "the refused call sends no request"
        );
    }

    #[test]
    fn the_mock_takes_an_image() {
        let gw = Gateway::builder()
            .providers(vec![ProviderConfig::new("m", ProviderKind::Mock)])
            .build();
        assert!(gw.complete(&with_image("m/echo"), &mut |_| {}).is_ok());
    }
}
