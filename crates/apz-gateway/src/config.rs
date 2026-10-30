//! Provider configuration (`apid`'s `providers.json`) and the presets the
//! "add provider" form offers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which API a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, apz_store::Column)]
pub enum ProviderKind {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// A local Codex app-server using its existing login; tools stay in AgentPlaza.
    #[serde(rename = "codex-app-server")]
    CodexAppServer,
    /// A locally installed OpenCode process, with tool execution returned to AgentPlaza.
    #[serde(rename = "opencode-local")]
    OpenCodeLocal,
    #[serde(rename = "azure-openai")]
    AzureOpenAi,
    #[serde(rename = "gemini")]
    Gemini,
    #[serde(rename = "bedrock")]
    Bedrock,
    #[serde(rename = "vertex-anthropic")]
    VertexAnthropic,
    #[serde(rename = "vertex-gemini")]
    VertexGemini,
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    /// OpenAI's Decisions API (`POST {base}/decisions`).
    #[serde(rename = "openai-decisions")]
    OpenAiDecisions,
    /// TypeSafe Jev over the System One API (`POST {base}/v1/systemone`).
    #[serde(rename = "systemone")]
    SystemOne,
    /// Microsoft's decision models on Azure AI Foundry (System One shape,
    /// `api-key` header, `options.path`).
    #[serde(rename = "azure-decisions")]
    AzureDecisions,
    #[serde(rename = "mock")]
    Mock,
}

impl ProviderKind {
    /// Every kind, in the order the contract lists them.
    pub const ALL: [ProviderKind; 15] = [
        ProviderKind::Anthropic,
        ProviderKind::OpenAi,
        ProviderKind::OpenAiResponses,
        ProviderKind::CodexAppServer,
        ProviderKind::OpenCodeLocal,
        ProviderKind::AzureOpenAi,
        ProviderKind::Gemini,
        ProviderKind::Bedrock,
        ProviderKind::VertexAnthropic,
        ProviderKind::VertexGemini,
        ProviderKind::OpenAiCompatible,
        ProviderKind::OpenAiDecisions,
        ProviderKind::SystemOne,
        ProviderKind::AzureDecisions,
        ProviderKind::Mock,
    ];

    /// Whether the kind's wire format carries an image block: the Anthropic messages format (and Vertex's), and the
    /// OpenAI-compatible chat format with `image_url` data URLs. The mock takes images too, so scripts can see them.
    pub fn takes_images(self) -> bool {
        matches!(
            self,
            ProviderKind::Anthropic
                | ProviderKind::VertexAnthropic
                | ProviderKind::OpenAi
                | ProviderKind::OpenAiCompatible
                | ProviderKind::AzureOpenAi
                | ProviderKind::CodexAppServer
                | ProviderKind::Mock
        )
    }

    /// The JSON name (`"openai-responses"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::OpenAiResponses => "openai-responses",
            ProviderKind::CodexAppServer => "codex-app-server",
            ProviderKind::OpenCodeLocal => "opencode-local",
            ProviderKind::AzureOpenAi => "azure-openai",
            ProviderKind::Gemini => "gemini",
            ProviderKind::Bedrock => "bedrock",
            ProviderKind::VertexAnthropic => "vertex-anthropic",
            ProviderKind::VertexGemini => "vertex-gemini",
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::OpenAiDecisions => "openai-decisions",
            ProviderKind::SystemOne => "systemone",
            ProviderKind::AzureDecisions => "azure-decisions",
            ProviderKind::Mock => "mock",
        }
    }

    /// The model group this kind belongs to: `"decisions"` answers named
    /// questions, every other kind is `"chat"`. The two never mix.
    pub fn group(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiDecisions
            | ProviderKind::SystemOne
            | ProviderKind::AzureDecisions => "decisions",
            _ => "chat",
        }
    }
}

/// Where a provider's credential comes from.
///
/// JSON: `"none"`, `{"env": "NAME"}`, `{"secret": "NAME"}` or
/// `{"command": "gcloud auth print-access-token"}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    #[default]
    None,
    /// An environment variable of `apid`'s process.
    Env(String),
    /// A named entry of `apid`'s secret store.
    Secret(String),
    /// A shell command whose trimmed stdout is a bearer token (cached ~50 min).
    Command(String),
}

