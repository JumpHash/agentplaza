//! Provider configuration (`apid`'s `providers.json`) and the presets the
//! "add provider" form offers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which API a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
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
    #[serde(rename = "mock")]
    Mock,
}

impl ProviderKind {
    /// Every kind, in the order the contract lists them.
    pub const ALL: [ProviderKind; 10] = [
        ProviderKind::Anthropic,
        ProviderKind::OpenAi,
        ProviderKind::OpenAiResponses,
        ProviderKind::AzureOpenAi,
        ProviderKind::Gemini,
        ProviderKind::Bedrock,
        ProviderKind::VertexAnthropic,
        ProviderKind::VertexGemini,
        ProviderKind::OpenAiCompatible,
        ProviderKind::Mock,
    ];

    /// The JSON name (`"openai-responses"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::OpenAiResponses => "openai-responses",
            ProviderKind::AzureOpenAi => "azure-openai",
            ProviderKind::Gemini => "gemini",
            ProviderKind::Bedrock => "bedrock",
            ProviderKind::VertexAnthropic => "vertex-anthropic",
            ProviderKind::VertexGemini => "vertex-gemini",
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::Mock => "mock",
        }
    }
}

/// Where a provider's credential comes from.
///
/// JSON: `"none"`, `{"env": "NAME"}`, `{"secret": "NAME"}` or
/// `{"command": "gcloud auth print-access-token"}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// One configured provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    /// Overrides the kind's default endpoint (required for `openai-compatible`
    /// and `azure-openai`; for `bedrock` and the Vertex kinds it replaces the
    /// regional host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth: Auth,
    /// Added to every request (replacing a header of the same name).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<String, String>,
    /// An object deep-merged into every request body; a `null` member removes
    /// that member from the body.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub extra_body: Value,
    /// Models to offer even when the API has no listing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Kind-specific settings: `region` (bedrock), `project` and `location`
    /// (vertex), `max_tokens_field` and `reasoning_effort: true` (Chat
    /// Completions kinds), `refusal_fallbacks: true` and `first_party`
    /// (anthropic; the latter for a pass-through proxy).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub options: Value,
}

impl ProviderConfig {
    /// A provider with defaults for everything but its id and kind.
    pub fn new(id: impl Into<String>, kind: ProviderKind) -> Self {
        ProviderConfig {
            id: id.into(),
            kind,
            base_url: None,
            auth: Auth::None,
            extra_headers: BTreeMap::new(),
            extra_body: Value::Null,
            models: Vec::new(),
            options: Value::Null,
        }
    }

    /// A string option (`options.<name>`), if set and non-empty.
    pub(crate) fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    /// The base URL without a trailing slash.
    pub(crate) fn base(&self) -> Option<&str> {
        self.base_url.as_deref().map(|b| b.trim_end_matches('/')).filter(|b| !b.is_empty())
    }
}

/// A ready-made provider for the "add provider" form.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub kind: ProviderKind,
    pub label: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<&'static str>,
    pub auth: Auth,
    pub notes: &'static str,
    /// Models to list statically (the plaza's supported models for this API).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<&'static str>,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub options: Value,
}

impl Preset {
    /// The provider this preset describes, under the preset's id.
    pub fn config(&self) -> ProviderConfig {
        ProviderConfig {
            base_url: self.base_url.map(str::to_string),
            auth: self.auth.clone(),
            models: self.models.iter().map(|m| m.to_string()).collect(),
            options: self.options.clone(),
            ..ProviderConfig::new(self.id, self.kind)
        }
    }

