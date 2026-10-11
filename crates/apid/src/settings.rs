//! Plaza settings and the owner's model policy (`settings.json`).
//!
//! OWNER: slice "providers". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_http::Router;
use serde::{Deserialize, Serialize};

use crate::state::App;

/// An agent's tier: the orchestrator role, or everyone else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Orchestrator,
    Worker,
}

/// The role whose agents are the orchestrator tier.
pub const ORCHESTRATOR_ROLE: &str = "orchestrator";

pub fn tier_of(role: &str) -> Tier {
    // contract
    if role == ORCHESTRATOR_ROLE { Tier::Orchestrator } else { Tier::Worker }
}

/// The models each tier may run (canonical `provider/model[@effort]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPolicy {
    pub orchestrator: Vec<String>,
    pub worker: Vec<String>,
}

impl Default for ModelPolicy {
    fn default() -> Self {
        // The owner's rule, docs/architecture.md "Model policy".
        Self {
            orchestrator: vec![
                "anthropic/claude-opus-5-5".into(),
                "anthropic/claude-fable-5-1".into(),
                "openai/gpt-6-astra".into(),
                "xai/grok-4.7".into(),
            ],
            worker: vec![
                "deepseek/deepseek-flash".into(),
                "anthropic/claude-haiku-5-5".into(),
                "anthropic/claude-opus-5-5@low".into(),
                "openai/gpt-6-luna".into(),
                "openai/gpt-5.6-terra".into(),
                "xai/grok-4.7@low".into(),
            ],
        }
    }
}

/// Price per million tokens, in credits (1 credit = $0.001).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The worker tier's default model.
    pub default_model: String,
    pub orchestrator_model: String,
    pub role_models: BTreeMap<String, String>,
    pub model_policy: ModelPolicy,
    pub effort: Option<String>,
    pub max_turns: u32,
    pub max_tokens: u32,
    pub fetch_allow: Vec<String>,
    pub prices: BTreeMap<String, Price>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_model: "deepseek/deepseek-flash".into(),
            orchestrator_model: "anthropic/claude-opus-5-5".into(),
            role_models: BTreeMap::new(),
            model_policy: ModelPolicy::default(),
            effort: None,
            max_turns: 24,
            max_tokens: 0,
            fetch_allow: Vec::new(),
            prices: BTreeMap::new(),
        }
    }
}

pub struct Store {
    path: PathBuf,
    dev_models: bool,
    inner: Mutex<Settings>,
}

impl Store {
    /// Loads `settings.json` (defaults on first start; dev defaults in dev mode).
    pub fn load(state_dir: &Path, dev_models: bool) -> std::io::Result<Self> {
        // contract
        let path = state_dir.join("settings.json");
        let settings = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Ok(Self { path, dev_models, inner: Mutex::new(settings) })
    }

    pub fn get(&self) -> Settings {
        // contract
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn dev_models(&self) -> bool {
        // contract
        self.dev_models
    }

    /// Whether `model` may run for `tier` (always in dev mode); the error
    /// names the allowed list.
    pub fn allowed(&self, tier: Tier, model: &str) -> Result<(), String> {
        // contract — TODO(providers slice): canonicalize with apz_gateway::ModelRef
        let _ = (tier, model, &self.path);
        Ok(())
    }

    /// The model an agent of `role` runs: its own override, else the role's
    /// default, else the tier's default.
    pub fn resolve(&self, role: &str, agent_override: Option<&str>) -> String {
        // contract
        let s = self.get();
        if let Some(model) = agent_override {
            return model.to_string();
        }
        if let Some(model) = s.role_models.get(role) {
            return model.clone();
        }
        match tier_of(role) {
            Tier::Orchestrator => s.orchestrator_model,
            Tier::Worker => s.default_model,
        }
    }
}

/// `GET/PUT /v1/settings`.
pub fn routes(router: Router<App>) -> Router<App> {
    // TODO(providers slice)
    router
}