/// One configured provider: an instance of a kind. Several instances of one kind may exist (`openai`,
/// `openai_2`), each with its own key, label and accent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "provider")]
pub struct ProviderConfig {
    /// The instance id: letters, digits, `-` and `_`. It names the instance in model names (`<id>/<model>`).
    #[table(key)]
    pub id: String,
    pub kind: ProviderKind,
    /// The name the consoles show; the preset's label (or the id) when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// An optional marker colour for lists and the model picker, `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// A disabled instance keeps its settings and key but takes no calls and offers no models.
    #[serde(default = "enabled_default", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
    /// Overrides the kind's default endpoint (required for `openai-compatible`
    /// and `azure-openai`; for `bedrock` and the Vertex kinds it replaces the
    /// regional host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    #[table(flatten)]
    pub auth: Auth,
    /// Added to every request (replacing a header of the same name).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[table(map_value)]
    pub extra_headers: BTreeMap<String, String>,
    /// An object deep-merged into every request body; a `null` member removes
    /// that member from the body.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    #[table(content)]
    pub extra_body: Value,
    /// Models to offer even when the API has no listing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[table(list)]
    pub models: Vec<String>,
    /// Kind-specific settings (see [`ProviderOptions`]): `region` (bedrock), `project` and `location`
    /// (vertex), `max_tokens_field` and `reasoning_effort: true` (Chat
    /// Completions kinds), `refusal_fallbacks: true` and `first_party`
    /// (anthropic; the latter for a pass-through proxy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[table(child)]
    pub options: Option<ProviderOptions>,
}

/// A provider's kind-specific settings (`options` in its JSON): each key the gateway reads, typed. A key that is not
/// set is absent from the JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
pub struct ProviderOptions {
    /// Local provider executable (Codex app-server); defaults to `codex` on PATH.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Empty host working directory for local providers, never a guest workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    /// The AWS region (`bedrock`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// The Google Cloud project (`vertex-anthropic`, `vertex-gemini`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The Google Cloud location (`vertex-anthropic`, `vertex-gemini`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The request path of a System One API (`systemone`, `microsoft-decisions`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The Chat Completions field that carries the output cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<String>,
    /// Seconds a model listing is kept before it is listed again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_cache_secs: Option<u64>,
    /// Whether calls use the WebSocket mode (`openai-responses`; on unless set off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket: Option<bool>,
    /// Whether the assistant's reasoning is sent back with tool calls (DeepSeek).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_reasoning: Option<bool>,
    /// Whether the provider is Mistral's (its tool-call ids, and `max_tokens`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mistral_compat: Option<bool>,
    /// Whether effort goes out as `reasoning_effort` on Chat Completions requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<bool>,
    /// Whether the Anthropic endpoint is first-party (on for the first-party host; a pass-through proxy sets it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_party: Option<bool>,
    /// Whether refusals fall back to another model (`anthropic`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal_fallbacks: Option<bool>,
    /// The contributor models: models that train their provider's future models on what they are sent.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[table(list)]
    pub trains_on_data: Vec<String>,
}

fn enabled_default() -> bool {
    true
}
fn is_enabled(enabled: &bool) -> bool {
    *enabled
}

impl ProviderConfig {
    /// A provider with defaults for everything but its id and kind.
    pub fn new(id: impl Into<String>, kind: ProviderKind) -> Self {
        ProviderConfig {
            id: id.into(),
            kind,
            label: None,
            accent: None,
            enabled: true,
            base_url: None,
            auth: Auth::None,
            extra_headers: BTreeMap::new(),
            extra_body: Value::Null,
            models: Vec::new(),
            options: None,
        }
    }

    /// The base URL without a trailing slash.
    pub(crate) fn base(&self) -> Option<&str> {
        self.base_url
            .as_deref()
            .map(|b| b.trim_end_matches('/'))
            .filter(|b| !b.is_empty())
    }
}

/// A ready-made provider for the "add provider" form.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub kind: ProviderKind,
    pub label: &'static str,
    /// The logo the UI shows (`deepseek`, `ollama`, ...).
    pub brand: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<&'static str>,
    pub auth: Auth,
    pub notes: &'static str,
    /// Models to list statically (the plaza's supported models for this API).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<ProviderOptions>,
    /// Headers sent on every request (see `ProviderConfig::extra_headers`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<&'static str, &'static str>,
}