    fn with(mut self, models: &[&'static str], options: Value) -> Self {
        self.models = models.to_vec();
        self.options = options;
        self
    }
}

/// Every preset, first-party APIs first.
pub fn presets() -> Vec<Preset> {
    use ProviderKind::*;
    let env = |name: &str| Auth::Env(name.to_string());
    let gcloud = || Auth::Command("gcloud auth print-access-token".to_string());
    let p = |id, kind, label, base_url, auth, notes| Preset {
        id,
        kind,
        label,
        base_url,
        auth,
        notes,
        models: Vec::new(),
        options: Value::Null,
    };
    const OPENAI_MODELS: &[&str] = &["gpt-6-astra", "gpt-6-luna", "gpt-5.6-terra"];
    vec![
        p("anthropic", Anthropic, "Anthropic", Some("https://api.anthropic.com"), env("ANTHROPIC_API_KEY"),
          "Claude via the Messages API. Use {\"env\": \"ANTHROPIC_AUTH_TOKEN\"} or a command for a bearer token. \
           options.refusal_fallbacks: true enables server-side refusal fallbacks.")
            .with(&["claude-opus-5-5", "claude-haiku-5-5"], Value::Null),
        p("openai", OpenAi, "OpenAI", Some("https://api.openai.com/v1"), env("OPENAI_API_KEY"), "Chat Completions.")
            .with(OPENAI_MODELS, Value::Null),
        p("openai-responses", OpenAiResponses, "OpenAI (Responses API)", Some("https://api.openai.com/v1"),
          env("OPENAI_API_KEY"), "Responses API, stateless (store: false) with encrypted reasoning replay.")
            .with(OPENAI_MODELS, Value::Null),
        p("azure-openai", AzureOpenAi, "Azure OpenAI", None, env("AZURE_OPENAI_API_KEY"),
          "base_url https://<resource>.openai.azure.com; the model id is the deployment name; list deployments in `models`."),
        p("gemini", Gemini, "Google Gemini", Some("https://generativelanguage.googleapis.com"), env("GEMINI_API_KEY"),
          "Gemini API (AI Studio key)."),
        p("bedrock", Bedrock, "Amazon Bedrock", None, Auth::None,
          "Converse API. options.region (else AWS_REGION); AWS_BEARER_TOKEN_BEDROCK or AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY."),
        p("vertex-anthropic", VertexAnthropic, "Claude on Vertex AI", None, gcloud(),
          "options.project (else GOOGLE_CLOUD_PROJECT) and options.location (default global); list models in `models`."),
        p("vertex-gemini", VertexGemini, "Gemini on Vertex AI", None, gcloud(),
          "options.project (else GOOGLE_CLOUD_PROJECT) and options.location (default global); list models in `models`."),
        p("deepseek", OpenAiCompatible, "DeepSeek", Some("https://api.deepseek.com/v1"), env("DEEPSEEK_API_KEY"), ""),
        p("groq", OpenAiCompatible, "Groq", Some("https://api.groq.com/openai/v1"), env("GROQ_API_KEY"), ""),
        p("mistral", OpenAiCompatible, "Mistral", Some("https://api.mistral.ai/v1"), env("MISTRAL_API_KEY"), ""),
        p("xai", OpenAiCompatible, "xAI", Some("https://api.x.ai/v1"), env("XAI_API_KEY"),
          "Effort goes out as reasoning_effort (low, medium, high, xhigh).")
            .with(&["grok-4.7"], serde_json::json!({"reasoning_effort": true})),
        p("openrouter", OpenAiCompatible, "OpenRouter", Some("https://openrouter.ai/api/v1"), env("OPENROUTER_API_KEY"), ""),
        p("together", OpenAiCompatible, "Together AI", Some("https://api.together.xyz/v1"), env("TOGETHER_API_KEY"), ""),
        p("fireworks", OpenAiCompatible, "Fireworks AI", Some("https://api.fireworks.ai/inference/v1"),
          env("FIREWORKS_API_KEY"), ""),
        p("perplexity", OpenAiCompatible, "Perplexity", Some("https://api.perplexity.ai"), env("PERPLEXITY_API_KEY"),
          "No model listing: put the models in `models`."),
        p("cerebras", OpenAiCompatible, "Cerebras", Some("https://api.cerebras.ai/v1"), env("CEREBRAS_API_KEY"), ""),
        p("nvidia-nim", OpenAiCompatible, "NVIDIA NIM", Some("https://integrate.api.nvidia.com/v1"),
          env("NVIDIA_API_KEY"), ""),
        p("ollama", OpenAiCompatible, "Ollama", Some("http://127.0.0.1:11434/v1"), Auth::None, "Local; no key."),
        p("lmstudio", OpenAiCompatible, "LM Studio", Some("http://127.0.0.1:1234/v1"), Auth::None, "Local; no key."),
        p("vllm", OpenAiCompatible, "vLLM", Some("http://127.0.0.1:8000/v1"), Auth::None, "Local; no key."),
        p("llamacpp", OpenAiCompatible, "llama.cpp server", Some("http://127.0.0.1:8080/v1"), Auth::None,
          "Local; no key."),
        p("mock", Mock, "Mock (offline)", None, Auth::None,
          "Deterministic, no network: models `agent` (calls a tool, then summarises) and `echo`."),
    ]
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
        assert_eq!(cfg.option("region"), Some("eu-west-1"));
        let none: ProviderConfig = serde_json::from_value(json!({"id": "o", "kind": "openai-compatible"})).unwrap();
        assert_eq!(none.auth, Auth::None);
        assert_eq!(serde_json::to_value(&none).unwrap(), json!({"id": "o", "kind": "openai-compatible", "auth": "none"}));
        for kind in ProviderKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        }
        let ids: Vec<_> = presets().iter().map(|p| p.id).collect();
        assert!(ids.contains(&"ollama") && ids.contains(&"mock") && ids.len() == 23, "{ids:?}");
    }
}
