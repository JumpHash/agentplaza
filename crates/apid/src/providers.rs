//! Model providers (the hub's store, component `providers`), their credentials (the secret vault, write-only through the
//! API) and the gateway built from them.
//!
//! OWNER: slice "providers". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use apz_gateway::{
    Auth, Caller, Gateway, ModelInfo, ProviderConfig, ProviderKind, SecretRead, SecretReason,
    SecretSource, presets,
};
use apz_http::{Params, Request, Response, RouteDoc, Router};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::App;
use crate::vault::{Consumer, OnBehalf, VaultError, name_for};

/// The actor of the vault's events for a provider key the owner set.
const OWNER: &str = "owner";

/// What the view shows in place of a value it keeps back: a header's value, a command's text, a key an
/// error echoed. An Edit that sends it back means "keep the stored value" (see [`restore_redacted`]).
pub(crate) const REDACTED: &str = "•••";

/// The providers a fresh store starts with.
const SEED_IDS: [&str; 16] = [
    "anthropic",
    "openai",
    "xai",
    "deepseek",
    "gemini",
    "zai",
    "moonshot",
    "ollama",
    "llamacpp",
    "mock",
    "mistral",
    "openai-decisions",
    "typesafe",
    "meta",
    "openrouter",
    "opencode",
];

/// A failed mutation: the HTTP status and a message that never names a secret's value.
pub(crate) type Failure = (u16, String);

/// The gateway's credentials: the vault's entry for the provider, as the caller may read it.
struct VaultSecrets(Arc<crate::vault::Store>);

impl SecretSource for VaultSecrets {
    fn get(&self, name: &str, read: &SecretRead<'_>) -> Option<String> {
        self.fetch(name, read).ok()
    }

    /// One vault lookup: the value, or the vault's own refusal, so a refused lookup is audited once.
    fn fetch(&self, name: &str, read: &SecretRead<'_>) -> Result<String, SecretReason> {
        match resolve_for(&self.0, name, read) {
            Ok(secret) => {
                let value = secret.expose_secret();
                if value.is_empty() {
                    Err(SecretReason::Missing)
                } else {
                    Ok(value.to_string())
                }
            }
            Err(VaultError::Sealed | VaultError::Uninitialized) => Err(SecretReason::Sealed),
            Err(VaultError::Tampered(reason)) => Err(SecretReason::Tampered(reason)),
            Err(VaultError::OutOfScope) => Err(SecretReason::OutOfScope),
            Err(_) => Err(SecretReason::Missing),
        }
    }
}

/// The vault's value of `name` for the provider `read` names, read for `read`'s caller. `Ok(None)` when
/// the vault has no such entry for that caller. `Err` when the vault refuses: sealed, uninitialised,
/// tampered, or the entry is held only by other nodes. No key may then come from anywhere else.
fn vault_secret(
    vault: &crate::vault::Store,
    name: &str,
    read: &SecretRead<'_>,
) -> Result<Option<String>, VaultError> {
    match resolve_for(vault, name, read) {
        Ok(secret) => Ok(Some(secret.expose_secret().to_string())),
        Err(
            err @ (VaultError::Sealed
            | VaultError::Uninitialized
            | VaultError::Tampered(_)
            | VaultError::OutOfScope),
        ) => Err(err),
        Err(_) => Ok(None),
    }
}

/// The vault's resolve of `name` for the provider `read` names, as `read`'s caller: the value, or the refusal.
fn resolve_for(
    vault: &crate::vault::Store,
    name: &str,
    read: &SecretRead<'_>,
) -> Result<SecretString, VaultError> {
    let on_behalf = match read.caller {
        Caller::Agent { cid, node } => OnBehalf::Agent {
            cid: *cid,
            node: node.clone(),
        },
        Caller::Owner { node } => OnBehalf::Owner { node: node.clone() },
        Caller::Hub => OnBehalf::Hub,
    };
    let consumer = Consumer {
        binding: provider_binding(read.provider),
        on_behalf,
    };
    vault.resolve(&name_for(name), &consumer, apz_plaza::now_ms())
}

/// The vault binding of a provider's credential: `provider:<id>`.
fn provider_binding(id: &str) -> String {
    format!("provider:{id}")
}

/// Non-secret settings the gateway reads from the process environment (regions, cloud projects).
/// Credentials never come from it: they are the vault's, and the env file's are imported there.
const CONFIG_ENV: [&str; 5] = [
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "GOOGLE_CLOUD_PROJECT",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
];

/// The AWS key pair and session token a Bedrock-kind provider signs with. Each such provider reads its own
/// pair: the vault entries bound to `provider:<its id>` (see [`bedrock_bindings`]). The first two are required.
pub(crate) const BEDROCK_KEYS: [&str; 3] = [
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
];
/// The pair names a Bedrock provider cannot sign without (the session token is optional).
const BEDROCK_REQUIRED: [&str; 2] = ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"];
/// The binding the pair falls back to when no Bedrock-kind provider exists (the migration binds to it then).
pub(crate) const BEDROCK_BINDING: &str = "provider:bedrock";

/// The vault bindings of the AWS pairs: `provider:<id>` for each Bedrock-kind provider, in list order.
pub(crate) fn bedrock_bindings(providers: &[ProviderConfig]) -> Vec<String> {
    providers
        .iter()
        .filter(|p| p.kind == ProviderKind::Bedrock)
        .map(|p| provider_binding(&p.id))
        .collect()
}

