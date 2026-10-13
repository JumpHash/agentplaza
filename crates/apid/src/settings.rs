//! Plaza settings and the owner's model policy (`settings.json`).
//!
//! OWNER: slice "providers". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_gateway::ModelRef;
use apz_http::{Response, Router};
use apz_plaza::social::{self, Prune};
use serde::{Deserialize, Serialize};
use serde_json::json;

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
    /// Minutes between an idle agent's outreach rounds (finding people to work with); 0 = off.
    pub outreach_minutes: u32,
    /// Whether apid resizes agents' VM memory with their work (through the balloon).
    pub memory_autoscale: bool,
    /// How much of the plaza's conversations and board is kept, and for how long.
    pub retention: Retention,
    /// What the plaza does to an agent that breaks a law (see `sanctions.rs`).
    pub sanctions: Sanctions,
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
            outreach_minutes: crate::society::DEFAULT_OUTREACH_MINUTES,
            memory_autoscale: true,
            retention: Retention::default(),
            sanctions: Sanctions::default(),
        }
    }
}

/// The most credits fines may take from one agent in seven days, and the most one fine may take
/// (sanctions.yaml, `caps.max_fine_per_week`).
pub const FINES_PER_WEEK_MAX: u64 = 1000;

/// Sanctions for law violations, by the schedule in sanctions.yaml.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sanctions {
    /// Whether violations are sanctioned at all. Off: nothing is applied and no suspension holds.
    pub enabled: bool,
    /// Whether the schedule's fines are taken from agents' earnings.
    pub fines: bool,
    /// Whether probation and lockout refuse the offending tool for their duration.
    pub suspensions: bool,
    /// The most one fine may take, in credits (`0..=FINES_PER_WEEK_MAX`).
    pub max_fine: u64,
}

impl Default for Sanctions {
    fn default() -> Self {
        Self { enabled: true, fines: true, suspensions: true, max_fine: 300 }
    }
}

/// The fewest a retention count may be, so the plaza keeps a usable window.
pub const RETENTION_MIN: u32 = 100;
/// The most a retention count may be: the store's absolute ceiling.
pub const RETENTION_MAX: u32 = 100_000;
/// The most days a retention age may be.
pub const RETENTION_DAYS_MAX: u32 = 3650;

// A retention count may never ask for more than the store holds.
const _: () = assert!(RETENTION_MAX as usize <= social::MAX_POSTS);
const _: () = assert!(RETENTION_MAX as usize <= social::MAX_CONVERSATIONS);

/// What the plaza keeps of its conversations and board: a count of each (the
/// oldest go first past it) and an age in days (`0` keeps them whatever their
/// age). Logs are kept until these say otherwise; the store never refuses a new
/// one for room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retention {
    /// The most exchanges the conversation record keeps (`RETENTION_MIN..=RETENTION_MAX`).
    pub conversations_max: u32,
    /// Exchanges older than this many days go; `0` keeps them whatever their age.
    pub conversations_days: u32,
    /// The most posts the board keeps (`RETENTION_MIN..=RETENTION_MAX`).
    pub posts_max: u32,
    /// Posts older than this many days go; `0` keeps them whatever their age.
    pub posts_days: u32,
    /// The most model calls the usage ledger keeps one row each for (`RETENTION_MIN..=RETENTION_MAX`).
    pub usage_calls_max: u32,
    /// Hourly usage totals older than this many days go; `0` keeps them whatever their age.
    pub usage_days: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            conversations_max: 4096,
            conversations_days: 30,
            posts_max: 1024,
            posts_days: 0,
            usage_calls_max: 20_000,
            usage_days: 365,
        }
    }
}

impl Retention {
    /// The retention as the plaza store applies it.
    pub fn policy(&self) -> Prune {
        const DAY_MS: u64 = 86_400_000;
        let age = |days: u32| (days > 0).then(|| u64::from(days) * DAY_MS);
        Prune {
            conversations_max: usize::try_from(self.conversations_max).unwrap_or(usize::MAX),
            conversations_older_than_ms: age(self.conversations_days),
            posts_max: usize::try_from(self.posts_max).unwrap_or(usize::MAX),
            posts_older_than_ms: age(self.posts_days),
        }
    }
}