impl Preset {
    /// The provider this preset describes, under the preset's id.
    pub fn config(&self) -> ProviderConfig {
        ProviderConfig {
            base_url: self.base_url.map(str::to_string),
            auth: self.auth.clone(),
            extra_headers: self
                .extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            models: self.models.iter().map(|m| m.to_string()).collect(),
            options: self.options.clone(),
            ..ProviderConfig::new(self.id, self.kind)
        }
    }

    fn with(mut self, models: &[&'static str], options: Option<ProviderOptions>) -> Self {
        self.models = models.to_vec();
        self.options = options;
        self
    }

    fn headers(mut self, headers: &[(&'static str, &'static str)]) -> Self {
        self.extra_headers = headers.iter().copied().collect();
        self
    }
}

/// Every preset, first-party APIs first.
pub fn presets() -> Vec<Preset> {
    use ProviderKind::*;
    let env = |name: &str| Auth::Env(name.to_string());
    let gcloud = || Auth::Command("gcloud auth print-access-token".to_string());
    let p = |id, kind, label, brand, base_url, auth, notes| Preset {
        id,
        kind,
        label,
        brand,
        base_url,
        auth,
        notes,
        models: Vec::new(),
        options: None,
        extra_headers: BTreeMap::new(),
    };
    const OPENAI_MODELS: &[&str] = &["gpt-6-astra", "gpt-6-luna", "gpt-5.6-terra"];
    const OPENCODE_MODELS: &[&str] = &[
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "kimi-k3",
        "deepseek-v4-pro",
        "glm-5.3",
        "minimax-m3",
    ];
    vec![
        p("opencode-local", OpenCodeLocal, "OpenCode (local)", "opencode", None, Auth::None,
          "Uses the installed OpenCode with a scoped provider key. AgentPlaza executes tools returned by structured output.")
          .with(&["deepseek/deepseek-flash"], None),
        p("codex-local", CodexAppServer, "Codex (local login)", "openai", None, Auth::None,
          "Uses the installed Codex app-server and its existing login. AgentPlaza executes its tools; credentials stay with Codex."),
        p("anthropic", Anthropic, "Anthropic", "anthropic", Some("https://api.anthropic.com"), env("ANTHROPIC_API_KEY"),
          "Claude via the Messages API. Use {\"env\": \"ANTHROPIC_AUTH_TOKEN\"} or a command for a bearer token. \
           options.refusal_fallbacks: true enables server-side refusal fallbacks.")
            .with(&["claude-opus-5-5", "claude-fable-5-1", "claude-haiku-5-5"], None),
        p("openai", OpenAiResponses, "OpenAI", "openai", Some("https://api.openai.com/v1"), env("OPENAI_API_KEY"), "Responses API (current models need it for tools with reasoning).")
            .with(OPENAI_MODELS, None),
        p("openai-responses", OpenAiResponses, "OpenAI (Responses API)", "openai", Some("https://api.openai.com/v1"),
          env("OPENAI_API_KEY"), "Responses API, stateless (store: false) with encrypted reasoning replay.")
            .with(OPENAI_MODELS, None),
        p("azure-openai", AzureOpenAi, "Azure OpenAI", "azure", None, env("AZURE_OPENAI_API_KEY"),
          "base_url https://<resource>.openai.azure.com; the model id is the deployment name; list deployments in `models`."),
        p("gemini", Gemini, "Google Gemini", "google", Some("https://generativelanguage.googleapis.com"), env("GEMINI_API_KEY"),
          "Gemini API (AI Studio key)."),
        p("bedrock", Bedrock, "Amazon Bedrock", "aws", None, Auth::None,
          "Converse API. options.region (else AWS_REGION). Keys: a Bedrock API key as this provider's key (bearer), or AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY and optionally AWS_SESSION_TOKEN in the vault, bound to this provider's id: `apz vault set NAME --bind provider:<id>` (for the preset, provider:bedrock). Each Bedrock provider has its own pair."),
        p("vertex-anthropic", VertexAnthropic, "Claude on Vertex AI", "anthropic", None, gcloud(),
          "options.project (else GOOGLE_CLOUD_PROJECT) and options.location (default global); list models in `models`."),
        p("vertex-gemini", VertexGemini, "Gemini on Vertex AI", "google", None, gcloud(),
          "options.project (else GOOGLE_CLOUD_PROJECT) and options.location (default global); list models in `models`."),
        p("deepseek", OpenAiCompatible, "DeepSeek", "deepseek", Some("https://api.deepseek.com"), env("DEEPSEEK_API_KEY"),
          "Reasoning content is replayed, which DeepSeek requires once tools are in use. Effort goes out as reasoning_effort.")
            .with(&["deepseek-flash", "deepseek-v4-pro"], Some(ProviderOptions {
                replay_reasoning: Some(true),
                reasoning_effort: Some(true),
                ..Default::default()
            })),
        p("zai", OpenAiCompatible, "Z.ai GLM", "zai", Some("https://api.z.ai/api/paas/v4"), env("ZAI_API_KEY"),
          "Coding-plan keys use https://api.z.ai/api/coding/paas/v4. Effort goes out as reasoning_effort.")
            .with(&["glm-5.3", "glm-5.3-flash", "glm-5.3-flashx", "glm-5v-turbo"], Some(ProviderOptions { reasoning_effort: Some(true), ..Default::default() })),
        p("moonshot", OpenAiCompatible, "Moonshot Kimi", "moonshot", Some("https://api.moonshot.ai/v1"), env("MOONSHOT_API_KEY"),
          "Mainland China accounts use https://api.moonshot.cn/v1.")
            .with(&["kimi-k3"], None),
        p("qwen", OpenAiCompatible, "Alibaba Qwen", "alibaba", Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
          env("DASHSCOPE_API_KEY"),
          "DashScope's OpenAI-compatible mode, international endpoint; the China region is \
           https://dashscope.aliyuncs.com/compatible-mode/v1. The models come from the provider's /models listing."),
        p("groq", OpenAiCompatible, "Groq", "groq", Some("https://api.groq.com/openai/v1"), env("GROQ_API_KEY"), ""),
        p("mistral", OpenAiCompatible, "Mistral", "mistral", Some("https://api.mistral.ai/v1"), env("MISTRAL_API_KEY"),
          "Magistral's thinking comes back as reasoning; tool-call ids are mapped to Mistral's 9-character form.")
            .with(
                &["mistral-medium-latest", "mistral-large-latest", "magistral-medium-latest", "devstral-latest",
                  "codestral-latest", "mistral-small-latest"],
                Some(ProviderOptions {
                    mistral_compat: Some(true),
                    ..Default::default()
                }),
            ),
        p("xai", OpenAiCompatible, "xAI", "xai", Some("https://api.x.ai/v1"), env("XAI_API_KEY"),
          "Effort goes out as reasoning_effort (low, medium, high, xhigh).")
            .with(&["grok-4.7"], Some(ProviderOptions { reasoning_effort: Some(true), ..Default::default() })),
        p("meta", OpenAiCompatible, "Meta", "meta", Some("https://api.meta.ai/v1"), env("MODEL_API_KEY"),
          "Muse models over Meta's Model API (OpenAI-compatible). The contributor models train Meta's future models on \
           everything sent to and received from them: they are never defaults and need allow_training_models in the \
           settings. Effort goes out as reasoning_effort.")
            .with(
                &["muse-wonder-1.0", "muse-spark-1.3", "muse-spark-1.2", "muse-spark-1.1",
                  "muse-spark-1.3-contributor", "muse-spark-1.2-contributor"],
                Some(ProviderOptions {
                    reasoning_effort: Some(true),
                    trains_on_data: vec!["muse-spark-1.3-contributor".into(), "muse-spark-1.2-contributor".into()],
                    ..Default::default()
                }),
            ),
        p("openrouter", OpenAiCompatible, "OpenRouter", "openrouter", Some("https://openrouter.ai/api/v1"), env("OPENROUTER_API_KEY"), "")
            .headers(&[("HTTP-Referer", "https://agentplaza.io"), ("X-OpenRouter-Title", "AgentPlaza")])
            .with(&[], Some(ProviderOptions {
                models_cache_secs: Some(3600),
                ..Default::default()
            })),
        p("opencode", OpenAiCompatible, "OpenCode Zen", "opencode", Some("https://opencode.ai/inference/v1"), env("OPENCODE_API_KEY"),
          "The model list is live from /models; the static list is the fallback. The models are pay-as-you-go. \
           The free models in options.trains_on_data train on what they are sent and return.")
            .with(OPENCODE_MODELS, Some(ProviderOptions {
                trains_on_data: [
                    "big-pickle", "cat-jam-free", "exo-free", "mimo-v2.6-flash-free", "mimo-v2.5-free",
                    "ling-3.1-flash-free", "ling-3.0-flash-fin-free", "muse-spark-1.3-contributor-free",
                ]
                .map(String::from)
                .to_vec(),
                ..Default::default()
            })),
        p("together", OpenAiCompatible, "Together AI", "together", Some("https://api.together.xyz/v1"), env("TOGETHER_API_KEY"), ""),
        p("fireworks", OpenAiCompatible, "Fireworks AI", "fireworks", Some("https://api.fireworks.ai/inference/v1"),
          env("FIREWORKS_API_KEY"), ""),
        p("perplexity", OpenAiCompatible, "Perplexity", "perplexity", Some("https://api.perplexity.ai"), env("PERPLEXITY_API_KEY"),
          "No model listing: put the models in `models`."),
        p("cerebras", OpenAiCompatible, "Cerebras", "cerebras", Some("https://api.cerebras.ai/v1"), env("CEREBRAS_API_KEY"), ""),
        p("nvidia-nim", OpenAiCompatible, "NVIDIA NIM", "nvidia", Some("https://integrate.api.nvidia.com/v1"),
          env("NVIDIA_API_KEY"), ""),
        p("ollama", OpenAiCompatible, "Ollama", "ollama", Some("http://127.0.0.1:11434/v1"), Auth::None, "Local; no key."),
        p("lmstudio", OpenAiCompatible, "LM Studio", "lmstudio", Some("http://127.0.0.1:1234/v1"), Auth::None, "Local; no key."),
        p("vllm", OpenAiCompatible, "vLLM", "vllm", Some("http://127.0.0.1:8000/v1"), Auth::None, "Local; no key."),
        p("llamacpp", OpenAiCompatible, "llama.cpp server", "llamacpp", Some("http://127.0.0.1:8080/v1"), Auth::None,
          "Local; no key."),
        p("openai-decisions", OpenAiDecisions, "OpenAI Decisions", "openai", Some("https://api.openai.com/v1"),
          env("OPENAI_API_KEY"), "Decisions API: named predicates, choices and scores, each with calibrated probabilities.")
            .with(&["gpt-6-luna"], None),
        p("microsoft-decisions", AzureDecisions, "Microsoft Decision", "microsoft", None, env("AZURE_AI_API_KEY"),
          "Microsoft-Decision-1 on Azure AI Foundry. base_url https://<resource>.services.ai.azure.com; options.path defaults to /v1/systemone.")
            .with(&["Microsoft-Decision-1"], None),
        p("typesafe", SystemOne, "TypeSafe Jev", "typesafe", Some("https://api.typesafe.ai"), env("TYPESAFE_API_KEY"),
          "Jev decision models over the System One API.")
            .with(&["jev-latest", "jev-1.13.0"], None),
        p("mock", Mock, "Mock (offline)", "mock", None, Auth::None,
          "Deterministic, no network: models `agent` (calls a tool, then summarises) and `echo`."),
    ]
}

/// Whether `model` (a bare model id, no provider) is one of the contributor models a provider's
/// `options.trains_on_data` lists: models that train the provider's future models on what they are
/// sent and return.
pub fn lists_training_model(options: Option<&ProviderOptions>, model: &str) -> bool {
    options.is_some_and(|o| o.trains_on_data.iter().any(|name| name == model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_config_json_shape() {
        let cfg: ProviderConfig = serde_json::from_value(json!({
            "id": "aws", "kind": "bedrock", "auth": {"env": "AWS_BEARER_TOKEN_BEDROCK"},
            "options": {"region": "eu-west-1"}, "models": ["m"]
        }))
        .unwrap();
        assert_eq!(cfg.kind, ProviderKind::Bedrock);
        assert_eq!(cfg.auth, Auth::Env("AWS_BEARER_TOKEN_BEDROCK".into()));
        assert_eq!(
            cfg.options.as_ref().and_then(|o| o.region.as_deref()),
            Some("eu-west-1")
        );
        let none: ProviderConfig =
            serde_json::from_value(json!({"id": "o", "kind": "openai-compatible"})).unwrap();
        assert_eq!(none.auth, Auth::None);
        assert_eq!(
            serde_json::to_value(&none).unwrap(),
            json!({"id": "o", "kind": "openai-compatible", "auth": "none"})
        );
        for kind in ProviderKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        }
        let ids: Vec<_> = presets().iter().map(|p| p.id).collect();
        assert!(
            ids.contains(&"ollama")
                && ids.contains(&"mock")
                && ids.contains(&"meta")
                && ids.contains(&"codex-local")
                && ids.contains(&"opencode-local")
                && ids.len() == 33,
            "{ids:?}"
        );
        assert_eq!(ProviderKind::SystemOne.group(), "decisions");
        assert_eq!(ProviderKind::Mock.group(), "chat");
    }

    #[test]
    fn presets_have_unique_ids_and_a_known_brand() {
        const BRANDS: &[&str] = &[
            "anthropic",
            "openai",
            "google",
            "aws",
            "azure",
            "xai",
            "deepseek",
            "zai",
            "moonshot",
            "alibaba",
            "mistral",
            "groq",
            "openrouter",
            "opencode",
            "together",
            "fireworks",
            "perplexity",
            "cerebras",
            "nvidia",
            "ollama",
            "lmstudio",
            "vllm",
            "llamacpp",
            "mock",
            "microsoft",
            "typesafe",
            "meta",
        ];
        let presets = presets();
        let mut ids: Vec<_> = presets.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), presets.len());
        for p in &presets {
            assert!(
                BRANDS.contains(&p.brand),
                "{} has brand `{}`",
                p.id,
                p.brand
            );
        }
    }

