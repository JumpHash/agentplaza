//! The providers & models routes on the hub, typed for the desktop: API provider instances
//! (`/v1/providers`), coding-engine instances (`/v1/engines`), the ACP registry, usage providers (CLIProxyAPI
//! hubs) and the health checks. The same contract as the web's `api-providers.ts`. Fetches only; draws nothing.
//!
//! An engine runs only inside an agent's VM and reaches models only through the plaza gateway, so no key ever
//! goes into an engine: subscription accounts come in through a CLIProxyAPI hub added as an API provider.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::client::Client;

/// A check's result. `status`: `ok`, `warn`, `crit`, `off` (disabled) or `unknown` (not checked yet).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Health {
    pub status: String,
    pub message: String,
    pub checked_ms: Option<u64>,
    pub latency_ms: Option<u64>,
    pub models: Option<u64>,
    pub route: Option<String>,
}

impl Health {
    /// The kit tone the status draws with.
    pub fn tone(&self) -> crate::console::ui::Tone {
        use crate::console::ui::Tone;
        match self.status.as_str() {
            "ok" => Tone::Ok,
            "warn" => Tone::Warn,
            "crit" => Tone::Danger,
            _ => Tone::Muted,
        }
    }
}

/// An API provider instance. Secret values never appear: header values and command auth are redacted.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ApiInstance {
    pub id: String,
    pub kind: String,
    pub group: String,
    pub label: String,
    pub accent: Option<String>,
    pub enabled: bool,
    pub health: Health,
    pub base_url: Option<String>,
    pub auth: Value,
    pub models: Vec<String>,
    pub options: Value,
    pub extra_headers: BTreeMap<String, String>,
    pub extra_body: Value,
    /// `env`, `secret`, `none`, `missing`, `command`, `sealed` or `out_of_scope`.
    pub credential: String,
}

impl ApiInstance {
    /// The body `PUT /v1/providers/{id}` takes: this instance with `change` applied (the hub restores the
    /// redacted values).
    pub fn put_body(&self, change: impl FnOnce(&mut Value)) -> Value {
        let mut body = json!({
            "id": self.id, "kind": self.kind, "label": self.label, "accent": self.accent,
            "enabled": self.enabled, "base_url": self.base_url, "auth": self.auth, "models": self.models,
            "options": self.options, "extra_headers": self.extra_headers, "extra_body": self.extra_body,
        });
        change(&mut body);
        body
    }
}

/// A coding-engine instance: a built-in engine, or one the owner added (`acp:<registry id>`, `acp-local`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EngineInstance {
    pub id: String,
    pub engine: String,
    pub builtin: bool,
    pub label: String,
    pub accent: Option<String>,
    pub enabled: bool,
    /// `npm`, `release`, `pip`, `command` or `catalogue`.
    pub source: String,
    pub package: Option<String>,
    pub pinned: Option<String>,
    pub version: Option<String>,
    pub latest: Option<String>,
    pub install: Value,
    pub args: Vec<String>,
    /// Plain variables only: the agent can read them, so the hub refuses secrets here.
    pub env: BTreeMap<String, String>,
    pub home: String,
    pub route: Option<String>,
    pub about: Value,
    pub health: Health,
}

