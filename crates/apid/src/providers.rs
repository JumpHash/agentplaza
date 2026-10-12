//! Model providers (`providers.json`), their credentials (`secrets.json`,
//! 0600, write-only through the API) and the gateway built from them.
//!
//! OWNER: slice "providers". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use apz_gateway::{presets, Auth, Gateway, ProviderConfig, SecretSource};
use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::App;

/// The providers a fresh state directory starts with.
const SEED_IDS: [&str; 10] = [
    "anthropic", "openai", "xai", "deepseek", "gemini", "zai", "moonshot", "ollama", "llamacpp", "mock",
];
/// How long `GET /v1/models` answers from its cache.
const MODEL_CACHE: Duration = Duration::from_secs(60);

/// A failed mutation: the HTTP status and a message that never names a secret's value.
type Failure = (u16, String);

/// Adapts the shared secret map to the gateway's `SecretSource`.
struct Secrets(Arc<Mutex<BTreeMap<String, String>>>);

impl SecretSource for Secrets {
    fn get(&self, name: &str) -> Option<String> {
        lock(&self.0).get(name).cloned()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Store {
    dir: PathBuf,
    env: BTreeMap<String, String>,
    gateway: Gateway,
    secrets: Arc<Mutex<BTreeMap<String, String>>>,
    /// Serializes every mutation of providers and secrets.
    write: Mutex<()>,
    /// The last `GET /v1/models` answer and when it was computed.
    models: Mutex<Option<(Instant, Value)>>,
}

impl Store {
    /// Loads providers and secrets; `env` is the env-file's variables (looked
    /// up before the process environment).
    pub fn load(state_dir: &Path, env: BTreeMap<String, String>) -> io::Result<Self> {
        // contract
        std::fs::create_dir_all(state_dir)?;
        let providers_path = state_dir.join("providers.json");
        let providers: Vec<ProviderConfig> = match std::fs::read(&providers_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(invalid)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let seeded = seed();
                write_atomic(&providers_path, &to_json(&seeded)?, 0o600)?;
                seeded
            }
            Err(e) => return Err(e),
        };
        let secrets_path = state_dir.join("secrets.json");
        let secrets: BTreeMap<String, String> = match std::fs::read(&secrets_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(invalid)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        let secrets = Arc::new(Mutex::new(secrets));
        let lookup = env.clone();
        let gateway = Gateway::builder()
            .providers(providers)
            .secrets(Secrets(secrets.clone()))
            .env(move |name| {
                lookup
                    .get(name)
                    .cloned()
                    .or_else(|| std::env::var(name).ok())
                    .filter(|v| !v.is_empty())
            })
            .build();
        Ok(Self {
            dir: state_dir.to_path_buf(),
            env,
            gateway,
            secrets,
            write: Mutex::new(()),
            models: Mutex::new(None),
        })
    }

    /// The gateway over the current providers and secrets.
    pub fn gateway(&self) -> Gateway {
        // contract
        self.gateway.clone()
    }

    /// Every provider's view, by id. Never carries a secret value.
    fn views(&self) -> Vec<Value> {
        self.gateway.providers().iter().map(|p| self.view(p)).collect()
    }

    fn view(&self, p: &ProviderConfig) -> Value {
        let credential = match &p.auth {
            Auth::None => "none",
            Auth::Command(_) => "command",
            Auth::Env(name) if self.env_has(name) => "env",
            Auth::Env(_) => "missing",
            Auth::Secret(name) if lock(&self.secrets).contains_key(name) => "secret",
            Auth::Secret(_) => "missing",
        };
        let auth = match &p.auth {
            // A command may embed a literal token, so only its presence shows.
            Auth::Command(_) => json!({ "command": "•••" }),
            other => serde_json::to_value(other).unwrap_or(Value::Null),
        };
        let headers: BTreeMap<&str, &str> = p.extra_headers.keys().map(|k| (k.as_str(), "•••")).collect();
        json!({
            "id": p.id,
            "kind": p.kind.as_str(),
            "base_url": p.base_url,
            "auth": auth,
            "models": p.models,
            "options": p.options,
            "extra_headers": headers,
            "credential": credential,
        })
    }

    fn env_has(&self, name: &str) -> bool {
        self.env.get(name).is_some_and(|v| !v.is_empty())
            || std::env::var(name).is_ok_and(|v| !v.is_empty())
    }

    /// A provider's endpoint and key, for the pass-through model gateway (`llm_proxy`): the
    /// configured base URL (or the kind's default for anthropic/openai) and the key from the
    /// secret store or `apid`'s environment. `None` for an unknown provider.
    pub fn upstream(&self, id: &str) -> Option<(String, Option<String>)> {
        let p = self.find(id).ok()?;
        let base = p.base_url.clone().unwrap_or_else(|| match id {
            "openai" => "https://api.openai.com/v1".to_string(),
            _ => "https://api.anthropic.com".to_string(),
        });
        let key = match &p.auth {
            Auth::Secret(name) => self.secrets.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(name).cloned(),
            Auth::Env(name) => self.env.get(name).cloned().or_else(|| std::env::var(name).ok()),
            _ => None,
        };
        Some((base, key))
    }

    fn find(&self, id: &str) -> Result<ProviderConfig, Failure> {
        self.gateway
            .providers()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| (404, format!("no provider `{id}`")))
    }

    /// Saves `providers` and makes them the gateway's providers.
    fn commit(&self, providers: Vec<ProviderConfig>) -> Result<(), Failure> {
        let saved = to_json(&providers).and_then(|b| write_atomic(&self.dir.join("providers.json"), &b, 0o600));
        saved.map_err(io_failure)?;
        self.gateway.set_providers(providers);
        *lock(&self.models) = None;
        Ok(())
    }

    fn insert(&self, cfg: ProviderConfig) -> Result<Value, Failure> {
        let _guard = lock(&self.write);
        if !valid_id(&cfg.id) {
            return Err((400, "id must match [a-z0-9-]{1,40}".into()));
        }
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
        let mut providers = self.gateway.providers();
        let slot = providers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| (404, format!("no provider `{id}`")))?;
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
        self.commit(providers)?;
        Ok(json!({ "deleted": id }))
    }