/// The gateway's environment: only [`CONFIG_ENV`] names resolve, so no key is read from the process.
fn config_env(name: &str) -> Option<String> {
    if !CONFIG_ENV.contains(&name) {
        return None;
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// A provider's label for the vault's consumers list: its preset's, else its id.
fn label_of(p: &ProviderConfig) -> String {
    presets()
        .into_iter()
        .find(|preset| preset.id == p.id)
        .map_or_else(|| p.id.clone(), |preset| preset.label.to_string())
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Store {
    gateway: Gateway,
    /// The secret vault: every provider key lives there.
    vault: Arc<crate::vault::Store>,
    /// Serializes every mutation of providers and their keys.
    write: Mutex<()>,
    /// Each provider's last good model listing, refreshed in the background (`provider_listings.rs`).
    pub listings: Arc<crate::provider_listings::Listings>,
    /// The saved default decision model (`/v1/decisions/default`).
    pub decisions: crate::decisions::DefaultStore,
    /// The last check of each provider, by id (`provider_health.rs`). Kept in memory: the first check
    /// after a start fills it again.
    health: Mutex<BTreeMap<String, Value>>,
    /// The preset ids offered to this install, so a deleted preset stays deleted (see `offer_presets`).
    offered: Mutex<Vec<String>>,
}

/// The providers' tables: the providers in order, and the preset ids offered to the install.
#[derive(Debug, apz_store::Table)]
#[table(name = "providers")]
struct Stored {
    #[table(child = "provider")]
    providers: Vec<ProviderConfig>,
    #[table(list = "provider_offered")]
    offered: Vec<String>,
}

/// The providers' tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Stored)
}

/// A store over `vault` loaded from an empty database: a first start, seeded with the presets.
#[cfg(test)]
pub(crate) fn test_store(vault: Arc<crate::vault::Store>) -> Store {
    let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(&def().ddl()).unwrap();
    let store = Store::new(vault);
    store.load(&conn).unwrap();
    store
}

impl Store {
    /// An empty store over `vault`: no provider until [`Store::load`] fills it at start.
    pub fn new(vault: Arc<crate::vault::Store>) -> Self {
        Self {
            gateway: Gateway::builder()
                .secrets(VaultSecrets(Arc::clone(&vault)))
                .env(config_env)
                .build(),
            vault,
            write: Mutex::new(()),
            listings: Arc::new(crate::provider_listings::Listings::new()),
            decisions: Default::default(),
            health: Mutex::new(BTreeMap::new()),
            offered: Mutex::new(Vec::new()),
        }
    }

    /// Replaces the providers with what the store holds and registers each provider's secret with the vault. A fresh
    /// store is seeded with the presets. Keys are read from the vault at call time; `Auth::Env` falls back to the
    /// process environment. The env file is not read here: the vault imports it at start.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let (mut providers, mut offered) = match apz_store::table::load_one::<Stored>(conn, def())?
        {
            Some(stored) => (stored.providers, stored.offered),
            None => {
                let seeded = seed();
                let offered = seeded.iter().map(|p| p.id.clone()).collect();
                (seeded, offered)
            }
        };
        for id in offer_presets(&mut providers, &mut offered) {
            eprintln!("providers: added {id} (new preset)");
        }
        // A credential the store blanked keeps the live value it has (none at start: `sync_vault` fills it once the
        // vault is up).
        keep_live_secrets(&mut providers, &self.gateway.providers());
        self.gateway.set_providers(providers.clone());
        self.register(&providers);
        *lock(&self.offered) = offered;
        Ok(())
    }

    /// The providers and the offered presets, as rows of the providers' tables (for the hub's store). A command
    /// credential and the header values are blanked, as they may hold literal tokens: the store keeps no secret.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let mut providers = self.gateway.providers();
        for p in &mut providers {
            blank_secrets(p);
        }
        let stored = Stored {
            providers,
            offered: lock(&self.offered).clone(),
        };
        apz_store::RowSet::one(def(), &stored)
    }

    /// The gateway over the current providers and secrets.
    pub fn gateway(&self) -> Gateway {
        // contract
        self.gateway.clone()
    }

    /// Every provider's view, for the engines' health (`engine_instances.rs`).
    pub(crate) fn views_for_engines(&self) -> Vec<Value> {
        self.views()
    }

    /// Every provider's view, by id. Never carries a secret value.
    fn views(&self) -> Vec<Value> {
        self.gateway
            .providers()
            .iter()
            .map(|p| self.view(p))
            .collect()
    }

    /// A provider's credential state, from the vault (nothing is decrypted): `none`, `command`,
    /// `secret` (this machine reads it), `out_of_scope` (only other nodes hold it), `sealed` (the
    /// vault is sealed), or `missing`.
    fn credential(&self, p: &ProviderConfig) -> &'static str {
        let name = match &p.auth {
            // A Bedrock provider with no key of its own signs with its AWS pair.
            Auth::None if p.kind == ProviderKind::Bedrock => return self.aws_pair(&p.id),
            Auth::None => return "none",
            Auth::Command(_) => return "command",
            Auth::Env(name) | Auth::Secret(name) => name,
        };
        self.vault
            .presence(&name_for(name), &provider_binding(&p.id))
    }

    /// The AWS pair's state for provider `id`: `secret` when both required keys resolve on this machine;
    /// otherwise the vault's refusal (sealed first, then out of scope), else `missing`.
    fn aws_pair(&self, id: &str) -> &'static str {
        let binding = provider_binding(id);
        let states: Vec<&str> = BEDROCK_REQUIRED
            .iter()
            .map(|name| self.vault.presence(&name_for(name), &binding))
            .collect();
        if states.iter().all(|state| *state == "secret") {
            "secret"
        } else if states.contains(&"sealed") {
            "sealed"
        } else if states.contains(&"out_of_scope") {
            "out_of_scope"
        } else {
            "missing"
        }
    }

    fn view(&self, p: &ProviderConfig) -> Value {
        let credential = self.credential(p);
        let auth = match &p.auth {
            // A command may embed a literal token, so only its presence shows.
            Auth::Command(_) => json!({ "command": REDACTED }),
            other => serde_json::to_value(other).unwrap_or(Value::Null),
        };
        let headers: BTreeMap<&str, &str> = p
            .extra_headers
            .keys()
            .map(|k| (k.as_str(), REDACTED))
            .collect();
        json!({
            "id": p.id,
            "family": "api",
            "kind": p.kind.as_str(),
            "group": p.kind.group(),
            "label": display_label(p),
            "accent": p.accent,
            "enabled": p.enabled,
            "health": self.health_of(p),
            "base_url": p.base_url,
            "auth": auth,
            "models": p.models,
            "options": p.options,
            "extra_headers": headers,
            // Not secret: plaza.db keeps it as it is, where it blanks the command and the header values.
            "extra_body": p.extra_body,
            "credential": credential,
        })
    }

    /// A provider's endpoint and key, for the pass-through model gateway (`llm_proxy`): the
    /// configured base URL (or the kind's default for anthropic/openai) and the key `caller` may
    /// read. `Ok(None)` for an unknown provider. `Err` when the vault is sealed, uninitialised or
    /// tampered: no key is served then, from the vault or anywhere else.
    pub fn upstream(
        &self,
        id: &str,
        caller: &Caller,
    ) -> Result<Option<(String, Option<String>)>, VaultError> {
        let Ok(p) = self.find(id) else {
            return Ok(None);
        };
        // A disabled provider takes no calls, through the gateway or past it.
        if !p.enabled {
            return Ok(None);
        }
        let base = p.base_url.clone().unwrap_or_else(|| match id {
            "openai" => "https://api.openai.com/v1".to_string(),
            _ => "https://api.anthropic.com".to_string(),
        });
        Ok(Some((base, self.key_for(&p, caller)?)))
    }

    /// A provider's key for `caller`, from the vault only (the env file and the process environment
    /// were imported at start). `None` for a provider with no named key, or none this caller may read.
    /// `Err` when the vault cannot answer at all (see [`vault_secret`]).
    fn key_for(&self, p: &ProviderConfig, caller: &Caller) -> Result<Option<String>, VaultError> {
        let read = SecretRead {
            provider: &p.id,
            caller,
        };
        match &p.auth {
            Auth::Env(name) | Auth::Secret(name) => vault_secret(&self.vault, name, &read),
            _ => Ok(None),
        }
    }

    /// The owner's check of provider `id`'s key: one tiny call on `node` (the hub's machine when
    /// `None`), made with the key the vault holds for that node. `Ok` names the model that
    /// answered. Neither side of the result carries a key value.
    pub fn check_key(&self, id: &str, node: Option<String>) -> Result<String, String> {
        let caller = Caller::Owner { node };
        // The key is read once, here: a refusal is audited once and ends the check, and the key read
        // here redacts the answer, so no second lookup happens.
        let provider = self.find(id).map_err(|(_, message)| message)?;
        let key = self
            .key_for(&provider, &caller)
            .map_err(|refusal| refusal.message())?;
        let report = self
            .gateway
            .test_for(id, None, &caller)
            .map_err(|e| e.to_string())?;
        if report.ok {
            return Ok(format!("model {} answered", report.model));
        }
        let error = report
            .error
            .unwrap_or_else(|| "the call failed".to_string());
        Err(redact(error, key.as_deref()))
    }

    fn find(&self, id: &str) -> Result<ProviderConfig, Failure> {
        self.gateway
            .providers()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| (404, format!("no provider `{id}`")))
    }

    /// The fixed headers a provider sends with every call (its `extra_headers`, e.g. OpenRouter's
    /// referer). Empty for an unknown provider or one with none.
    pub fn extra_headers(&self, provider: &str) -> Vec<(String, String)> {
        self.find(provider)
            .map(|p| p.extra_headers.into_iter().collect())
            .unwrap_or_default()
    }

    /// Makes `providers` the gateway's providers (the store saves them). A command credential or header value that is
    /// new or changed goes to the vault first (plaza.db keeps them blank): a vault that cannot take it refuses the
    /// change, and nothing changes.
    fn commit(&self, providers: Vec<ProviderConfig>) -> Result<(), Failure> {
        let old = self.gateway.providers();
        let now = apz_plaza::now_ms();
        for p in &providers {
            let before = old
                .iter()
                .find(|o| o.id == p.id)
                .map(literal_secrets)
                .unwrap_or_default();
            for (name, value) in literal_secrets(p) {
                if !before.contains(&(name.clone(), value.clone())) {
                    self.vault
                        .put_plaza(
                            &name,
                            &provider_binding(&p.id),
                            "provider credential",
                            &SecretString::from(value),
                            OWNER,
                            now,
                        )
                        .map_err(vault_failure)?;
                }
            }
        }
        self.unregister_changed(&old, &providers);
        self.register(&providers);
        // A provider's listing follows its generation: bump the providers that changed, forget the removed ones.
        for p in &providers {
            let before = old.iter().find(|o| o.id == p.id);
            if before.is_none_or(|o| serde_json::to_value(o).ok() != serde_json::to_value(p).ok()) {
                self.listings.changed(&p.id);
            }
        }
        for o in old
            .iter()
            .filter(|o| !providers.iter().any(|p| p.id == o.id))
        {
            self.listings.removed(&o.id);
        }
        self.gateway.set_providers(providers);
        Ok(())
    }

    /// Fills the command credentials and header values plaza.db keeps blank from the vault, where [`Store::commit`]
    /// put them, and puts there the ones it does not hold yet (a fresh start's presets). Runs once the vault is up at
    /// start (state.rs) and after each unseal; a value a sealed vault cannot give stays blank until then.
    pub fn sync_vault(&self) {
        let _guard = lock(&self.write);
        let mut providers = self.gateway.providers();
        let now = apz_plaza::now_ms();
        let mut filled = false;
        for p in &mut providers {
            let binding = provider_binding(&p.id);
            let consumer = Consumer {
                binding: binding.clone(),
                on_behalf: OnBehalf::Hub,
            };
            let mut sync =
                |name: &str, value: &mut String| match self.vault.resolve(name, &consumer, now) {
                    Ok(stored) if value.is_empty() => {
                        value.push_str(stored.expose_secret());
                        filled |= !value.is_empty();
                    }
                    Err(VaultError::NotFound) if !value.is_empty() => {
                        let secret = SecretString::from(value.clone());
                        if let Err(e) = self.vault.put_plaza(
                            name,
                            &binding,
                            "provider credential",
                            &secret,
                            OWNER,
                            now,
                        ) {
                            eprintln!(
                                "providers: {}: the vault did not take {name}: {}",
                                p.id,
                                e.message()
                            );
                        }
                    }
                    _ => {}
                };
            if let Auth::Command(command) = &mut p.auth {
                sync(&command_secret(&p.id), command);
            }
            for (header, value) in p.extra_headers.iter_mut() {
                sync(&header_secret(&p.id, header), value);
            }
        }
        if filled {
            self.gateway.set_providers(providers);
        }
    }

    /// Drops the vault registrations of providers that were removed, or whose secret name changed, so
    /// the consumers list shows only what the providers read now. [`Store::register`] adds the rest.
    fn unregister_changed(&self, old: &[ProviderConfig], new: &[ProviderConfig]) {
        // What a provider reads from the vault: its secret name, and whether it reads the AWS pair.
        let reads = |p: &ProviderConfig| {
            let secret = match &p.auth {
                Auth::Env(name) | Auth::Secret(name) => Some(name_for(name)),
                _ => None,
            };
            (secret, p.kind == ProviderKind::Bedrock)
        };
        for p in old {
            let unchanged = new
                .iter()
                .find(|q| q.id == p.id)
                .is_some_and(|q| reads(q) == reads(p));
            if !unchanged {
                // Every registration under the provider's binding goes; `register` adds back what the new list reads.
                self.vault.unregister_consumer(&provider_binding(&p.id));
            }
        }
    }

    /// Tells the vault which secret each provider reads (`provider:<id>` wants the name), so the
    /// consumers list shows it. Registering a pair again replaces the earlier entry. A Bedrock provider
    /// also registers its own AWS pair under `provider:<its id>`.
    fn register(&self, providers: &[ProviderConfig]) {
        for p in providers {
            if let Auth::Env(name) | Auth::Secret(name) = &p.auth {
                self.vault.register_consumer(
                    &provider_binding(&p.id),
                    &label_of(p),
                    &name_for(name),
                );
            }
            if p.kind == ProviderKind::Bedrock {
                for name in BEDROCK_KEYS {
                    self.vault.register_consumer(
                        &provider_binding(&p.id),
                        &label_of(p),
                        &name_for(name),
                    );
                }
            }
        }
    }

    fn insert(&self, cfg: ProviderConfig) -> Result<Value, Failure> {
        let _guard = lock(&self.write);
        if !valid_id(&cfg.id) {
            return Err((400, "id must match [a-z0-9_-]{1,40}".into()));
        }
        check_identity(&cfg)?;
        let mut providers = self.gateway.providers();
        if providers.iter().any(|p| p.id == cfg.id) {
            return Err((409, format!("provider `{}` exists", cfg.id)));
        }
        let view = self.view(&cfg);
        providers.push(cfg);
        self.commit(providers)?;
        Ok(view)
    }

    fn replace(&self, id: &str, cfg: ProviderConfig) -> Result<Value, Failure> {
        let _guard = lock(&self.write);
        if cfg.id != id {
            return Err((400, "the body's id must equal the path's id".into()));
        }
        check_identity(&cfg)?;
        let mut providers = self.gateway.providers();
        let slot = providers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| (404, format!("no provider `{id}`")))?;
        let cfg = restore_redacted(slot, cfg)?;
        *slot = cfg;
        let view = self.view(slot);
        self.commit(providers)?;
        Ok(view)
    }

    fn remove(&self, id: &str) -> Result<Value, Failure> {
        let _guard = lock(&self.write);
        let mut providers = self.gateway.providers();
        let before = providers.len();
        providers.retain(|p| p.id != id);
        if providers.len() == before {
            return Err((404, format!("no provider `{id}`")));
        }
        lock(&self.health).remove(id);
        self.commit(providers)?;
        Ok(json!({ "deleted": id }))
    }

    /// Stores `value` as the provider's key: a plaza entry in the vault, bound to `provider:<id>`,
    /// named by the provider's `Auth::Env` or `Auth::Secret` name. A provider with neither gets
    /// `Auth::Secret(<ID>_API_KEY)`. Write-only: the value is never returned or written to a file.
    fn set_secret(&self, id: &str, value: &str) -> Result<Value, Failure> {
        let _guard = lock(&self.write);
        let mut providers = self.gateway.providers();
        let slot = providers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| (404, format!("no provider `{id}`")))?;
        let name = match &slot.auth {
            Auth::Env(name) | Auth::Secret(name) => name_for(name),
            _ => name_for(&format!("{id}-api-key")),
        };
        let secret = SecretString::from(value.to_string());
        let now = apz_plaza::now_ms();
        self.vault
            .put_plaza(
                &name,
                &provider_binding(id),
                "provider key",
                &secret,
                OWNER,
                now,
            )
            .map_err(vault_failure)?;
        if !matches!(slot.auth, Auth::Env(_) | Auth::Secret(_)) {
            slot.auth = Auth::Secret(name);
        }
        let view = self.view(slot);
        self.commit(providers)?;
        // A new key changes what the provider lists even when its config did not change.
        self.listings.changed(id);
        Ok(view)
    }

    /// Every enabled provider's last good listing, however old, without asking any provider, in the
    /// `GET /v1/models` shape; `pending` names the providers never listed yet. Never waits.
    fn cached_models(&self) -> Value {
        let mut models = Vec::new();
        let mut errors = Vec::new();
        let mut pending = Vec::new();
        for p in self.gateway.providers().into_iter().filter(|p| p.enabled) {
            match self.listings.peek(&p.id) {
                Some(listing) => models.extend(
                    listing
                        .models
                        .iter()
                        .map(|m| model_entry(&p, ModelInfo::from(m))),
                ),
                None => pending.push(p.id.clone()),
            }
            if let Some(error) = self.listings.attempt(&p.id).and_then(|a| a.error) {
                errors.push(json!({ "provider": p.id, "error": error }));
            }
        }
        json!({ "models": models, "errors": errors, "pending": pending })
    }

    /// `GET /v1/models`: every enabled provider's models from the cache, at once. Providers never listed are
    /// listed now, all together, and waited for at most [`crate::provider_listings::COLD_WAIT`] (a cold start);
    /// one that takes longer lands in the cache later and is named in `pending` meanwhile.
    fn all_models(&self) -> Value {
        let providers: Vec<ProviderConfig> = self.gateway.providers();
        let missing: Vec<&ProviderConfig> = providers
            .iter()
            .filter(|p| p.enabled && self.listings.peek(&p.id).is_none())
            .collect();
        if !missing.is_empty() {
            self.listings.fetch_all(
                &self.gateway,
                &missing,
                false,
                crate::provider_listings::COLD_WAIT,
            );
        }
        self.cached_models()
    }

    /// Lists every provider whose listing is due (missing, from an older generation, or past its interval), all at
    /// once; returns whether any listing was fetched. The refresher thread calls it every
    /// [`crate::provider_listings::TICK`].
    pub fn refresh_due(&self) -> bool {
        let providers = self.gateway.providers();
        let due = self.listings.due(&providers);
        if due.is_empty() {
            return false;
        }
        // Each listing runs under the listing client's own deadline; this waits for all of them.
        self.listings.fetch_all(
            &self.gateway,
            &due,
            false,
            apz_gateway::LISTING_DEADLINE + Duration::from_secs(1),
        );
        true
    }

    /// The last check of `p`, or `unknown` before its first.
    fn health_of(&self, p: &ProviderConfig) -> Value {
        if !p.enabled {
            return json!({ "status": "off", "message": "Disabled" });
        }
        lock(&self.health)
            .get(&p.id)
            .cloned()
            .unwrap_or_else(|| json!({ "status": "unknown", "message": "Not checked yet" }))
    }

    /// Checks provider `id` now (see [`crate::provider_health::check`]: a forced refresh of its listing) and keeps
    /// the result.
    pub fn check(&self, id: &str) -> Result<Value, Failure> {
        let p = self.find(id)?;
        let health =
            crate::provider_health::check(&self.listings, &self.gateway, &p, self.credential(&p));
        lock(&self.health).insert(p.id.clone(), health.clone());
        Ok(json!({ "id": p.id, "health": health }))
    }

    /// The ids of the providers the gateway holds, in its order.
    pub fn provider_ids(&self) -> Vec<String> {
        self.gateway.providers().into_iter().map(|p| p.id).collect()
    }

    /// The model ids provider `id` lists now. The listing is refreshed, as a health check refreshes it (the engine
    /// watch reads these, PLAN 12.6).
    pub fn listed_model_ids(&self, id: &str) -> Result<Vec<String>, String> {
        let p = self
            .gateway
            .providers()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no provider {id}"))?;
        let listing = self.listings.fetch(&self.gateway, &p, true)?;
        Ok(listing.models.iter().map(|m| m.id.clone()).collect())
    }

    /// Checks every enabled provider at once (one thread each, each under the listing deadline) and returns each
    /// one's result by id. A check refreshes the listing it probes; it clears nothing.
    pub fn check_all(&self) -> Value {
        let providers = self.gateway.providers();
        let checked: Vec<(String, Value)> = std::thread::scope(|scope| {
            let running: Vec<_> = providers
                .iter()
                .map(|p| {
                    scope.spawn(move || {
                        let health = if p.enabled {
                            crate::provider_health::check(
                                &self.listings,
                                &self.gateway,
                                p,
                                self.credential(p),
                            )
                        } else {
                            self.health_of(p)
                        };
                        (p.id.clone(), health)
                    })
                })
                .collect();
            running.into_iter().filter_map(|t| t.join().ok()).collect()
        });
        let mut out = serde_json::Map::new();
        for (id, health) in checked {
            if providers.iter().any(|p| p.id == id && p.enabled) {
                lock(&self.health).insert(id.clone(), health.clone());
            }
            out.insert(id, health);
        }
        Value::Object(out)
    }
}