/// The fields `POST /v1/engines` and `PUT /v1/engines/{id}` take; `None` keeps a field's value.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EngineEdit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// An npm engine's version, `latest`, or `pinned`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub website: Option<String>,
    pub repository: Option<String>,
    pub icon: Option<String>,
    pub license: Option<String>,
    pub distribution: Option<String>,
    pub runs_in_vm: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Registry {
    pub fetched_ms: u64,
    pub agents: Vec<RegistryAgent>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CatalogEngine {
    pub id: String,
    pub label: String,
    pub source: String,
    pub package: String,
    pub pinned: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CatalogApi {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub brand: String,
    pub base_url: Option<String>,
    pub auth: Value,
    pub notes: String,
    pub models: Vec<String>,
}

/// What the Add provider dialog offers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Catalog {
    pub api: Vec<CatalogApi>,
    pub engines: Vec<CatalogEngine>,
    pub registry: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UsageTotals {
    pub requests: u64,
    pub success: u64,
    pub failure: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UsageModel {
    pub model: String,
    pub requests: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UsageAccount {
    pub name: String,
    pub provider: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UsageReading {
    pub status: String,
    pub message: String,
    pub checked_ms: u64,
    pub totals: Option<UsageTotals>,
    pub models: Vec<UsageModel>,
    pub accounts: Vec<UsageAccount>,
}

/// A usage provider (a CLIProxyAPI hub). Its management key stays in the hub's vault.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UsageProvider {
    pub id: String,
    pub kind: String,
    pub url: String,
    pub label: String,
    pub key: String,
    pub last: Option<UsageReading>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EngineChange {
    pub id: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EngineUpdate {
    pub dry_run: bool,
    pub changes: Vec<EngineChange>,
}

fn enc(id: &str) -> String {
    id.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// API instances
pub fn list(c: &Client) -> Result<Vec<ApiInstance>, String> {
    c.get("/v1/providers")
}
pub fn create(c: &Client, body: &Value) -> Result<ApiInstance, String> {
    c.post("/v1/providers", body)
}
pub fn replace(c: &Client, id: &str, body: &Value) -> Result<ApiInstance, String> {
    c.put(&format!("/v1/providers/{}", enc(id)), body)
}
pub fn remove(c: &Client, id: &str) -> Result<Value, String> {
    c.delete(&format!("/v1/providers/{}", enc(id)))
}
pub fn set_key(c: &Client, id: &str, value: &str) -> Result<ApiInstance, String> {
    c.put(
        &format!("/v1/providers/{}/secret", enc(id)),
        &json!({ "value": value }),
    )
}
pub fn check(c: &Client, id: &str) -> Result<Value, String> {
    c.post(&format!("/v1/providers/{}/check", enc(id)), &json!({}))
}
pub fn check_all(c: &Client) -> Result<BTreeMap<String, Health>, String> {
    c.post("/v1/providers/check", &json!({}))
}
pub fn catalog(c: &Client) -> Result<Catalog, String> {
    c.get("/v1/provider-catalog")
}

// Engines
pub fn engines(c: &Client) -> Result<Vec<EngineInstance>, String> {
    c.get("/v1/engines")
}
pub fn add_engine(c: &Client, edit: &EngineEdit) -> Result<EngineInstance, String> {
    c.post(
        "/v1/engines",
        &serde_json::to_value(edit).unwrap_or_default(),
    )
}
pub fn edit_engine(c: &Client, id: &str, edit: &EngineEdit) -> Result<EngineInstance, String> {
    c.put(
        &format!("/v1/engines/{}", enc(id)),
        &serde_json::to_value(edit).unwrap_or_default(),
    )
}
pub fn remove_engine(c: &Client, id: &str) -> Result<Value, String> {
    c.delete(&format!("/v1/engines/{}", enc(id)))
}
pub fn check_engines(c: &Client) -> Result<BTreeMap<String, Health>, String> {
    c.post("/v1/engines/check", &json!({}))
}
pub fn update_engines(c: &Client, dry_run: bool) -> Result<EngineUpdate, String> {
    c.post("/v1/engines/update", &json!({ "dry_run": dry_run }))
}
pub fn registry(c: &Client, refresh: bool) -> Result<Registry, String> {
    c.get(if refresh {
        "/v1/acp/registry?refresh=1"
    } else {
        "/v1/acp/registry"
    })
}

// Usage providers
pub fn usage(c: &Client) -> Result<Vec<UsageProvider>, String> {
    c.get("/v1/usage-providers")
}
pub fn add_usage(
    c: &Client,
    url: &str,
    key: &str,
    label: Option<&str>,
) -> Result<UsageProvider, String> {
    c.post(
        "/v1/usage-providers",
        &json!({ "kind": "cliproxyapi", "url": url, "key": key, "label": label }),
    )
}
pub fn read_usage(c: &Client, id: &str) -> Result<UsageReading, String> {
    c.get(&format!("/v1/usage-providers/{}/usage", enc(id)))
}
pub fn remove_usage(c: &Client, id: &str) -> Result<Value, String> {
    c.delete(&format!("/v1/usage-providers/{}", enc(id)))
}

/// The newest check time across instances, or `None` before any check: the "Checked just now" line.
pub fn last_checked<'a>(healths: impl IntoIterator<Item = &'a Health>) -> Option<u64> {
    healths.into_iter().filter_map(|h| h.checked_ms).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_parse_with_missing_fields() {
        let api: ApiInstance = serde_json::from_value(json!({"id": "openai", "kind": "openai", "label": "OpenAI",
            "enabled": true, "health": {"status": "ok", "message": "Ready · 3 models", "checked_ms": 5}})).unwrap();
        assert_eq!(api.health.tone(), crate::console::ui::Tone::Ok);
        let eng: EngineInstance = serde_json::from_value(json!({"id": "codex", "engine": "codex", "builtin": true,
            "label": "Codex", "enabled": false, "health": {"status": "off", "message": "Disabled"}})).unwrap();
        assert_eq!(last_checked([&api.health, &eng.health]), Some(5));
        let edit = EngineEdit {
            enabled: Some(true),
            accent: Some(None),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&edit).unwrap(),
            json!({"enabled": true, "accent": null})
        );
        assert_eq!(enc("a/b c"), "a%2Fb%20c");
    }
}