    /// Stores `value` under the provider's secret name; a provider without a
    /// secret gets `Auth::Secret(id)`.
    fn set_secret(&self, id: &str, value: &str) -> Result<Value, Failure> {
        let _guard = lock(&self.write);
        let mut providers = self.gateway.providers();
        let slot = providers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| (404, format!("no provider `{id}`")))?;
        let name = match slot.auth.clone() {
            Auth::Secret(name) => name,
            _ => id.to_string(),
        };
        slot.auth = Auth::Secret(name.clone());
        let snapshot = {
            let mut secrets = lock(&self.secrets);
            secrets.insert(name, value.to_string());
            secrets.clone()
        };
        save_secrets(&self.dir.join("secrets.json"), &snapshot).map_err(io_failure)?;
        let view = self.view(slot);
        self.commit(providers)?;
        Ok(view)
    }

    /// `GET /v1/models`: every provider's models, cached for [`MODEL_CACHE`].
    fn all_models(&self) -> Value {
        let cached = lock(&self.models)
            .as_ref()
            .filter(|(at, _)| at.elapsed() < MODEL_CACHE)
            .map(|(_, v)| v.clone());
        if let Some(value) = cached {
            return value;
        }
        let mut models = Vec::new();
        let mut errors = Vec::new();
        for p in self.gateway.providers() {
            match self.gateway.list_models(&p.id) {
                Ok(list) => models.extend(
                    list.into_iter().map(|m| json!({ "provider": p.id, "id": m.id, "name": m.name })),
                ),
                Err(e) => errors.push(json!({ "provider": p.id, "error": e.to_string() })),
            }
        }
        let value = json!({ "models": models, "errors": errors });
        *lock(&self.models) = Some((Instant::now(), value.clone()));
        value
    }
}