/// Starts the listings' refresher: every [`crate::provider_listings::TICK`] it lists the providers whose listing is
/// due, and tells the consoles when a listing changed (a `models` event: a hint to revalidate their list).
pub fn spawn_refresher(app: Arc<App>) {
    std::thread::Builder::new()
        .name("model-listings".into())
        .spawn(move || {
            loop {
                if app.providers.refresh_due() {
                    app.bus.publish("models", &json!({ "refreshed": true }));
                }
                std::thread::sleep(crate::provider_listings::TICK);
            }
        })
        .expect("spawn the model listings thread");
}

/// `/v1/provider-kinds`, `/v1/providers…`, `/v1/models`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/provider-kinds", kinds_route)
        .doc(RouteDoc::new("List the provider kinds you can add"))
        .get("/v1/providers", list_route)
        .doc(RouteDoc::new("List the providers"))
        .post("/v1/providers/check", check_all_route)
        .doc(
            RouteDoc::new("Check every enabled provider now: its key and its model listing")
                .outbound(),
        )
        .post("/v1/providers/{id}/check", check_route)
        .doc(
            RouteDoc::new("Check one provider now: its key and its model listing")
                .outbound()
                .errors(&[404]),
        )
        .post("/v1/providers", create_route)
        .doc(RouteDoc::new("Add a provider").status(201).errors(&[400]))
        .get("/v1/providers/{id}", get_route)
        .doc(RouteDoc::new("Show a provider"))
        .route("PUT", "/v1/providers/{id}", replace_route)
        .doc(RouteDoc::new("Replace a provider's settings").errors(&[400]))
        .delete("/v1/providers/{id}", delete_route)
        .doc(RouteDoc::new("Delete a provider"))
        .route("PUT", "/v1/providers/{id}/secret", secret_route)
        .doc(RouteDoc::new("Set a provider's API key").errors(&[400]))
        .post("/v1/providers/{id}/test", test_route)
        .doc(
            RouteDoc::new("Test a provider's connection")
                .outbound()
                .errors(&[502]),
        )
        .get("/v1/providers/{id}/models", provider_models_route)
        .doc(
            RouteDoc::new("List a provider's models")
                .outbound()
                .errors(&[502]),
        )
        .get("/v1/models", all_models_route)
        .doc(RouteDoc::new("List the models of every provider").outbound())
}

fn check_all_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let health = app.providers.check_all();
    app.bus.publish("providers.health", &health);
    Response::json(200, &health)
}

fn check_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.providers.check(p.get("id")) {
        Ok(v) => {
            app.bus.publish(
                "providers.health",
                &json!({ v["id"].as_str().unwrap_or(""): v["health"] }),
            );
            Response::json(200, &v)
        }
        Err(f) => failure(f),
    }
}

fn kinds_route(_: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &presets())
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &app.providers.views())
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.providers.find(p.get("id")) {
        Ok(cfg) => Response::json(200, &app.providers.view(&cfg)),
        Err(f) => failure(f),
    }
}