    #[test]
    fn preset_endpoints_headers_and_models_are_pinned() {
        // The DeepSeek, Z.ai and Moonshot presets pin their endpoints, options and key variable.
        {
            let by_id = |id: &str| presets().into_iter().find(|p| p.id == id).unwrap();
            let ds = by_id("deepseek");
            assert_eq!(ds.base_url, Some("https://api.deepseek.com"));
            assert_eq!(ds.models, vec!["deepseek-flash", "deepseek-v4-pro"]);
            assert_eq!(
                ds.options,
                Some(ProviderOptions {
                    replay_reasoning: Some(true),
                    reasoning_effort: Some(true),
                    ..ProviderOptions::default()
                })
            );
            assert_eq!(by_id("zai").base_url, Some("https://api.z.ai/api/paas/v4"));
            let kimi = by_id("moonshot");
            assert_eq!(
                (kimi.base_url, kimi.auth),
                (
                    Some("https://api.moonshot.ai/v1"),
                    Auth::Env("MOONSHOT_API_KEY".into())
                )
            );
        }

        // The OpenRouter and OpenCode presets pin their endpoints, headers, models and training flags.
        {
            let by_id = |id: &str| presets().into_iter().find(|p| p.id == id).unwrap();
            let or = by_id("openrouter");
            assert_eq!(
                (or.kind, or.base_url, or.brand),
                (
                    ProviderKind::OpenAiCompatible,
                    Some("https://openrouter.ai/api/v1"),
                    "openrouter"
                )
            );
            assert_eq!(or.auth, Auth::Env("OPENROUTER_API_KEY".into()));
            assert!(or.models.is_empty());
            assert_eq!(
                or.options.as_ref().and_then(|o| o.models_cache_secs),
                Some(3600)
            );
            let or_cfg = or.config();
            assert_eq!(or_cfg.extra_headers.len(), 2);
            assert_eq!(
                or_cfg.extra_headers.get("HTTP-Referer").map(String::as_str),
                Some("https://agentplaza.io")
            );
            assert_eq!(
                or_cfg
                    .extra_headers
                    .get("X-OpenRouter-Title")
                    .map(String::as_str),
                Some("AgentPlaza")
            );
            let oc = by_id("opencode");
            assert_eq!(
                (oc.kind, oc.base_url, oc.brand),
                (
                    ProviderKind::OpenAiCompatible,
                    Some("https://opencode.ai/inference/v1"),
                    "opencode"
                )
            );
            assert_eq!(oc.auth, Auth::Env("OPENCODE_API_KEY".into()));
            assert_eq!(oc.models.len(), 10);
            assert!(oc.models.contains(&"claude-opus-5-5") && oc.models.contains(&"minimax-m3"));
            assert!(lists_training_model(oc.options.as_ref(), "big-pickle"));
            assert!(!lists_training_model(oc.options.as_ref(), "gpt-5.6-sol"));
            assert!(oc.config().extra_headers.is_empty());
            let oc_json = serde_json::to_value(&oc).unwrap();
            assert!(oc_json.get("extra_headers").is_none(), "{oc_json}");
        }
    }