pub struct Store {
    path: PathBuf,
    dev_models: bool,
    inner: Mutex<Settings>,
}

/// The model in canonical `provider/model[@effort]` form.
fn canonical(model: &str) -> Result<String, String> {
    ModelRef::parse(model).map(|r| r.to_string()).map_err(|e| e.to_string())
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Orchestrator => "orchestrator",
        Tier::Worker => "worker",
    }
}

/// Whether `host` is a bare hostname: no scheme, path, port or whitespace.
fn is_bare_host(host: &str) -> bool {
    !host.is_empty()
        && !host.contains("://")
        && !host.contains(['/', ':'])
        && !host.chars().any(char::is_whitespace)
}

impl Store {
    /// Loads `settings.json` (defaults on first start; dev defaults in dev mode).
    pub fn load(state_dir: &Path, dev_models: bool) -> std::io::Result<Self> {
        // contract
        let path = state_dir.join("settings.json");
        let settings = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => {
                let mut s = Settings::default();
                if dev_models {
                    s.default_model = "mock/agent".into();
                    s.orchestrator_model = "mock/agent".into();
                }
                s
            }
        };
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
    /// names the allowed list. Workers may run any well-formed model: their
    /// list is the presets offered first, not a limit. The orchestrator tier
    /// stays limited to its list.
    pub fn allowed(&self, tier: Tier, model: &str) -> Result<(), String> {
        // contract
        if self.dev_models {
            return Ok(());
        }
        let wanted = canonical(model)?;
        if tier == Tier::Worker {
            return Ok(());
        }
        let policy = self.get().model_policy;
        let list = match tier {
            Tier::Orchestrator => policy.orchestrator,
            Tier::Worker => policy.worker,
        };
        let names: Vec<String> = list.iter().filter_map(|m| canonical(m).ok()).collect();
        // An entry without an effort pin admits that model at any effort; a pinned entry only itself.
        let base = ModelRef::parse(model).map(|r| r.base_name()).map_err(|e| e.to_string())?;
        if names.contains(&wanted) || names.contains(&base) {
            return Ok(());
        }
        Err(format!(
            "{model} is not allowed for the {} tier; allowed: {}",
            tier_name(tier),
            names.join(", ")
        ))
    }

    /// Validates `new`, writes `settings.json` atomically and replaces the
    /// in-memory settings. Returns the saved value.
    pub fn put(&self, new: Settings) -> Result<Settings, String> {
        self.check(&new)?;
        self.replace(new)
    }

    /// The checks every saved settings value passes (`put` and `restore`).
    fn check(&self, new: &Settings) -> Result<(), String> {
        self.allowed(Tier::Worker, &new.default_model)?;
        self.allowed(Tier::Orchestrator, &new.orchestrator_model)?;
        for (role, model) in &new.role_models {
            self.allowed(tier_of(role), model)?;
        }
        if !(1..=200).contains(&new.max_turns) {
            return Err(format!("max_turns must be 1..=200, got {}", new.max_turns));
        }
        if let Some(effort) = &new.effort {
            const LEVELS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
            if !LEVELS.contains(&effort.as_str()) {
                return Err(format!("unknown effort {effort:?}; expected one of {}", LEVELS.join(", ")));
            }
        }
        if let Some(bad) = new.fetch_allow.iter().find(|h| !is_bare_host(h)) {
            return Err(format!("fetch_allow entry {bad:?} is not a bare hostname"));
        }
        let retention = &new.retention;
        for (name, max) in [
            ("conversations_max", retention.conversations_max),
            ("posts_max", retention.posts_max),
            ("usage_calls_max", retention.usage_calls_max),
        ] {
            if !(RETENTION_MIN..=RETENTION_MAX).contains(&max) {
                return Err(format!(
                    "retention.{name} must be {RETENTION_MIN}..={RETENTION_MAX}, got {max}"
                ));
            }
        }
        for (name, days) in [
            ("conversations_days", retention.conversations_days),
            ("posts_days", retention.posts_days),
            ("usage_days", retention.usage_days),
        ] {
            if days > RETENTION_DAYS_MAX {
                return Err(format!(
                    "retention.{name} must be 0..={RETENTION_DAYS_MAX} (0 keeps everything), got {days}"
                ));
            }
        }
        if new.sanctions.max_fine > FINES_PER_WEEK_MAX {
            return Err(format!("sanctions.max_fine must be 0..={FINES_PER_WEEK_MAX}, got {}", new.sanctions.max_fine));
        }
        Ok(())
    }