fn create_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let cfg: ProviderConfig = match req.json() {
        Ok(cfg) => cfg,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    match app.providers.insert(cfg) {
        Ok(view) => Response::json(201, &view),
        Err(f) => failure(f),
    }
}

fn replace_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let cfg: ProviderConfig = match req.json() {
        Ok(cfg) => cfg,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    match app.providers.replace(p.get("id"), cfg) {
        Ok(view) => Response::json(200, &view),
        Err(f) => failure(f),
    }
}

fn delete_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.providers.remove(p.get("id")) {
        Ok(v) => Response::json(200, &v),
        Err(f) => failure(f),
    }
}

#[derive(Deserialize)]
struct SecretBody {
    value: String,
}

fn secret_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: SecretBody = match req.json() {
        Ok(body) => body,
        Err(_) => return failure((400, "body must be {\"value\": \"...\"}".into())),
    };
    if body.value.is_empty() {
        return failure((400, "value is empty".into()));
    }
    match app.providers.set_secret(p.get("id"), &body.value) {
        Ok(view) => Response::json(200, &view),
        Err(f) => failure(f),
    }
}

#[derive(Deserialize)]
struct TestBody {
    model: Option<String>,
}

fn test_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if let Err(f) = app.providers.find(id) {
        return failure(f);
    }
    let model = req.json::<TestBody>().ok().and_then(|b| b.model);
    match app
        .providers
        .gateway()
        .test_for(id, model.as_deref(), &Caller::Owner { node: None })
    {
        Ok(report) => Response::json(200, &report),
        Err(e) => Response::json(502, &json!({ "error": e.to_string() })),
    }
}

/// `GET /v1/providers/{id}/models[?refresh=1]`: the provider's listing from the cache (listed now if it never was);
/// `refresh=1` lists it again first.
fn provider_models_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let config = match app.providers.find(id) {
        Ok(config) => config,
        Err(f) => return failure(f),
    };
    let force = req
        .query_param("refresh")
        .is_some_and(|v| v == "1" || v == "true");
    let listing = match app.providers.listings.peek(id) {
        Some(listing) if !force => Ok(listing),
        _ => app
            .providers
            .listings
            .fetch(&app.providers.gateway, &config, force),
    };
    match listing {
        Ok(listing) => {
            let models: Vec<ModelInfo> = listing.models.iter().map(ModelInfo::from).collect();
            Response::json(200, &models)
        }
        Err(e) => Response::json(502, &json!({ "error": e })),
    }
}

fn all_models_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let mut all = app.providers.all_models();
    let favourites = app.settings.get().favourite_models;
    if let Some(models) = all["models"].as_array_mut() {
        for m in models.iter_mut() {
            let name = format!(
                "{}/{}",
                m["provider"].as_str().unwrap_or(""),
                m["id"].as_str().unwrap_or("")
            );
            m["favourite"] = Value::Bool(favourites.contains(&name));
        }
    }
    Response::json(200, &all)
}

/// One `/v1/models` entry: the model, its provider, the provider's group, whether the model is a
/// contributor model that trains its provider's future models on what it is sent (`trains_on_data`),
/// and its context window in tokens (`context`, null when the provider's listing does not say).
fn model_entry(p: &ProviderConfig, m: ModelInfo) -> Value {
    let trains_on_data = apz_gateway::lists_training_model(p.options.as_ref(), &m.id);
    json!({ "provider": p.id, "provider_label": display_label(p), "accent": p.accent, "group": p.kind.group(), "id": m.id, "name": m.name, "context": m.context, "trains_on_data": trains_on_data })
}

/// The context window of `model` (`provider/model`, or a bare model id) in tokens: the window the
/// provider's listing states for it, else the window of its known model family (see [`known_window`]).
/// None when neither knows it, as for a local model.
/// Reads only the cached listing: metrics compose this every few seconds and must never wait on a provider.
pub fn context_window(app: &App, model: &str) -> Option<u64> {
    listed_window(&app.providers.cached_models()["models"], model).or_else(|| known_window(model))
}

/// The window the `/v1/models` entries (`models`, the array `all_models` answers with) state for
/// `model`. A `provider/id` match is preferred over a bare `id` match; the first match that states a
/// window answers.
fn listed_window(models: &Value, model: &str) -> Option<u64> {
    let entries = models.as_array()?;
    let qualified = entries.iter().filter(|e| {
        let provider = e["provider"].as_str().unwrap_or_default();
        let id = e["id"].as_str().unwrap_or_default();
        format!("{provider}/{id}") == model
    });
    let bare = entries.iter().filter(|e| e["id"].as_str() == Some(model));
    qualified.chain(bare).find_map(|e| e["context"].as_u64())
}

/// The context windows of the model families the plaza knows, by the prefix of the model's id. Used
/// when a provider's listing does not state a window for a model.
const KNOWN_WINDOWS: &[(&str, u64)] = &[
    ("claude-opus", 200_000),
    ("claude-sonnet", 200_000),
    ("claude-haiku", 200_000),
    ("gpt-5", 400_000),
    ("deepseek", 128_000),
];

/// The known window of `model`, matched on the id after its last `/` (so a `provider/` prefix, or
/// the `openrouter/anthropic/…` form, does not hide the family). None for a family not in the table.
fn known_window(model: &str) -> Option<u64> {
    let id = model.rsplit('/').next().unwrap_or(model);
    KNOWN_WINDOWS
        .iter()
        .find(|(prefix, _)| id.starts_with(prefix))
        .map(|(_, window)| *window)
}

/// `text` with `key` replaced by [`REDACTED`] when it carries it: an upstream error may echo a key.
fn redact(text: String, key: Option<&str>) -> String {
    match key {
        Some(key) if !key.is_empty() && text.contains(key) => text.replace(key, REDACTED),
        _ => text,
    }
}

/// The body of an Edit with the values the view redacted put back from `stored`: a header whose value
/// is [`REDACTED`] keeps the stored value, a command equal to it keeps the stored command, and an
/// `extra_body` the body omits (null) keeps the stored one. A redacted value with nothing stored to
/// keep is refused, so the marker never becomes a live header or command.
fn restore_redacted(
    stored: &ProviderConfig,
    mut body: ProviderConfig,
) -> Result<ProviderConfig, Failure> {
    for (name, value) in body.extra_headers.iter_mut() {
        if value.as_str() == REDACTED {
            *value = stored.extra_headers.get(name).cloned().ok_or_else(|| {
                (
                    400,
                    format!("header `{name}` has no stored value to keep: send its value"),
                )
            })?;
        }
    }
    match (&mut body.auth, &stored.auth) {
        (Auth::Command(command), Auth::Command(stored_command)) if command.as_str() == REDACTED => {
            command.clone_from(stored_command);
        }
        (Auth::Command(command), _) if command.as_str() == REDACTED => {
            return Err((
                400,
                "the command is redacted but the provider has none stored: send the command".into(),
            ));
        }
        _ => {}
    }
    if body.extra_body.is_null() {
        body.extra_body = stored.extra_body.clone();
    }
    Ok(body)
}

pub(crate) fn failure((status, message): Failure) -> Response {
    Response::json(status, &json!({ "error": message }))
}

/// A vault refusal as a failure. The vault's messages never carry a value.
fn vault_failure(e: VaultError) -> Failure {
    let status = match &e {
        VaultError::Sealed | VaultError::Uninitialized | VaultError::Tampered(_) => 503,
        VaultError::Invalid(_) => 400,
        VaultError::Conflict(_) => 409,
        _ => 500,
    };
    (status, e.message())
}

/// The providers of a fresh store: the presets named in `SEED_IDS`.
fn seed() -> Vec<ProviderConfig> {
    presets()
        .iter()
        .filter(|p| SEED_IDS.contains(&p.id))
        .map(|p| p.config())
        .collect()
}

/// Offers each `SEED_IDS` preset not yet in `offered`: appends its config to `providers` unless a
/// provider with that id exists, and records the id in `offered` either way. Returns the ids
/// appended.
fn offer_presets(providers: &mut Vec<ProviderConfig>, offered: &mut Vec<String>) -> Vec<String> {
    let mut introduced = Vec::new();
    for id in SEED_IDS {
        if offered.iter().any(|o| o == id) {
            continue;
        }
        offered.push(id.to_string());
        if providers.iter().any(|p| p.id == id) {
            continue;
        }
        if let Some(preset) = presets().into_iter().find(|p| p.id == id) {
            providers.push(preset.config());
            introduced.push(id.to_string());
        }
    }
    introduced
}

/// Provider ids: `[a-z0-9-]`, 1 to 40 characters.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// The most characters a provider's label may have.
const MAX_LABEL: usize = 60;

/// A provider's label and accent: a label of at most [`MAX_LABEL`] characters with no control
/// characters, and an accent of the form `#rrggbb`.
fn check_identity(cfg: &ProviderConfig) -> Result<(), Failure> {
    if let Some(label) = &cfg.label
        && (label.trim().is_empty()
            || label.chars().count() > MAX_LABEL
            || label.chars().any(char::is_control))
    {
        return Err((
            400,
            format!("label must be 1 to {MAX_LABEL} characters, with no control characters"),
        ));
    }
    if let Some(accent) = &cfg.accent
        && !valid_accent(accent)
    {
        return Err((400, "accent must be a #rrggbb colour".into()));
    }
    Ok(())
}

/// Whether `accent` is `#` and six hex digits.
fn valid_accent(accent: &str) -> bool {
    accent.len() == 7
        && accent.starts_with('#')
        && accent[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// The name a provider shows: its own label, else its preset's, else its id.
pub(crate) fn display_label(p: &ProviderConfig) -> String {
    p.label.clone().unwrap_or_else(|| label_of(p))
}

/// Writes `bytes` to `path` through a temporary file created with `mode`.
/// A private mode (no group or other bits, 0600) goes through [`apz_os::private::write`]; any other
/// mode is applied to the new file on Unix (ignored on Windows).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    if mode & 0o077 == 0 {
        return apz_os::private::write(path, bytes);
    }
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// The vault name of a provider's command credential.
fn command_secret(id: &str) -> String {
    name_for(&format!("{id}-command"))
}

/// The vault name of one of a provider's header values.
fn header_secret(id: &str, header: &str) -> String {
    name_for(&format!("{id}-header-{header}"))
}

/// A provider's literal credentials, by vault name: its command (`Auth::Command`) and its header values, which may
/// hold literal tokens. plaza.db keeps them blank and the vault holds them.
fn literal_secrets(p: &ProviderConfig) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Auth::Command(command) = &p.auth
        && !command.is_empty()
    {
        out.push((command_secret(&p.id), command.clone()));
    }
    for (header, value) in &p.extra_headers {
        if !value.is_empty() {
            out.push((header_secret(&p.id, header), value.clone()));
        }
    }
    out
}