    #[test]
    fn the_compatible_presets_pass_a_request_and_a_listing_in_their_wire_format() {
        use crate::testutil::{FakeServer, Reply, gateway, request, sse_data};
        use apz_proto::chat::ChatRequest;
        // The OpenAI-compatible presets of PLAN 11.3, each against a fake server that records what it is sent. A
        // chat goes to `<base>/chat/completions` with the preset's key as a bearer token (none for a local server),
        // naming the model without its provider id; the models are listed from `<base>/models`.
        const PRESETS: &[&str] = &[
            "xai", "deepseek", "mistral", "moonshot", "zai", "qwen", "vllm", "llamacpp",
        ];
        let stream = sse_data(&[
            r#"{"model":"wire-test-model","choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        for id in PRESETS {
            let preset = presets()
                .into_iter()
                .find(|p| p.id == *id)
                .unwrap_or_else(|| panic!("{id} is a preset"));
            assert_eq!(preset.kind, ProviderKind::OpenAiCompatible, "{id}");
            // The preset's path prefix (its `/v1`, or `/api/paas/v4`) stays; the host becomes the fake server.
            let base = preset.base_url.expect("a hosted or local base");
            let prefix = base
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(_, path)| format!("/{path}"))
                .unwrap_or_default();
            let server = FakeServer::start(vec![
                Reply::sse(stream.clone()),
                Reply::json(200, &json!({"data": [{"id": "listed-model"}]})),
            ]);
            let mut cfg = preset.config();
            cfg.base_url = Some(format!("{}{prefix}", server.url()));
            let gw = gateway(vec![cfg]);
            let req = ChatRequest {
                system_parts: Vec::new(),
                cache_last_message: false,
                model: format!("{id}/wire-test-model"),
                max_tokens: 16,
                ..request("hi")
            };
            let resp = gw.complete(&req, &mut |_| {}).expect("the chat answers");
            assert!(format!("{:?}", resp.content).contains("ok"), "{id}");
            let listed = gw.list_models(id).expect("the listing answers");
            assert!(
                format!("{listed:?}").contains("listed-model"),
                "{id}: {listed:?}"
            );

            let sent = server.requests();
            assert_eq!(sent.len(), 2, "{id}: one chat, one listing");
            assert_eq!(sent[0].path, format!("{prefix}/chat/completions"), "{id}");
            assert_eq!(sent[0].json()["model"], json!("wire-test-model"), "{id}");
            assert_eq!(sent[1].method, "GET", "{id}");
            assert_eq!(sent[1].path, format!("{prefix}/models"), "{id}");
            match &preset.auth {
                Auth::Env(name) => assert_eq!(
                    sent[0].header("authorization"),
                    Some(format!("Bearer env-{name}").as_str()),
                    "{id} sends its key"
                ),
                _ => assert!(
                    sent[0].header("authorization").is_none(),
                    "{id} sends no key"
                ),
            }
        }
    }

    #[test]
    fn meta_preset_flags_its_contributor_models() {
        let meta = presets().into_iter().find(|p| p.id == "meta").unwrap();
        assert_eq!(
            (meta.kind, meta.base_url, meta.brand),
            (
                ProviderKind::OpenAiCompatible,
                Some("https://api.meta.ai/v1"),
                "meta"
            )
        );
        assert_eq!(meta.auth, Auth::Env("MODEL_API_KEY".into()));
        assert!(
            meta.models.contains(&"muse-wonder-1.0")
                && !meta.models.contains(&"muse-wonder-1.0-contributor")
        );
        assert!(lists_training_model(
            meta.options.as_ref(),
            "muse-spark-1.3-contributor"
        ));
        assert!(lists_training_model(
            meta.options.as_ref(),
            "muse-spark-1.2-contributor"
        ));
        assert!(!lists_training_model(
            meta.options.as_ref(),
            "muse-spark-1.3"
        ));
        assert!(!lists_training_model(
            meta.options.as_ref(),
            "muse-wonder-1.0"
        ));
        assert!(!lists_training_model(None, "muse-spark-1.3-contributor"));
    }
}