/// `/v1/provider-kinds`, `/v1/providers…`, `/v1/models`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/provider-kinds", kinds_route)
        .get("/v1/providers", list_route)
        .post("/v1/providers", create_route)
        .get("/v1/providers/{id}", get_route)
        .route("PUT", "/v1/providers/{id}", replace_route)
        .delete("/v1/providers/{id}", delete_route)
        .route("PUT", "/v1/providers/{id}/secret", secret_route)
        .post("/v1/providers/{id}/test", test_route)
        .get("/v1/providers/{id}/models", provider_models_route)
        .get("/v1/models", all_models_route)
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
    match app.providers.gateway().test(id, model.as_deref()) {
        Ok(report) => Response::json(200, &report),
        Err(e) => Response::json(502, &json!({ "error": e.to_string() })),
    }
}

fn provider_models_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if let Err(f) = app.providers.find(id) {
        return failure(f);
    }
    match app.providers.gateway().list_models(id) {
        Ok(models) => Response::json(200, &models),
        Err(e) => Response::json(502, &json!({ "error": e.to_string() })),
    }
}

fn all_models_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &app.providers.all_models())
}

fn failure((status, message): Failure) -> Response {
    Response::json(status, &json!({ "error": message }))
}

/// The providers of a fresh state directory: the presets named in `SEED_IDS`.
fn seed() -> Vec<ProviderConfig> {
    presets()
        .iter()
        .filter(|p| SEED_IDS.contains(&p.id))
        .map(|p| p.config())
        .collect()
}

/// Provider ids: `[a-z0-9-]`, 1 to 40 characters.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn save_secrets(path: &Path, secrets: &BTreeMap<String, String>) -> io::Result<()> {
    write_atomic(path, &to_json(secrets)?, 0o600)
}

fn to_json<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(invalid)
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

fn io_failure(e: io::Error) -> Failure {
    (500, format!("cannot save: {e}"))
}

/// Writes `bytes` to `path` through a temporary file created with `mode`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}


impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        String::new() // contract: D6
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, _json: &str) -> Result<(), String> {
        Ok(()) // contract: D6
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_gateway::ProviderKind;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-providers-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn first_load_seeds_the_presets() {
        let dir = scratch("seed");
        let store = Store::load(&dir, BTreeMap::new()).unwrap();
        let mut ids: Vec<String> = store.gateway().providers().into_iter().map(|p| p.id).collect();
        ids.sort();
        let mut want: Vec<String> = SEED_IDS.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(ids, want);
        assert!(dir.join("providers.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_never_appears_in_the_list_view() {
        let dir = scratch("view");
        let store = Store::load(&dir, BTreeMap::new()).unwrap();
        store.set_secret("openai", "sk-test-SECRET-123").unwrap();
        let views = serde_json::to_string(&store.views()).unwrap();
        assert!(!views.contains("sk-test-SECRET-123"));
        assert!(views.contains("\"secret\""));
        // It is stored, though, and survives a reload.
        let stored = std::fs::read_to_string(dir.join("secrets.json")).unwrap();
        assert!(stored.contains("sk-test-SECRET-123"));
        let reloaded = Store::load(&dir, BTreeMap::new()).unwrap();
        let openai = reloaded.find("openai").unwrap();
        assert_eq!(openai.auth, Auth::Secret("openai".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secrets_file_is_mode_0600() {
        let dir = scratch("mode");
        let store = Store::load(&dir, BTreeMap::new()).unwrap();
        store.set_secret("mock", "value").unwrap();
        let mode = std::fs::metadata(dir.join("secrets.json")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_and_malformed_ids_are_refused() {
        let dir = scratch("ids");
        let store = Store::load(&dir, BTreeMap::new()).unwrap();
        assert_eq!(store.insert(ProviderConfig::new("openai", ProviderKind::OpenAi)).unwrap_err().0, 409);
        assert_eq!(store.insert(ProviderConfig::new("Bad_ID", ProviderKind::Mock)).unwrap_err().0, 400);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