/// Blanks a provider's command credential and header values, which may hold literal tokens: what the store keeps.
fn blank_secrets(p: &mut ProviderConfig) {
    if let Auth::Command(command) = &mut p.auth {
        command.clear();
    }
    for value in p.extra_headers.values_mut() {
        value.clear();
    }
}

/// Keeps, for each blanked command credential or header value of `providers`, the live value of the same provider.
fn keep_live_secrets(providers: &mut [ProviderConfig], live: &[ProviderConfig]) {
    for p in providers.iter_mut() {
        let Some(old) = live.iter().find(|o| o.id == p.id) else {
            continue;
        };
        if let (Auth::Command(command), Auth::Command(live_command)) = (&mut p.auth, &old.auth)
            && command.is_empty()
        {
            command.clone_from(live_command);
        }
        for (name, value) in p.extra_headers.iter_mut() {
            if let (true, Some(live_value)) = (value.is_empty(), old.extra_headers.get(name)) {
                value.clone_from(live_value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{NewSecret, Scope};
    use apz_gateway::ProviderKind;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// A fake key value. No view, check result or file may carry it.
    const SENTINEL: &str = "SENTINEL-PROVIDER-4417";

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apid-providers-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A booted vault in `dir`, as `App` starts one (the key file is created at boot).
    fn vault_in(dir: &Path) -> Arc<crate::vault::Store> {
        std::fs::create_dir_all(dir).unwrap();
        let vault = crate::vault::Store::open_in(dir, &dir.join("vault.key"));
        vault.boot(apz_plaza::now_ms()).unwrap();
        Arc::new(vault)
    }

    /// Every file under `dir`, recursively.
    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(files_under(&path));
            } else {
                found.push(path);
            }
        }
        found
    }

    /// A vault entry for `name`, bound to `binding`, in `scope`.
    fn put(vault: &crate::vault::Store, name: &str, binding: &str, value: &str, scope: Scope) {
        let new = NewSecret {
            name: name.to_string(),
            kind: "api_key".to_string(),
            label: "test".to_string(),
            value: SecretString::from(value.to_string()),
            scope,
            bindings: vec![binding.to_string()],
        };
        vault.create(new, "owner", apz_plaza::now_ms()).unwrap();
    }

    /// What an agent on `node` (`None`: a revoked node) reads as `name` for the provider anthropic.
    fn agent_reads(
        vault: &Arc<crate::vault::Store>,
        name: &str,
        node: Option<&str>,
    ) -> Option<String> {
        let caller = Caller::Agent {
            cid: 7,
            node: node.map(String::from),
        };
        let read = SecretRead {
            provider: "anthropic",
            caller: &caller,
        };
        VaultSecrets(Arc::clone(vault)).get(name, &read)
    }

    /// A database holding the providers' tables, empty.
    fn db() -> apz_store::rusqlite::Connection {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        conn
    }

    /// A store over `vault`, loaded from `conn`: what a start reads.
    fn load_from(conn: &apz_store::rusqlite::Connection, vault: Arc<crate::vault::Store>) -> Store {
        let store = Store::new(vault);
        store.load(conn).unwrap();
        store
    }

    /// Saves `store`'s rows to `conn`, as the saver does.
    fn save_to(conn: &apz_store::rusqlite::Connection, store: &Store) {
        apz_store::table::replace(conn, def(), &store.rows().unwrap()).unwrap();
    }

    /// A database holding `stored` as the providers' tables.
    fn saved(stored: &Stored) -> apz_store::rusqlite::Connection {
        let conn = db();
        let rows = apz_store::RowSet::one(def(), stored).unwrap();
        apz_store::table::replace(&conn, def(), &rows).unwrap();
        conn
    }

    /// A vault in `dir` as it is after a restart: boot refuses a tampered store, and the vault stays up to say why.
    fn reopen(dir: &Path) -> Arc<crate::vault::Store> {
        let vault = crate::vault::Store::open_in(dir, &dir.join("vault.key"));
        let _ = vault.boot(apz_plaza::now_ms());
        Arc::new(vault)
    }

    /// Changes the binding `from` to `to` in the stored vault outside the API, so its index MAC no longer matches.
    fn tamper_with_the_stored_vault(dir: &Path, from: &str, to: &str) {
        let edited = crate::vault::tamper_rows(
            dir,
            "UPDATE vault_entry_binding SET value = ?2 WHERE value = ?1",
            &[from, to],
        );
        assert_eq!(edited, 1, "the stored vault names {from}");
    }

    #[test]
    fn labels_and_accents_are_checked() {
        let mut cfg = ProviderConfig::new("codex_2", ProviderKind::OpenAi);
        assert!(valid_id(&cfg.id));
        assert!(check_identity(&cfg).is_ok());
        cfg.label = Some("Work account".into());
        cfg.accent = Some("#5cc8ff".into());
        assert!(check_identity(&cfg).is_ok());
        assert_eq!(display_label(&cfg), "Work account");
        cfg.accent = Some("blue".into());
        assert_eq!(check_identity(&cfg).unwrap_err().0, 400);
        cfg.accent = None;
        cfg.label = Some("  ".into());
        assert!(check_identity(&cfg).is_err());
        cfg.label = Some("x".repeat(MAX_LABEL + 1));
        assert!(check_identity(&cfg).is_err());
    }

    #[test]
    fn a_disabled_provider_keeps_its_settings_and_round_trips() {
        let mut cfg = ProviderConfig::new("openai", ProviderKind::OpenAi);
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(
            !text.contains("enabled"),
            "an enabled provider writes no flag: {text}"
        );
        cfg.enabled = false;
        let text = serde_json::to_string(&cfg).unwrap();
        let back: ProviderConfig = serde_json::from_str(&text).unwrap();
        assert!(!back.enabled);
        let old: ProviderConfig =
            serde_json::from_str(r#"{"id":"openai","kind":"openai"}"#).unwrap();
        assert!(old.enabled && old.label.is_none() && old.accent.is_none());
    }

    #[test]
    fn first_load_seeds_the_presets() {
        let dir = scratch("seed");
        let store = test_store(vault_in(&dir));
        let mut ids: Vec<String> = store
            .gateway()
            .providers()
            .into_iter()
            .map(|p| p.id)
            .collect();
        ids.sort();
        let mut want: Vec<String> = SEED_IDS.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(ids, want);
        assert_eq!(want.len(), 16, "the seeded providers: {want:?}");
        assert!(ids.iter().any(|id| id == "openrouter") && ids.iter().any(|id| id == "opencode"));
        // A fresh install records every seed as offered, so a deleted preset is never offered back.
        let mut offered = lock(&store.offered).clone();
        offered.sort();
        assert_eq!(offered, want, "every seed is recorded as offered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_install_gains_the_new_presets_once() {
        let vault = vault_in(&scratch("offered-once"));
        // An install offered only the presets it holds, not the two added since.
        let held: Vec<ProviderConfig> = seed()
            .into_iter()
            .filter(|p| p.id != "openrouter" && p.id != "opencode")
            .collect();
        let offered: Vec<String> = held.iter().map(|p| p.id.clone()).collect();
        let conn = saved(&Stored {
            providers: held.clone(),
            offered,
        });
        let store = load_from(&conn, Arc::clone(&vault));
        let after = store.gateway().providers();
        assert_eq!(after.len(), held.len() + 2);
        for p in &held {
            assert!(after.contains(p), "{} is untouched", p.id);
        }
        for id in ["openrouter", "opencode"] {
            assert_eq!(after.iter().filter(|p| p.id == id).count(), 1, "{id} once");
        }
        // Saved and loaded again, the two come back once and nothing more is added.
        save_to(&conn, &store);
        let again = load_from(&conn, vault).gateway().providers();
        assert_eq!(again.len(), after.len(), "a second load adds nothing");
    }

    #[test]
    fn a_deleted_preset_stays_deleted() {
        let vault = vault_in(&scratch("offered-deleted"));
        let conn = db();
        let store = test_store(Arc::clone(&vault));
        store.remove("opencode").unwrap();
        store.remove("meta").unwrap();
        save_to(&conn, &store);
        let store = load_from(&conn, Arc::clone(&vault));
        let ids: Vec<String> = store
            .gateway()
            .providers()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert!(ids.contains(&"openrouter".to_string()), "{ids:?}");
        assert!(
            !ids.contains(&"opencode".to_string()) && !ids.contains(&"meta".to_string()),
            "{ids:?}"
        );
        drop(store);
    }

    #[test]
    fn a_secret_never_appears_in_the_list_view() {
        let dir = scratch("view");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        store.set_secret("openai", SENTINEL).unwrap();
        let views = serde_json::to_string(&store.views()).unwrap();
        assert!(!views.contains(SENTINEL));
        assert!(views.contains("\"secret\""));
        // It is stored in the vault, and a reload still reads it (for the upstream call).
        let conn = db();
        save_to(&conn, &store);
        let reloaded = load_from(&conn, Arc::clone(&vault));
        let openai = reloaded.find("openai").unwrap();
        assert_eq!(openai.auth, Auth::Env("OPENAI_API_KEY".to_string()));
        let key = reloaded
            .upstream("openai", &Caller::Owner { node: None })
            .ok()
            .flatten()
            .and_then(|(_, key)| key);
        assert_eq!(key.as_deref(), Some(SENTINEL));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_is_written_to_the_vault_and_no_secrets_file() {
        let dir = scratch("mode");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        store.set_secret("mock", "value-for-the-mode-test").unwrap();
        // The mock provider had no named key: it now names one, and the vault holds it.
        assert_eq!(
            store.find("mock").unwrap().auth,
            Auth::Secret("MOCK_API_KEY".to_string())
        );
        let entry = vault
            .list()
            .unwrap()
            .into_iter()
            .find(|e| e["name"] == "MOCK_API_KEY")
            .unwrap();
        assert_eq!(entry["bindings"], json!(["provider:mock"]));
        assert_eq!(entry["scope"]["kind"], "plaza");
        assert!(!dir.join("secrets.json").exists());
        // The key file is private (0600), as the secrets file was: the vault's rows are in plaza.db.
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(dir.join("vault.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_only_on_another_node_shows_out_of_scope() {
        let dir = scratch("scope");
        let vault = vault_in(&dir);
        put(
            &vault,
            "OPENAI_API_KEY",
            "provider:openai",
            "N3-ONLY-4417",
            Scope::Nodes {
                nodes: vec!["n3".into()],
            },
        );
        let store = test_store(Arc::clone(&vault));
        let views = store.views();
        let credential = |id: &str| {
            views
                .iter()
                .find(|v| v["id"] == id)
                .map(|v| v["credential"].clone())
        };
        assert_eq!(credential("openai"), Some(json!("out_of_scope")));
        assert_eq!(credential("mock"), Some(json!("none")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_agent_reads_its_nodes_entry_before_the_plaza_entry() {
        let dir = scratch("nodes");
        let vault = vault_in(&dir);
        let n3 = || Scope::Nodes {
            nodes: vec!["n3".into()],
        };
        put(
            &vault,
            "ANTHROPIC_API_KEY",
            "provider:anthropic",
            "PLAZA-VALUE-4417",
            Scope::Plaza,
        );
        put(
            &vault,
            "ANTHROPIC_API_KEY",
            "provider:anthropic",
            "N3-VALUE-4417",
            n3(),
        );
        put(
            &vault,
            "N3_ONLY_KEY",
            "provider:anthropic",
            "N3-ONLY-4417",
            n3(),
        );
        assert_eq!(
            agent_reads(&vault, "ANTHROPIC_API_KEY", Some("n3")).as_deref(),
            Some("N3-VALUE-4417")
        );
        assert_eq!(
            agent_reads(&vault, "ANTHROPIC_API_KEY", Some("n4")).as_deref(),
            Some("PLAZA-VALUE-4417")
        );
        assert_eq!(
            agent_reads(&vault, "N3_ONLY_KEY", Some("n3")).as_deref(),
            Some("N3-ONLY-4417")
        );
        assert_eq!(agent_reads(&vault, "N3_ONLY_KEY", Some("n4")), None);
        // An agent whose node is revoked reads nothing, not even the plaza entry.
        assert_eq!(agent_reads(&vault, "ANTHROPIC_API_KEY", None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_value_reaches_no_view_no_check_and_no_file() {
        let dir = scratch("leak");
        let store = test_store(vault_in(&dir));
        // A local endpoint that refuses connections: the check fails without a real network call.
        let mut local = ProviderConfig::new("local", ProviderKind::OpenAiCompatible);
        local.base_url = Some("http://127.0.0.1:9/v1".to_string());
        local.models = vec!["model-x".to_string()];
        store.insert(local).unwrap();
        store.set_secret("local", SENTINEL).unwrap();

        let views = serde_json::to_string(&store.views()).unwrap();
        assert!(!views.contains(SENTINEL));
        let checked = store.check_key("local", None);
        assert!(checked.is_err());
        assert!(!format!("{checked:?}").contains(SENTINEL));
        // An upstream error that echoes the key comes back redacted.
        assert_eq!(
            redact(format!("bad key {SENTINEL}"), Some(SENTINEL)),
            "bad key •••"
        );
        // Nothing on disk carries the value: the vault stores ciphertext only.
        assert!(!dir.join("secrets.json").exists());
        for file in files_under(&dir) {
            let bytes = std::fs::read(&file).unwrap();
            assert!(
                !bytes
                    .windows(SENTINEL.len())
                    .any(|w| w == SENTINEL.as_bytes()),
                "{} holds the value",
                file.display()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn openrouter_and_opencode_read_their_env_keys_through_their_bindings() {
        let dir = scratch("openrouter");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        assert_eq!(
            store.find("openrouter").unwrap().auth,
            Auth::Env("OPENROUTER_API_KEY".to_string())
        );
        assert_eq!(
            store.find("opencode").unwrap().auth,
            Auth::Env("OPENCODE_API_KEY".to_string())
        );
        // The env import binds each key to its provider's binding (vault startup reads these pairs).
        let refs = crate::vault::credential_refs(&store.gateway().providers());
        assert!(refs.contains(&(
            "provider:openrouter".to_string(),
            name_for("OPENROUTER_API_KEY")
        )));
        assert!(refs.contains(&(
            "provider:opencode".to_string(),
            name_for("OPENCODE_API_KEY")
        )));
        // Both bindings are valid, and the owner's key resolves through each provider's binding.
        put(
            &vault,
            "OPENROUTER_API_KEY",
            "provider:openrouter",
            SENTINEL,
            Scope::Plaza,
        );
        put(
            &vault,
            "OPENCODE_API_KEY",
            "provider:opencode",
            SENTINEL,
            Scope::Plaza,
        );
        let owner = Caller::Owner { node: None };
        assert_eq!(
            store
                .upstream("openrouter", &owner)
                .unwrap()
                .and_then(|(_, key)| key)
                .as_deref(),
            Some(SENTINEL)
        );
        assert_eq!(
            store
                .upstream("opencode", &owner)
                .unwrap()
                .and_then(|(_, key)| key)
                .as_deref(),
            Some(SENTINEL)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_headers_are_listed_for_a_provider_that_has_them() {
        let dir = scratch("headers");
        let store = test_store(vault_in(&dir));
        let mut providers = store.gateway().providers();
        let openrouter = providers.iter_mut().find(|p| p.id == "openrouter").unwrap();
        openrouter.extra_headers.insert(
            "HTTP-Referer".to_string(),
            "https://plaza.example".to_string(),
        );
        let expected: Vec<(String, String)> =
            openrouter.extra_headers.clone().into_iter().collect();
        store.commit(providers).unwrap();
        assert!(expected.contains(&(
            "HTTP-Referer".to_string(),
            "https://plaza.example".to_string()
        )));
        assert_eq!(store.extra_headers("openrouter"), expected);
        assert!(store.extra_headers("no-such-provider").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn views_and_model_entries_carry_the_group() {
        let dir = scratch("group");
        let store = test_store(vault_in(&dir));
        let views = store.views();
        let group_of = |id: &str| {
            views
                .iter()
                .find(|v| v["id"] == id)
                .map(|v| v["group"].clone())
        };
        assert_eq!(group_of("openai-decisions"), Some(json!("decisions")));
        assert_eq!(group_of("typesafe"), Some(json!("decisions")));
        assert_eq!(group_of("openai"), Some(json!("chat")));
        assert_eq!(group_of("mistral"), Some(json!("chat")));
        let typesafe = ProviderConfig::new("typesafe", ProviderKind::SystemOne);
        assert_eq!(
            model_entry(&typesafe, ModelInfo::new("jev-latest"))["group"],
            "decisions"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn meta_contributor_models_are_flagged_in_the_model_list() {
        let meta = seed().into_iter().find(|p| p.id == "meta").unwrap();
        let flag = |id: &str| model_entry(&meta, ModelInfo::new(id))["trains_on_data"].clone();
        assert_eq!(flag("muse-spark-1.3-contributor"), json!(true));
        assert_eq!(flag("muse-spark-1.2-contributor"), json!(true));
        assert_eq!(flag("muse-spark-1.3"), json!(false));
        assert_eq!(flag("muse-wonder-1.0"), json!(false));
    }

    #[test]
    fn consumers_follow_provider_registrations() {
        // The consumers list follows the providers.
        {
            let dir = scratch("consumers");
            let vault = vault_in(&dir);
            let store = test_store(Arc::clone(&vault));
            let wants = |vault: &crate::vault::Store| -> Vec<String> {
                vault
                    .consumers(&[])
                    .unwrap()
                    .iter()
                    .filter(|c| c["binding"] == "provider:local-x")
                    .map(|c| c["wants"].as_str().unwrap_or_default().to_string())
                    .collect()
            };
            let mut local = ProviderConfig::new("local-x", ProviderKind::OpenAiCompatible);
            local.auth = Auth::Env("LOCAL_X_KEY".to_string());
            store.insert(local.clone()).unwrap();
            assert_eq!(wants(&vault), vec!["LOCAL_X_KEY".to_string()]);
            // A changed secret name replaces the registration.
            local.auth = Auth::Secret("other-key".to_string());
            store.replace("local-x", local).unwrap();
            assert_eq!(wants(&vault), vec!["OTHER_KEY".to_string()]);
            // A removed provider leaves nothing behind.
            store.remove("local-x").unwrap();
            assert!(wants(&vault).is_empty());
            let _ = std::fs::remove_dir_all(&dir);
        }

        // Each bedrock provider registers its own aws pair and renames and removals keep it right.
        {
            let dir = scratch("bedrock-consumers");
            let vault = vault_in(&dir);
            let store = test_store(Arc::clone(&vault));
            let wants = |vault: &crate::vault::Store, id: &str| -> Vec<String> {
                vault
                    .consumers(&[])
                    .unwrap()
                    .iter()
                    .filter(|c| c["binding"] == provider_binding(id))
                    .map(|c| c["wants"].as_str().unwrap_or_default().to_string())
                    .collect()
            };
            let all = BEDROCK_KEYS.map(String::from);
            store
                .insert(ProviderConfig::new("aws-a", ProviderKind::Bedrock))
                .unwrap();
            store
                .insert(ProviderConfig::new("aws-b", ProviderKind::Bedrock))
                .unwrap();
            assert_eq!(wants(&vault, "aws-a"), all);
            assert_eq!(wants(&vault, "aws-b"), all);
            // Renamed: aws-a leaves its binding, the new id registers the pair, aws-b keeps its own.
            store.remove("aws-a").unwrap();
            store
                .insert(ProviderConfig::new("aws-c", ProviderKind::Bedrock))
                .unwrap();
            assert!(wants(&vault, "aws-a").is_empty());
            assert_eq!(wants(&vault, "aws-c"), all);
            assert_eq!(wants(&vault, "aws-b"), all);
            // A plain edit keeps the registration; a provider that stops being Bedrock-kind drops it.
            let mut edited = ProviderConfig::new("aws-c", ProviderKind::Bedrock);
            edited.base_url = Some("https://bedrock.example".to_string());
            store.replace("aws-c", edited).unwrap();
            assert_eq!(wants(&vault, "aws-c"), all);
            store
                .replace(
                    "aws-b",
                    ProviderConfig::new("aws-b", ProviderKind::OpenAiCompatible),
                )
                .unwrap();
            assert!(wants(&vault, "aws-b").is_empty());
            assert_eq!(wants(&vault, "aws-c"), all);
            store.remove("aws-c").unwrap();
            assert!(wants(&vault, "aws-c").is_empty());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    #[allow(unsafe_code)] // the test sets a process variable on purpose, to show a key is never read from it
    fn a_key_is_served_from_the_vault_by_node_not_the_process_environment() {
        let dir = scratch("served");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        let mut local = ProviderConfig::new("local-env", ProviderKind::OpenAiCompatible);
        local.auth = Auth::Env("SERVED_KEY_4417".to_string());
        store.insert(local).unwrap();
        let key = |node: Option<&str>| {
            let caller = Caller::Agent {
                cid: 7,
                node: node.map(String::from),
            };
            store
                .upstream("local-env", &caller)
                .ok()
                .flatten()
                .and_then(|(_, key)| key)
        };
        // The process holds a value, but the vault holds no entry of that name: nothing is served.
        unsafe { std::env::set_var("SERVED_KEY_4417", "PROCESS-VALUE-4417") };
        assert_eq!(key(Some("n3")), None);
        // Once the value is in the vault (the import's plaza entry, and a node's own entry), each node gets its own.
        put(
            &vault,
            "SERVED_KEY_4417",
            "provider:local-env",
            "PLAZA-VALUE-4417",
            Scope::Plaza,
        );
        put(
            &vault,
            "SERVED_KEY_4417",
            "provider:local-env",
            "N3-VALUE-4417",
            Scope::Nodes {
                nodes: vec!["n3".into()],
            },
        );
        assert_eq!(key(Some("n3")).as_deref(), Some("N3-VALUE-4417"));
        assert_eq!(key(Some("n4")).as_deref(), Some("PLAZA-VALUE-4417"));
        unsafe { std::env::remove_var("SERVED_KEY_4417") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_node_scoped_aws_pair_beats_the_plaza_pair_for_one_bedrock_provider_and_not_the_other() {
        let dir = scratch("bedrock-pair");
        let vault = vault_in(&dir);
        // Fake values: the plaza pair both providers share, and the pair node n3 holds for `aws-a` only.
        let plaza = ["AKIAFAKESENTINEL0001", "SENTINEL-AWS-SECRET-7"];
        let n3 = ["AKIAFAKESENTINEL0003", "SENTINEL-AWS-SECRET-3"];
        for (i, name) in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"]
            .into_iter()
            .enumerate()
        {
            for binding in ["provider:aws-a", "provider:aws-b"] {
                let value = SecretString::from(plaza[i].to_string());
                vault
                    .put_plaza(name, binding, "test", &value, OWNER, apz_plaza::now_ms())
                    .unwrap();
            }
            put(
                &vault,
                name,
                "provider:aws-a",
                n3[i],
                Scope::Nodes {
                    nodes: vec!["n3".into()],
                },
            );
        }
        let read = |provider: &str, name: &str, node: &str| {
            let caller = Caller::Agent {
                cid: 7,
                node: Some(node.to_string()),
            };
            let read = SecretRead {
                provider,
                caller: &caller,
            };
            VaultSecrets(Arc::clone(&vault)).get(name, &read)
        };
        assert_eq!(
            read("aws-a", "AWS_ACCESS_KEY_ID", "n3").as_deref(),
            Some(n3[0])
        );
        assert_eq!(
            read("aws-a", "AWS_SECRET_ACCESS_KEY", "n3").as_deref(),
            Some(n3[1])
        );
        assert_eq!(
            read("aws-a", "AWS_ACCESS_KEY_ID", "n4").as_deref(),
            Some(plaza[0])
        );
        // aws-b holds no node entry: node n3 gets the plaza pair, not aws-a's.
        assert_eq!(
            read("aws-b", "AWS_ACCESS_KEY_ID", "n3").as_deref(),
            Some(plaza[0])
        );
        assert_eq!(
            read("aws-b", "AWS_SECRET_ACCESS_KEY", "n3").as_deref(),
            Some(plaza[1])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_aws_pair_view_reports_secret_missing_out_of_scope_and_sealed() {
        let dir = scratch("bedrock-view");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        let credential = |store: &Store, id: &str| {
            store.view(&store.find(id).unwrap())["credential"]
                .as_str()
                .unwrap()
                .to_string()
        };
        for id in ["aws-v", "aws-w", "aws-x"] {
            store
                .insert(ProviderConfig::new(id, ProviderKind::Bedrock))
                .unwrap();
        }
        // No pair yet: missing, never none.
        assert_eq!(credential(&store, "aws-v"), "missing");
        // The access key alone is not a pair.
        put(
            &vault,
            "AWS_ACCESS_KEY_ID",
            "provider:aws-v",
            SENTINEL,
            Scope::Plaza,
        );
        assert_eq!(credential(&store, "aws-v"), "missing");
        put(
            &vault,
            "AWS_SECRET_ACCESS_KEY",
            "provider:aws-v",
            SENTINEL,
            Scope::Plaza,
        );
        assert_eq!(credential(&store, "aws-v"), "secret");
        // A pair held only by node n3 is out of scope on this machine.
        for name in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"] {
            put(
                &vault,
                name,
                "provider:aws-x",
                SENTINEL,
                Scope::Nodes {
                    nodes: vec!["n3".into()],
                },
            );
        }
        assert_eq!(credential(&store, "aws-x"), "out_of_scope");
        assert_eq!(credential(&store, "aws-w"), "missing");
        // A sealed vault answers sealed for every pair.
        vault.seal("owner", apz_plaza::now_ms()).unwrap();
        assert_eq!(credential(&store, "aws-v"), "sealed");
        assert_eq!(credential(&store, "aws-w"), "sealed");
        assert_eq!(credential(&store, "aws-x"), "sealed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[allow(unsafe_code)] // a process variable is set on purpose: a sealed vault must not fall back to it
    fn a_sealed_vault_serves_no_key_not_even_from_the_process_environment() {
        let dir = scratch("sealed");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        let mut local = ProviderConfig::new("sealed-env", ProviderKind::OpenAiCompatible);
        local.auth = Auth::Env("SEALED_KEY_4417".to_string());
        store.insert(local).unwrap();
        put(
            &vault,
            "SEALED_KEY_4417",
            "provider:sealed-env",
            "PLAZA-VALUE-4417",
            Scope::Plaza,
        );
        unsafe { std::env::set_var("SEALED_KEY_4417", "PROCESS-VALUE-4417") };
        let caller = Caller::Agent {
            cid: 7,
            node: Some("n3".to_string()),
        };
        let served = store
            .upstream("sealed-env", &caller)
            .unwrap()
            .and_then(|(_, key)| key);
        assert_eq!(served.as_deref(), Some("PLAZA-VALUE-4417"));
        vault.seal("owner", apz_plaza::now_ms()).unwrap();
        assert_eq!(
            store.upstream("sealed-env", &caller).unwrap_err(),
            VaultError::Sealed
        );
        // The gateway's read is refused too, and falls back to nothing.
        let read = SecretRead {
            provider: "sealed-env",
            caller: &caller,
        };
        assert_eq!(
            VaultSecrets(Arc::clone(&vault)).get("SEALED_KEY_4417", &read),
            None
        );
        unsafe { std::env::remove_var("SEALED_KEY_4417") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tampered_vault_refuses_the_key_with_its_reason() {
        let dir = scratch("tampered");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        let mut local = ProviderConfig::new("tamp-env", ProviderKind::OpenAiCompatible);
        local.auth = Auth::Env("TAMPERED_KEY_4417".to_string());
        store.insert(local).unwrap();
        put(
            &vault,
            "TAMPERED_KEY_4417",
            "provider:tamp-env",
            "PLAZA-VALUE-4417",
            Scope::Plaza,
        );
        let conn = db();
        save_to(&conn, &store);
        drop(store);
        drop(vault);
        tamper_with_the_stored_vault(&dir, "provider:tamp-env", "provider:tamp-xxx");
        let vault = reopen(&dir);
        let store = load_from(&conn, Arc::clone(&vault));
        let caller = Caller::Agent {
            cid: 7,
            node: Some("n3".to_string()),
        };
        match store.upstream("tamp-env", &caller) {
            Err(VaultError::Tampered(reason)) => assert!(reason.contains("index MAC"), "{reason}"),
            _ => panic!("expected the tamper refusal"),
        }
        let read = SecretRead {
            provider: "tamp-env",
            caller: &caller,
        };
        let source = VaultSecrets(Arc::clone(&vault));
        assert!(matches!(
            source.fetch("TAMPERED_KEY_4417", &read),
            Err(SecretReason::Tampered(_))
        ));
        assert_eq!(source.get("TAMPERED_KEY_4417", &read), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_agent_off_the_node_holding_the_key_gets_out_of_scope() {
        let dir = scratch("out-of-scope");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        let mut local = ProviderConfig::new("scoped-env", ProviderKind::OpenAiCompatible);
        local.auth = Auth::Env("SCOPED_KEY_4417".to_string());
        store.insert(local).unwrap();
        put(
            &vault,
            "SCOPED_KEY_4417",
            "provider:scoped-env",
            "N3-ONLY-4417",
            Scope::Nodes {
                nodes: vec!["n3".into()],
            },
        );
        let on = |node: &str| Caller::Agent {
            cid: 7,
            node: Some(node.to_string()),
        };
        let on_n3 = store
            .upstream("scoped-env", &on("n3"))
            .unwrap()
            .and_then(|(_, key)| key);
        assert_eq!(on_n3.as_deref(), Some("N3-ONLY-4417"));
        assert_eq!(
            store.upstream("scoped-env", &on("n4")).unwrap_err(),
            VaultError::OutOfScope
        );
        let caller = on("n4");
        let read = SecretRead {
            provider: "scoped-env",
            caller: &caller,
        };
        assert_eq!(
            VaultSecrets(Arc::clone(&vault)).fetch("SCOPED_KEY_4417", &read),
            Err(SecretReason::OutOfScope)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The deny lines in the vault's audit log.
    fn deny_lines(vault: &crate::vault::Store) -> usize {
        vault
            .audit_page(0, usize::MAX)
            .unwrap_or_default()
            .iter()
            .filter(|line| line["event"] == "deny")
            .count()
    }

    #[test]
    fn a_refused_lookup_is_audited_once_per_call() {
        let dir = scratch("audit-once");
        let vault = vault_in(&dir);
        let store = test_store(Arc::clone(&vault));
        let mut local = ProviderConfig::new("n3-env", ProviderKind::OpenAiCompatible);
        local.auth = Auth::Env("N3_KEY_4417".to_string());
        local.base_url = Some("http://127.0.0.1:9/v1".to_string());
        store.insert(local).unwrap();
        put(
            &vault,
            "N3_KEY_4417",
            "provider:n3-env",
            "N3-ONLY-4417",
            Scope::Nodes {
                nodes: vec!["n3".into()],
            },
        );
        let on_n4 = Caller::Agent {
            cid: 7,
            node: Some("n4".to_string()),
        };
        // The upstream path: one lookup, one deny line.
        let before = deny_lines(&vault);
        assert_eq!(
            store.upstream("n3-env", &on_n4).unwrap_err(),
            VaultError::OutOfScope
        );
        assert_eq!(deny_lines(&vault) - before, 1);
        // The gateway path: one refused call, one lookup, one deny line.
        let before = deny_lines(&vault);
        let err = store
            .gateway()
            .list_models_for("n3-env", &on_n4)
            .unwrap_err();
        assert!(
            matches!(err, apz_gateway::GatewayError::Vault { status: 403, .. }),
            "{err:?}"
        );
        assert_eq!(deny_lines(&vault) - before, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_and_malformed_ids_are_refused() {
        let dir = scratch("ids");
        let store = test_store(vault_in(&dir));
        assert_eq!(
            store
                .insert(ProviderConfig::new("openai", ProviderKind::OpenAi))
                .unwrap_err()
                .0,
            409
        );
        assert_eq!(
            store
                .insert(ProviderConfig::new("Bad_ID", ProviderKind::Mock))
                .unwrap_err()
                .0,
            400
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A provider with a command, two headers and an extra body, as the owner saved it.
    fn stored_gateway() -> ProviderConfig {
        let mut gw = ProviderConfig::new("gw", ProviderKind::OpenAiCompatible);
        gw.base_url = Some("http://127.0.0.1:9/v1".to_string());
        gw.auth = Auth::Command(format!("echo {SENTINEL}"));
        gw.extra_headers.insert(
            "HTTP-Referer".to_string(),
            format!("https://{SENTINEL}.test"),
        );
        gw.extra_headers
            .insert("X-Title".to_string(), "AgentPlaza".to_string());
        gw.extra_body = json!({ "reasoning": { "effort": "low" } });
        gw
    }

    #[test]
    fn redacted_values_are_restored_or_refused() {
        // An edit of the view keeps the redacted values and extra body.
        {
            let dir = scratch("edit-redacted");
            let store = test_store(vault_in(&dir));
            let stored = stored_gateway();
            store.insert(stored.clone()).unwrap();

            // The view shows the markers and the extra body, never the command or the header values.
            let view = store.views().into_iter().find(|v| v["id"] == "gw").unwrap();
            assert_eq!(view["auth"], json!({ "command": REDACTED }));
            assert_eq!(
                view["extra_headers"],
                json!({ "HTTP-Referer": REDACTED, "X-Title": REDACTED })
            );
            assert_eq!(view["extra_body"], stored.extra_body);
            assert!(!view.to_string().contains(SENTINEL));

            // An Edit that sends the view back as it is changes nothing it redacted.
            let mut body = stored.clone();
            body.auth = Auth::Command(REDACTED.to_string());
            body.extra_headers = stored
                .extra_headers
                .keys()
                .map(|k| (k.clone(), REDACTED.to_string()))
                .collect();
            body.extra_body = Value::Null;
            store.replace("gw", body).unwrap();
            assert_eq!(store.find("gw").unwrap(), stored);

            // A real value replaces the stored one; the other redacted header and the command keep theirs.
            let mut edited = stored.clone();
            edited
                .extra_headers
                .insert("X-Title".to_string(), "Renamed".to_string());
            edited.auth = Auth::Command(REDACTED.to_string());
            edited.extra_body = json!({ "reasoning": { "effort": "high" } });
            store.replace("gw", edited).unwrap();
            let after = store.find("gw").unwrap();
            assert_eq!(after.auth, stored.auth);
            assert_eq!(
                after.extra_headers.get("HTTP-Referer"),
                stored.extra_headers.get("HTTP-Referer")
            );
            assert_eq!(
                after.extra_headers.get("X-Title").map(String::as_str),
                Some("Renamed")
            );
            assert_eq!(
                after.extra_body,
                json!({ "reasoning": { "effort": "high" } })
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        // A redacted value with nothing stored is refused.
        {
            let stored = stored_gateway();
            let mut header = stored.clone();
            header
                .extra_headers
                .insert("X-New".to_string(), REDACTED.to_string());
            assert_eq!(restore_redacted(&stored, header).unwrap_err().0, 400);

            let mut plain = stored.clone();
            plain.auth = Auth::None;
            let mut command = plain.clone();
            command.auth = Auth::Command(REDACTED.to_string());
            assert_eq!(restore_redacted(&plain, command).unwrap_err().0, 400);

            // A body that omits extra_body keeps the stored one; a real command is taken as it is.
            let mut body = stored.clone();
            body.auth = Auth::Command("echo fresh".to_string());
            body.extra_body = Value::Null;
            let merged = restore_redacted(&stored, body).unwrap();
            assert_eq!(merged.auth, Auth::Command("echo fresh".to_string()));
            assert_eq!(merged.extra_body, stored.extra_body);
        }
    }

    #[test]
    fn a_listed_window_is_found_with_or_without_the_provider_prefix() {
        let models = json!([
            {"provider": "openrouter", "id": "anthropic/claude-x", "context": 1_000_000},
            {"provider": "anthropic", "id": "claude-sonnet-4-5", "context": 300_000},
            {"provider": "local", "id": "llama", "context": null},
        ]);
        assert_eq!(
            listed_window(&models, "anthropic/claude-sonnet-4-5"),
            Some(300_000)
        );
        assert_eq!(listed_window(&models, "claude-sonnet-4-5"), Some(300_000));
        assert_eq!(
            listed_window(&models, "openrouter/anthropic/claude-x"),
            Some(1_000_000)
        );
        assert_eq!(listed_window(&models, "local/llama"), None);
        assert_eq!(listed_window(&models, "missing/model"), None);
    }

    #[test]
    fn a_known_family_gives_its_window_and_a_model_outside_the_table_none() {
        assert_eq!(known_window("anthropic/claude-opus-5-5"), Some(200_000));
        assert_eq!(known_window("claude-haiku-5-5"), Some(200_000));
        assert_eq!(
            known_window("openrouter/anthropic/claude-sonnet-4-5"),
            Some(200_000)
        );
        assert_eq!(known_window("openai/gpt-5-mini"), Some(400_000));
        assert_eq!(known_window("deepseek/deepseek-chat"), Some(128_000));
        assert_eq!(known_window("ollama/llama3"), None);
        assert_eq!(known_window("local/qwen2.5-coder"), None);
    }

    /// A command credential and header values live in the vault, never in plaza.db, and come back after a restart.
    #[test]
    fn a_command_and_header_values_are_kept_in_the_vault_and_come_back_after_a_restart() {
        let dir = scratch("literal-secrets");
        let vault = vault_in(&dir);
        let conn = db();
        let store = load_from(&conn, Arc::clone(&vault));
        store.sync_vault();
        store.insert(stored_gateway()).unwrap();
        save_to(&conn, &store);
        let rows = format!("{:?}", store.rows().unwrap().tables);
        assert!(
            !rows.contains(SENTINEL),
            "plaza.db keeps neither the command nor the header values: {rows}"
        );

        // A restart: the rows load blank, and the vault fills them once it is up.
        let restarted = load_from(&conn, Arc::clone(&vault));
        assert_eq!(
            restarted.find("gw").unwrap().auth,
            Auth::Command(String::new())
        );
        restarted.sync_vault();
        assert_eq!(restarted.find("gw").unwrap(), stored_gateway());
        // A preset's fixed headers reached the vault at the first start too.
        let openrouter = |store: &Store| store.find("openrouter").unwrap().extra_headers;
        assert!(openrouter(&store).values().all(|v| !v.is_empty()));
        assert_eq!(openrouter(&restarted), openrouter(&store));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A sealed vault leaves the blanked values blank; its unseal fills them.
    #[test]
    fn a_sealed_vault_leaves_the_command_blank_until_it_is_unsealed() {
        let dir = scratch("literal-sealed");
        let vault = vault_in(&dir);
        let conn = db();
        let store = load_from(&conn, Arc::clone(&vault));
        store.insert(stored_gateway()).unwrap();
        save_to(&conn, &store);
        vault.seal("owner", apz_plaza::now_ms()).unwrap();

        let restarted = load_from(&conn, Arc::clone(&vault));
        restarted.sync_vault();
        assert_eq!(
            restarted.find("gw").unwrap().auth,
            Auth::Command(String::new())
        );
        // An edit that brings a new command needs the vault, and changes nothing without it.
        let mut edited = stored_gateway();
        edited.auth = Auth::Command("echo other".to_string());
        assert_eq!(restarted.replace("gw", edited).unwrap_err().0, 503);

        vault
            .unseal(crate::vault::Unlock::Keyfile, "owner", apz_plaza::now_ms())
            .unwrap();
        restarted.sync_vault();
        assert_eq!(restarted.find("gw").unwrap(), stored_gateway());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