    /// Writes `settings.json` atomically and swaps the in-memory settings.
    fn replace(&self, new: Settings) -> Result<Settings, String> {
        let mut guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes = serde_json::to_vec_pretty(&new).map_err(|e| e.to_string())?;
        let tmp = self.path.with_file_name("settings.json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("write settings: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("replace settings: {e}"))?;
        *guard = new.clone();
        Ok(new)
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
    router
        .route("GET", "/v1/settings", |app, _, _| {
            let mut value = match serde_json::to_value(app.settings.get()) {
                Ok(value) => value,
                Err(e) => return Response::json(500, &json!({ "error": e.to_string() })),
            };
            if let Some(obj) = value.as_object_mut() {
                obj.insert("dev_models".into(), json!(app.settings.dev_models()));
            }
            Response::json(200, &value)
        })
        .route("PUT", "/v1/settings", |app, req, _| {
            let new: Settings = match req.json() {
                Ok(settings) => settings,
                Err(e) => return Response::json(400, &json!({ "error": format!("bad settings JSON: {e}") })),
            };
            match app.settings.put(new) {
                Ok(saved) => Response::json(200, &saved),
                Err(msg) => Response::json(400, &json!({ "error": msg })),
            }
        })
}


impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(&self.get()).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let new: Settings = serde_json::from_str(json).map_err(|e| format!("bad settings snapshot: {e}"))?;
        self.check(&new)?;
        self.replace(new).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty state directory under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn allowed_is_per_tier_and_canonical() {
        let dir = temp_dir("allowed");
        let store = Store::load(&dir, false).unwrap();
        // Worker tier: the effort-suffixed entry passes, the plain one does not.
        assert!(store.allowed(Tier::Worker, "anthropic/claude-opus-5-5@low").is_ok());
        assert!(store.allowed(Tier::Worker, "deepseek/deepseek-flash").is_ok());
        assert!(store.allowed(Tier::Worker, "  anthropic/claude-haiku-5-5 ").is_ok());
        // Workers may run any model; their list only holds the presets.
        assert!(store.allowed(Tier::Worker, "anthropic/claude-opus-5-5").is_ok());
        assert!(store.allowed(Tier::Worker, "openai/gpt-4-turbo-2024-04-09").is_ok());
        let err = store.allowed(Tier::Orchestrator, "deepseek/deepseek-flash").unwrap_err();
        assert!(err.contains("not allowed for the orchestrator tier"), "{err}");
        assert!(err.contains("allowed: "), "{err}");
        // Orchestrator tier: only its own list, at any effort when the entry pins none.
        assert!(store.allowed(Tier::Orchestrator, "anthropic/claude-opus-5-5").is_ok());
        assert!(store.allowed(Tier::Orchestrator, "openai/gpt-6-astra@high").is_ok());
        assert!(store.allowed(Tier::Orchestrator, "anthropic/claude-opus-5-5@max").is_ok());
        assert!(store.allowed(Tier::Orchestrator, "deepseek/deepseek-flash@high").is_err());
        assert!(store.allowed(Tier::Orchestrator, "deepseek/deepseek-flash").is_err());
        // Unparseable names fail with the parser's text.
        assert!(store.allowed(Tier::Worker, "no-slash-here").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dev_mode_allows_mock_and_defaults_to_it() {
        let dir = temp_dir("dev");
        let store = Store::load(&dir, true).unwrap();
        assert!(store.allowed(Tier::Worker, "mock/agent").is_ok());
        assert!(store.allowed(Tier::Orchestrator, "anything/at-all").is_ok());
        let s = store.get();
        assert_eq!(s.default_model, "mock/agent");
        assert_eq!(s.orchestrator_model, "mock/agent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_round_trips_and_a_refusal_changes_nothing() {
        let dir = temp_dir("snapshot");
        let store = Store::load(&dir, false).unwrap();
        let mut s = Settings::default();
        s.max_turns = 50;
        store.put(s.clone()).unwrap();
        let json = store.snapshot();

        let other_dir = temp_dir("snapshot-other");
        let other = Store::load(&other_dir, false).unwrap();
        other.restore(&json).unwrap();
        assert_eq!(other.get(), s);
        assert_eq!(Store::load(&other_dir, false).unwrap().get(), s);

        let mut bad = Settings::default();
        bad.max_turns = 0;
        assert!(other.restore(&serde_json::to_string(&bad).unwrap()).is_err());
        assert_eq!(other.get(), s);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other_dir);
    }

    #[test]
    fn put_rejects_bad_fetch_allow_entries() {
        let dir = temp_dir("fetch");
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load(&dir, false).unwrap();
        for bad in ["https://example.com", "example.com/path", "has space.com", "host:8080", ""] {
            let mut s = Settings::default();
            s.fetch_allow = vec![bad.to_string()];
            assert!(store.put(s).is_err(), "{bad:?} should be rejected");
        }
        let mut ok = Settings::default();
        ok.fetch_allow = vec!["example.com".into()];
        assert!(store.put(ok).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_validates_and_persists() {
        let dir = temp_dir("persist");
        let store = Store::load(&dir, false).unwrap();
        // A worker default may be any model; an orchestrator model outside its list is refused.
        let mut any = Settings::default();
        any.default_model = "anthropic/claude-fable-5-1".into();
        assert!(store.put(any).is_ok());
        let mut bad = Settings::default();
        bad.orchestrator_model = "deepseek/deepseek-flash".into();
        assert!(store.put(bad).is_err());
        let mut zero = Settings::default();
        zero.max_turns = 0;
        assert!(store.put(zero).is_err());

        let mut s = Settings::default();
        s.max_turns = 50;
        s.effort = Some("low".into());
        let saved = store.put(s.clone()).unwrap();
        assert_eq!(saved, s);
        assert_eq!(store.get(), s);
        assert!(!dir.join("settings.json.tmp").exists());
        // A fresh store reads the same values back from disk.
        let reloaded = Store::load(&dir, false).unwrap();
        assert_eq!(reloaded.get(), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_is_validated_and_settings_saved_without_it_load_the_defaults() {
        let dir = temp_dir("retention");
        let store = Store::load(&dir, false).unwrap();
        for bad in [
            Retention { conversations_max: RETENTION_MIN - 1, ..Retention::default() },
            Retention { conversations_max: RETENTION_MAX + 1, ..Retention::default() },
            Retention { posts_max: 0, ..Retention::default() },
            Retention { conversations_days: RETENTION_DAYS_MAX + 1, ..Retention::default() },
            Retention { posts_days: 4000, ..Retention::default() },
        ] {
            let mut s = Settings::default();
            s.retention = bad;
            assert!(store.put(s).is_err(), "{bad:?} should be refused");
        }
        let mut ok = Settings::default();
        ok.retention = Retention {
            conversations_max: RETENTION_MIN,
            conversations_days: 0,
            posts_max: RETENTION_MAX,
            posts_days: RETENTION_DAYS_MAX,
            usage_calls_max: RETENTION_MIN,
            usage_days: 0,
        };
        assert_eq!(store.put(ok.clone()).unwrap(), ok);

        // Settings saved before retention existed carry no such key: they load the defaults.
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut().unwrap().remove("retention");
        let loaded: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(loaded.retention, Retention::default());
        assert_eq!(loaded.retention.conversations_max, 4096);

        // The default keeps a month of exchanges and every post until its count goes.
        let policy = Retention::default().policy();
        assert_eq!(policy.conversations_max, 4096);
        assert_eq!(policy.conversations_older_than_ms, Some(30 * 86_400_000));
        assert_eq!(policy.posts_max, 1024);
        assert_eq!(policy.posts_older_than_ms, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
