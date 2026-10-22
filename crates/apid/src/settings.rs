//! Plaza settings and the owner's model policy (`settings.json`).
//!
//! OWNER: slice "providers". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_gateway::ModelRef;
use apz_http::{Response, RouteDoc, Router};
use apz_plaza::social::{self, Prune};
use apz_station::StationSettings;
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
    if role == ORCHESTRATOR_ROLE {
        Tier::Orchestrator
    } else {
        Tier::Worker
    }
}

/// The models each tier's list names (canonical `provider/model[@effort]`). The orchestrator may run
/// only its list. Workers may run any model: their list is the presets offered first, plus the default.
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
    /// Hosts agents may not fetch from: `example.com` (and its subdomains) or `*.example.com`
    /// (subdomains only). Everything else is allowed, subject to the laws; private, loopback and
    /// link-local addresses are always refused by the egress proxy.
    pub fetch_block: Vec<String>,
    pub prices: BTreeMap<String, Price>,
    /// Whether apid resizes agents' VM memory with their work (through the balloon).
    pub memory_autoscale: bool,
    /// Whether the hub checkpoints an agent's whole VM before an irreversible tool call runs (see
    /// `vm_snapshots.rs`). Off: only the owner's explicit checkpoints are taken.
    pub vm_auto_checkpoints: bool,
    /// How much of the plaza's board is kept, and for how long.
    pub retention: Retention,
    /// How the plaza's laws are applied (see `laws.rs`).
    pub laws: LawsSettings,
    /// How payments and model calls are gated and charged (see `payments.rs`, `llm_proxy.rs`).
    pub payments: Payments,
    /// The Lab's settings (`docs/design/foundry.md`, "Hugging Face").
    pub foundry: FoundrySettings,
    /// The owner's explicit confirmation that a contributor model (one that trains its provider's
    /// future models on what it is sent, `apz_gateway::trains_on_data`) may be a default, a role
    /// model or a policy entry. Off: the save refuses those; a per-agent pick is never affected.
    pub allow_training_models: bool,
    /// Minutes a done task waits for a reviewer before the owner is asked to rule on it (an
    /// approval of kind `review`). 0 asks at the scheduler's next tick. See `scheduler.rs`.
    pub review_timeout_minutes: u32,
    /// The recording settings a newly joined workstation starts with (`stations/recordings.rs`); a station's own
    /// settings are edited on the station after that.
    pub workstation_recordings: RecordingDefaults,
    /// The owner's pauses of the board's deliveries (`owner_board.rs`). The pause route sets them;
    /// a settings save keeps the ones in force.
    pub board_pauses: BoardPauses,
    /// The coding engines the owner has enabled. A `code_engine` call names one of these, and the
    /// offer lists only these. Never empty, never repeated.
    pub engines_enabled: Vec<String>,
    /// How the engines run: always `acp` (the Agent Client Protocol). The field stays so a saved value
    /// still loads; see [`ENGINE_MODE`].
    pub engines_mode: String,
    /// Whether editors may connect to the hub's ACP server (`/v1/acp`). Off until the owner turns it on.
    pub acp_enabled: bool,
    /// Whether an editor may apply a pull request its agent opened to the editor's project, when the
    /// owner says yes in the editor. Off until the owner turns it on; the agent never writes the project itself.
    pub acp_write_back: bool,
    /// Whether editors may speak the ACP v2 draft to the hub, next to v1 (`/v1/acp`): an editor that asks
    /// for protocol 2 gets it, and every other editor keeps v1. Off until the owner turns it on.
    pub acp_v2: bool,
    /// Minutes an agent must be idle before the sweeper puts it to sleep (its VM snapshotted and
    /// stopped, `sleep.rs`). 0: agents never sleep on their own.
    pub idle_sleep_min: u32,
    /// Roles whose agents never sleep on their own. The orchestrator is awake so the owner can chat
    /// with it at any time.
    pub sleep_never_roles: Vec<String>,
    /// Seconds an agent must be idle before the sweeper dozes it (`sleep.rs`): its VM paused and its
    /// memory paged out, resumed in milliseconds. 0: agents never doze. A sleep due sooner wins.
    pub doze_idle_s: u32,
    /// Shared data volumes (noded's `/v1/volumes`) by role: every agent of the role spawned from now
    /// on mounts them read-only at `/data/<name>`. At most four per role.
    pub role_volumes: BTreeMap<String, Vec<String>>,
    /// Seconds between the hub's background checks of every provider (`provider_health.rs`): its key,
    /// its model listing, its latency. 0: no background checks; the owner checks by hand.
    pub health_interval_s: u32,
    /// The owner's favourite models, `<provider>/<model>`: the model pickers list them first.
    pub favourite_models: Vec<String>,
}

/// The default for `health_interval_s`: five minutes.
pub const DEFAULT_HEALTH_INTERVAL_S: u32 = 300;

/// The most `health_interval_s` may be: a day.
pub const MAX_HEALTH_INTERVAL_S: u32 = 86_400;

/// The most favourite models the owner may keep.
pub const MAX_FAVOURITE_MODELS: usize = 200;

/// The owner's pauses of the board's deliveries: the whole board, and the projects whose boards are
/// paused. A paused board's posts are stored, and their mentions are not delivered until it resumes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoardPauses {
    /// Whether the whole board is paused.
    pub all: bool,
    /// The projects whose boards are paused.
    pub projects: BTreeSet<String>,
}

impl BoardPauses {
    /// Whether the deliveries for `project`'s board (`None` for the whole board) are held.
    pub fn holds(&self, project: Option<&str>) -> bool {
        self.all || project.is_some_and(|id| self.projects.contains(id))
    }
}

/// The default for `review_timeout_minutes`: half an hour.
pub const DEFAULT_REVIEW_TIMEOUT_MINUTES: u32 = 30;

/// The coding engines a `code_engine` call may name.
pub const ENGINE_IDS: [&str; 12] = [
    "claude", "codex", "opencode", "gemini", "qwen", "kilo", "cline", "copilot", "goose", "kimi",
    "auggie", "vibe",
];

/// The engines enabled out of the box. Gemini CLI, Qwen Code, Kilo Code, Cline, GitHub Copilot, Goose,
/// Kimi Code, Auggie and Mistral Vibe are named but off until the owner enables them.
pub const DEFAULT_ENGINE_IDS: [&str; 3] = ["claude", "codex", "opencode"];

/// The one transport the engines run over, the Agent Client Protocol: the only `engines_mode`. The
/// headless run is gone, so a saved `headless` loads as this (see `repair`).
pub const ENGINE_MODE: &str = "acp";
/// The default for `idle_sleep_min`: ten minutes.
pub const DEFAULT_IDLE_SLEEP_MIN: u32 = 10;

/// The most `idle_sleep_min` may be: a day.
pub const MAX_IDLE_SLEEP_MIN: u32 = 1440;

/// The default for `doze_idle_s`: 0, no doze. The doze tier is behind this setting until a soak has
/// passed on a real node; 60 (a minute) is the value it is meant for.
pub const DEFAULT_DOZE_IDLE_S: u32 = 0;

/// The most `doze_idle_s` may be: a day.
pub const MAX_DOZE_IDLE_S: u32 = 86_400;

/// A workstation's recording settings, as the hub's defaults hold them (the same fields as `StationSettings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingDefaults {
    /// Whether a lease on the station is recorded.
    pub record: bool,
    /// How many days ended recordings are kept.
    pub retention_days: u32,
    /// Frames per second a pixel lease keeps (0: actions only; at most 5).
    pub record_fps: u8,
    /// Whether the owner's takeovers of a session are recorded too.
    pub record_takeovers: bool,
}

impl Default for RecordingDefaults {
    fn default() -> Self {
        let station = StationSettings::default();
        Self {
            record: station.record,
            retention_days: station.retention_days,
            record_fps: station.record_fps,
            record_takeovers: station.record_takeovers,
        }
    }
}

/// The Lab's settings. `hf_max_gb`: the most a Hugging Face import may download, in GiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FoundrySettings {
    pub hf_max_gb: u32,
}

impl Default for FoundrySettings {
    fn default() -> Self {
        Self { hf_max_gb: 20 }
    }
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
            fetch_block: Vec::new(),
            prices: BTreeMap::new(),
            memory_autoscale: true,
            vm_auto_checkpoints: true,
            retention: Retention::default(),
            laws: LawsSettings::default(),
            payments: Payments::default(),
            foundry: FoundrySettings::default(),
            allow_training_models: false,
            review_timeout_minutes: DEFAULT_REVIEW_TIMEOUT_MINUTES,
            workstation_recordings: RecordingDefaults::default(),
            board_pauses: BoardPauses::default(),
            engines_enabled: DEFAULT_ENGINE_IDS.map(String::from).to_vec(),
            engines_mode: ENGINE_MODE.into(),
            acp_enabled: false,
            acp_write_back: false,
            acp_v2: false,
            idle_sleep_min: DEFAULT_IDLE_SLEEP_MIN,
            sleep_never_roles: vec![ORCHESTRATOR_ROLE.into()],
            doze_idle_s: DEFAULT_DOZE_IDLE_S,
            role_volumes: BTreeMap::new(),
            health_interval_s: DEFAULT_HEALTH_INTERVAL_S,
            favourite_models: Vec::new(),
        }
    }
}

/// How the laws are applied (`laws.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LawsSettings {
    /// Whether an operation that has permit laws is refused unless one of them allows it. Off: only
    /// forbids refuse, and a permit only allows.
    pub strict_permits: bool,
}

/// How payments and model calls are gated and charged. Any `u64` is a valid threshold, so
/// there is nothing to refuse at save time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Payments {
    /// A payment over this many credits waits for the owner's approval. `0` means every payment asks.
    pub approve_over: u64,
    /// Whether model calls are charged to the agent's task escrow (or its wallet when it has no
    /// task), and refused when it cannot pay.
    pub charge_models: bool,
}

impl Default for Payments {
    fn default() -> Self {
        Self {
            approve_over: 500,
            charge_models: true,
        }
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

/// What the plaza keeps of its board: a count of posts (the oldest go first past
/// it) and an age in days (`0` keeps them whatever their age). The board is kept
/// until these say otherwise; the store never refuses a new post for room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retention {
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
    ModelRef::parse(model)
        .map(|r| r.to_string())
        .map_err(|e| e.to_string())
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
        let loaded: Settings = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(settings) => settings,
                Err(e) => {
                    eprintln!(
                        "settings: {} is not valid settings JSON ({e}); using the defaults",
                        path.display()
                    );
                    fresh(dev_models)
                }
            },
            Err(_) => fresh(dev_models),
        };
        // A hand-edited file gets the same checks a save gets; what fails is repaired, and named.
        let (settings, notes) = repair(loaded, dev_models);
        for note in &notes {
            eprintln!("settings: {} repaired on load: {note}", path.display());
        }
        crate::laws::global().set_strict(settings.laws.strict_permits);
        Ok(Self {
            path,
            dev_models,
            inner: Mutex::new(settings),
        })
    }

    pub fn get(&self) -> Settings {
        // contract
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn dev_models(&self) -> bool {
        // contract
        self.dev_models
    }

    /// Whether `model` may run for `tier` under the stored policy (always in dev mode); the error
    /// names the tier's list. See [`allowed_under`].
    pub fn allowed(&self, tier: Tier, model: &str) -> Result<(), String> {
        // contract
        allowed_under(&self.get().model_policy, self.dev_models, tier, model)
    }

    /// Validates `new`, writes `settings.json` atomically and replaces the
    /// in-memory settings. Returns the saved value.
    pub fn put(&self, mut new: Settings) -> Result<Settings, String> {
        self.check(&new)?;
        let mut guard = self.lock();
        new.board_pauses = guard.board_pauses.clone();
        self.write(&mut guard, new)
    }

    /// Pauses (`on`) or resumes the deliveries of one project's board, or of the whole board (`project`
    /// `None`), and saves it. Returns whether the pauses changed, and the pauses now in force.
    pub fn set_board_pause(
        &self,
        project: Option<&str>,
        on: bool,
    ) -> Result<(bool, BoardPauses), String> {
        let mut guard = self.lock();
        let mut next = guard.clone();
        let before = next.board_pauses.clone();
        match project {
            None => next.board_pauses.all = on,
            Some(project) if on => {
                next.board_pauses.projects.insert(project.to_string());
            }
            Some(project) => {
                next.board_pauses.projects.remove(project);
            }
        }
        let changed = next.board_pauses != before;
        let saved = self.write(&mut guard, next)?;
        Ok((changed, saved.board_pauses))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Settings> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The checks every saved settings value passes (`put` and `restore`).
    fn check(&self, new: &Settings) -> Result<(), String> {
        check_settings(new, self.dev_models)
    }

    /// Writes `settings.json` atomically and swaps the in-memory settings.
    fn replace(&self, new: Settings) -> Result<Settings, String> {
        let mut guard = self.lock();
        self.write(&mut guard, new)
    }

    /// [`Store::replace`] with the lock already held: `guard` is the in-memory settings.
    fn write(&self, guard: &mut Settings, new: Settings) -> Result<Settings, String> {
        let bytes = serde_json::to_vec_pretty(&new).map_err(|e| e.to_string())?;
        let tmp = self.path.with_file_name("settings.json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("write settings: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("replace settings: {e}"))?;
        *guard = new.clone();
        crate::laws::global().set_strict(new.laws.strict_permits);
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

/// The reasoning efforts a settings `effort` may name. `None` (unset) is the provider's own default.
pub(crate) const EFFORT_LEVELS: [&str; 7] =
    ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Whether `model` may run for `tier` under `policy` (always in dev mode). The error names the
/// tier's list.
///
/// The orchestrator tier may run only its list, and an empty list admits nothing. Workers may run
/// any well-formed model: their list is the presets offered first and the default, not a limit. An
/// entry without an effort pin admits that model at any effort; a pinned entry only itself.
fn allowed_under(
    policy: &ModelPolicy,
    dev_models: bool,
    tier: Tier,
    model: &str,
) -> Result<(), String> {
    if dev_models {
        return Ok(());
    }
    let wanted = canonical(model)?;
    if tier == Tier::Worker {
        return Ok(());
    }
    let list = &policy.orchestrator;
    let names: Vec<String> = list.iter().filter_map(|m| canonical(m).ok()).collect();
    let base = ModelRef::parse(model)
        .map(|r| r.base_name())
        .map_err(|e| e.to_string())?;
    if names.contains(&wanted) || names.contains(&base) {
        return Ok(());
    }
    let shown = if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    };
    Err(format!(
        "{model} is not allowed for the {} tier; allowed: {shown}",
        tier_name(tier)
    ))
}

/// The checks every settings value passes: on save and restore (`Store::put`, `Store::restore`),
/// and on load (`repair`'s last look). The model rules are those of [`allowed_under`] against the
/// policy this value carries.
/// The engine settings' rules: the enabled engines are known, at least one, and none twice; the mode is known.
fn check_engines(new: &Settings) -> Result<(), String> {
    if let Some(bad) = new
        .engines_enabled
        .iter()
        .find(|id| !ENGINE_IDS.contains(&id.as_str()))
    {
        return Err(format!(
            "unknown engine {bad:?}; expected one of {}",
            ENGINE_IDS.join(", ")
        ));
    }
    if new.engines_enabled.is_empty() {
        return Err("engines_enabled needs at least one engine".into());
    }
    if new.engines_enabled.iter().collect::<BTreeSet<_>>().len() != new.engines_enabled.len() {
        return Err("engines_enabled lists an engine twice".into());
    }
    if new.engines_mode != ENGINE_MODE {
        return Err(format!(
            "unknown engines_mode {:?}; expected {ENGINE_MODE}",
            new.engines_mode
        ));
    }
    Ok(())
}

/// Favourite models are `<provider>/<model>` names, each at most once, at most [`MAX_FAVOURITE_MODELS`].
fn check_favourites(favourites: &[String]) -> Result<(), String> {
    if favourites.len() > MAX_FAVOURITE_MODELS {
        return Err(format!(
            "favourite_models holds at most {MAX_FAVOURITE_MODELS} models"
        ));
    }
    let mut seen = BTreeSet::new();
    for model in favourites {
        let named = model
            .split_once('/')
            .is_some_and(|(provider, rest)| !provider.is_empty() && !rest.is_empty());
        if !named || model.len() > 200 || model.chars().any(char::is_whitespace) {
            return Err(format!(
                "favourite model {model:?} is not a <provider>/<model> name"
            ));
        }
        if !seen.insert(model.as_str()) {
            return Err(format!("favourite model {model} is listed twice"));
        }
    }
    Ok(())
}

fn check_settings(new: &Settings, dev_models: bool) -> Result<(), String> {
    if new.health_interval_s > MAX_HEALTH_INTERVAL_S {
        return Err(format!(
            "health_interval_s is at most {MAX_HEALTH_INTERVAL_S} (a day); 0 turns the checks off"
        ));
    }
    check_favourites(&new.favourite_models)?;
    if !new.allow_training_models {
        // A contributor model trains Meta's future models on what it is sent: never a default, a role
        // model or a policy entry until the owner says so (`allow_training_models`).
        let named = std::iter::once(&new.default_model)
            .chain(std::iter::once(&new.orchestrator_model))
            .chain(new.role_models.values())
            .chain(new.model_policy.orchestrator.iter())
            .chain(new.model_policy.worker.iter());
        if let Some(model) = named.into_iter().find(|m| apz_gateway::trains_on_data(m)) {
            return Err(format!(
                "{model} is a contributor model: it trains the provider's future models on what it is sent. \
                 It may be a default, a role model or a policy entry only with allow_training_models set"
            ));
        }
    }
    let policy = &new.model_policy;
    allowed_under(policy, dev_models, Tier::Worker, &new.default_model)?;
    allowed_under(
        policy,
        dev_models,
        Tier::Orchestrator,
        &new.orchestrator_model,
    )?;
    for (role, model) in &new.role_models {
        allowed_under(policy, dev_models, tier_of(role), model)?;
    }
    if !(1..=200).contains(&new.max_turns) {
        return Err(format!("max_turns must be 1..=200, got {}", new.max_turns));
    }
    check_engines(new)?;
    if new.idle_sleep_min > MAX_IDLE_SLEEP_MIN {
        return Err(format!(
            "idle_sleep_min must be 0..={MAX_IDLE_SLEEP_MIN}, got {}",
            new.idle_sleep_min
        ));
    }
    if let Some(role) = new.sleep_never_roles.iter().find(|r| r.trim().is_empty()) {
        return Err(format!(
            "sleep_never_roles entry {role:?} is not a role name"
        ));
    }
    if new.doze_idle_s > MAX_DOZE_IDLE_S {
        return Err(format!(
            "doze_idle_s must be 0..={MAX_DOZE_IDLE_S}, got {}",
            new.doze_idle_s
        ));
    }
    for (role, volumes) in &new.role_volumes {
        check_role_volumes(role, volumes)?;
    }
    if let Some(effort) = &new.effort
        && !EFFORT_LEVELS.contains(&effort.as_str())
    {
        return Err(format!(
            "unknown effort {effort:?}; expected one of {}",
            EFFORT_LEVELS.join(", ")
        ));
    }
    if !(1..=500).contains(&new.foundry.hf_max_gb) {
        return Err(format!(
            "foundry.hf_max_gb must be 1..=500, got {}",
            new.foundry.hf_max_gb
        ));
    }
    if let Some(bad) = new
        .fetch_block
        .iter()
        .find(|h| !is_bare_host(h) || h.trim_start_matches("*.").contains('*'))
    {
        return Err(format!(
            "fetch_block entry {bad:?} is not a bare hostname (or *.domain)"
        ));
    }
    let retention = &new.retention;
    for (name, max) in [
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
        ("posts_days", retention.posts_days),
        ("usage_days", retention.usage_days),
    ] {
        if days > RETENTION_DAYS_MAX {
            return Err(format!(
                "retention.{name} must be 0..={RETENTION_DAYS_MAX} (0 keeps everything), got {days}"
            ));
        }
    }
    Ok(())
}

/// Settings as a first start has them (dev defaults in dev mode).
fn fresh(dev_models: bool) -> Settings {
    let mut s = Settings::default();
    if dev_models {
        s.default_model = "mock/agent".into();
        s.orchestrator_model = "mock/agent".into();
    }
    s
}

/// The tier's default when its list admits it; otherwise the list's first entry (the owner's own
/// choice). Used for the orchestrator, the only tier with a list that restricts.
fn safe_model(policy: &ModelPolicy, dev_models: bool, tier: Tier, default: &str) -> String {
    if allowed_under(policy, dev_models, tier, default).is_ok() {
        return default.to_string();
    }
    let list = match tier {
        Tier::Orchestrator => &policy.orchestrator,
        Tier::Worker => &policy.worker,
    };
    list.iter()
        .find(|m| canonical(m).is_ok())
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

/// `value` when `ok`, else `default`, naming the reset in `notes`.
fn or_default<T: Copy + std::fmt::Display>(
    notes: &mut Vec<String>,
    name: &str,
    value: T,
    default: T,
    ok: bool,
) -> T {
    if ok {
        value
    } else {
        notes.push(format!("{name} {value} is not allowed; using {default}"));
        default
    }
}

/// Repairs a loaded settings value so that [`check_settings`] passes: an entry that fails is
/// dropped, or reset to its default, and each change is named in the returned notes. Nothing
/// fails here; the last resort is the defaults.
fn repair(mut s: Settings, dev_models: bool) -> (Settings, Vec<String>) {
    let d = Settings::default();
    let mut notes = Vec::new();

    if !s.allow_training_models {
        let trains = apz_gateway::trains_on_data;
        for (tier, list, default) in [
            (
                "orchestrator",
                &mut s.model_policy.orchestrator,
                &d.model_policy.orchestrator,
            ),
            ("worker", &mut s.model_policy.worker, &d.model_policy.worker),
        ] {
            let before = list.len();
            list.retain(|m| {
                let keep = !trains(m);
                if !keep {
                    notes.push(format!("dropped {m} from the {tier} policy: a contributor model needs allow_training_models"));
                }
                keep
            });
            if before > 0 && list.is_empty() {
                notes.push(format!("the {tier} policy held only contributor models; it is reset to the default list"));
                *list = default.clone();
            }
        }
        s.role_models.retain(|role, model| {
            let keep = !trains(model.as_str());
            if !keep {
                notes.push(format!("dropped the {role} role model {model}: a contributor model needs allow_training_models"));
            }
            keep
        });
        if trains(&s.default_model) {
            notes.push(format!(
                "default_model {} is a contributor model; using {}",
                s.default_model, d.default_model
            ));
            s.default_model = d.default_model.clone();
        }
        if trains(&s.orchestrator_model) {
            notes.push(format!(
                "orchestrator_model {} is a contributor model; using {}",
                s.orchestrator_model, d.orchestrator_model
            ));
            s.orchestrator_model = d.orchestrator_model.clone();
        }
    }

    // Only the orchestrator is held to its list: workers may run any model, so no worker default or
    // worker role model is dropped for being off a list.
    let policy = s.model_policy.clone();
    if allowed_under(
        &policy,
        dev_models,
        Tier::Orchestrator,
        &s.orchestrator_model,
    )
    .is_err()
    {
        let fallback = safe_model(
            &policy,
            dev_models,
            Tier::Orchestrator,
            &d.orchestrator_model,
        );
        notes.push(format!(
            "orchestrator_model {} is not on the orchestrator list; using {fallback}",
            s.orchestrator_model
        ));
        s.orchestrator_model = fallback;
    }
    s.role_models.retain(|role, model| {
        let keep = allowed_under(&policy, dev_models, tier_of(role), model.as_str()).is_ok();
        if !keep {
            notes.push(format!("role model {model} for {role} is not on its tier's list; the role uses the tier default"));
        }
        keep
    });

    s.max_turns = or_default(
        &mut notes,
        "max_turns",
        s.max_turns,
        d.max_turns,
        (1..=200).contains(&s.max_turns),
    );
    // An unknown or repeated engine is dropped; a list left with no engine is the defaults.
    let mut kept: Vec<String> = Vec::new();
    for id in std::mem::take(&mut s.engines_enabled) {
        if ENGINE_IDS.contains(&id.as_str()) && !kept.contains(&id) {
            kept.push(id);
        } else {
            notes.push(format!(
                "dropped engines_enabled entry {id:?}: not an engine, or listed twice"
            ));
        }
    }
    if kept.is_empty() {
        notes.push(format!(
            "engines_enabled names no engine; using {}",
            d.engines_enabled.join(", ")
        ));
        kept = d.engines_enabled.clone();
    }
    s.engines_enabled = kept;
    if s.engines_mode != ENGINE_MODE {
        notes.push(format!(
            "engines_mode {:?} is not a mode; using {}",
            s.engines_mode, d.engines_mode
        ));
        s.engines_mode = d.engines_mode.clone();
    }
    s.idle_sleep_min = or_default(
        &mut notes,
        "idle_sleep_min",
        s.idle_sleep_min,
        d.idle_sleep_min,
        s.idle_sleep_min <= MAX_IDLE_SLEEP_MIN,
    );
    s.doze_idle_s = or_default(
        &mut notes,
        "doze_idle_s",
        s.doze_idle_s,
        d.doze_idle_s,
        s.doze_idle_s <= MAX_DOZE_IDLE_S,
    );
    s.sleep_never_roles.retain(|role| {
        let ok = !role.trim().is_empty();
        if !ok {
            notes.push("dropped an empty sleep_never_roles entry".to_string());
        }
        ok
    });
    s.role_volumes
        .retain(|role, volumes| match check_role_volumes(role, volumes) {
            Ok(()) => true,
            Err(why) => {
                notes.push(format!("dropped the volumes of role {role:?}: {why}"));
                false
            }
        });
    s.effort = match s.effort.take() {
        Some(level) if !EFFORT_LEVELS.contains(&level.as_str()) => {
            notes.push(format!(
                "effort {level:?} is not a known level; it is unset"
            ));
            None
        }
        kept => kept,
    };
    s.foundry.hf_max_gb = or_default(
        &mut notes,
        "foundry.hf_max_gb",
        s.foundry.hf_max_gb,
        d.foundry.hf_max_gb,
        (1..=500).contains(&s.foundry.hf_max_gb),
    );
    s.fetch_block.retain(|host| {
        let ok = is_bare_host(host) && !host.trim_start_matches("*.").contains('*');
        if !ok {
            notes.push(format!(
                "dropped fetch_block entry {host:?}: not a bare hostname (or *.domain)"
            ));
        }
        ok
    });
    let (r, rd) = (s.retention, Retention::default());
    let count_ok = |v: u32| (RETENTION_MIN..=RETENTION_MAX).contains(&v);
    let days_ok = |v: u32| v <= RETENTION_DAYS_MAX;
    s.retention = Retention {
        posts_max: or_default(
            &mut notes,
            "retention.posts_max",
            r.posts_max,
            rd.posts_max,
            count_ok(r.posts_max),
        ),
        posts_days: or_default(
            &mut notes,
            "retention.posts_days",
            r.posts_days,
            rd.posts_days,
            days_ok(r.posts_days),
        ),
        usage_calls_max: or_default(
            &mut notes,
            "retention.usage_calls_max",
            r.usage_calls_max,
            rd.usage_calls_max,
            count_ok(r.usage_calls_max),
        ),
        usage_days: or_default(
            &mut notes,
            "retention.usage_days",
            r.usage_days,
            rd.usage_days,
            days_ok(r.usage_days),
        ),
    };

    if let Err(e) = check_settings(&s, dev_models) {
        notes.push(format!(
            "the settings still fail the checks ({e}); the defaults are used"
        ));
        s = fresh(dev_models);
    }
    (s, notes)
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
        .doc(RouteDoc::new("Get the hub's settings").errors(&[500]))
        .route("PUT", "/v1/settings", |app, req, _| {
            let new: Settings = match req.json() {
                Ok(settings) => settings,
                Err(e) => {
                    return Response::json(
                        400,
                        &json!({ "error": format!("bad settings JSON: {e}") }),
                    );
                }
            };
            match app.settings.put(new) {
                Ok(saved) => Response::json(200, &saved),
                Err(msg) => Response::json(400, &json!({ "error": msg })),
            }
        })
        .doc(RouteDoc::new("Save the hub's settings"))
}

impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(&self.get()).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let new: Settings =
            serde_json::from_str(json).map_err(|e| format!("bad settings snapshot: {e}"))?;
        self.check(&new)?;
        self.replace(new).map(|_| ())
    }
}

/// The most shared data volumes one agent mounts (noded's limit, `vmm::spawner::MAX_DATA_VOLUMES`).
pub const MAX_ROLE_VOLUMES: usize = 4;

/// Checks one `role_volumes` entry: a role name, and at most [`MAX_ROLE_VOLUMES`] distinct volume
/// names of 1 to 20 characters of `a-z`, `0-9` and `-` (not starting with `-`), as noded names them.
pub fn check_role_volumes(role: &str, volumes: &[String]) -> Result<(), String> {
    if role.trim().is_empty() {
        return Err(format!("role_volumes key {role:?} is not a role name"));
    }
    if volumes.len() > MAX_ROLE_VOLUMES {
        return Err(format!(
            "role {role}: at most {MAX_ROLE_VOLUMES} volumes, not {}",
            volumes.len()
        ));
    }
    for (index, name) in volumes.iter().enumerate() {
        let bytes = name.as_bytes();
        let valid = (1..=20).contains(&bytes.len())
            && bytes[0] != b'-'
            && bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
        if !valid {
            return Err(format!("role {role}: {name:?} is not a volume name"));
        }
        if volumes[..index].contains(name) {
            return Err(format!("role {role}: volume {name:?} is given twice"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favourites_and_the_health_interval_are_checked() {
        let mut s = Settings::default();
        assert_eq!(s.health_interval_s, DEFAULT_HEALTH_INTERVAL_S);
        s.favourite_models = vec!["anthropic/claude-sonnet-5-5".into()];
        assert!(check_settings(&s, false).is_ok());
        s.favourite_models
            .push("anthropic/claude-sonnet-5-5".into());
        assert!(check_settings(&s, false).unwrap_err().contains("twice"));
        s.favourite_models = vec!["no-slash".into()];
        assert!(check_settings(&s, false).is_err());
        s.favourite_models.clear();
        s.health_interval_s = MAX_HEALTH_INTERVAL_S + 1;
        assert!(check_settings(&s, false).is_err());
        s.health_interval_s = 0;
        assert!(check_settings(&s, false).is_ok());
    }

    #[test]
    fn role_volumes_are_checked_and_bad_entries_dropped_on_load() {
        let ok = vec!["datasets".to_string(), "weights-7b".to_string()];
        check_role_volumes("builder", &ok).expect("two good names");
        assert!(check_role_volumes(" ", &ok).is_err(), "no role");
        assert!(
            check_role_volumes("builder", &["Data".into()]).is_err(),
            "uppercase"
        );
        assert!(
            check_role_volumes("builder", &["-x".into()]).is_err(),
            "leading dash"
        );
        assert!(
            check_role_volumes("builder", &["a".repeat(21)]).is_err(),
            "too long"
        );
        assert!(
            check_role_volumes("builder", &["a".into(), "a".into()]).is_err(),
            "twice"
        );
        let five: Vec<String> = (0..5).map(|i| format!("v{i}")).collect();
        assert!(
            check_role_volumes("builder", &five).is_err(),
            "more than four"
        );
        let mut settings = Settings::default();
        settings.role_volumes.insert("builder".into(), ok.clone());
        assert!(check_settings(&settings, false).is_ok());
        settings
            .role_volumes
            .insert("reviewer".into(), vec!["Bad".into()]);
        assert!(check_settings(&settings, false).is_err());
    }

    /// A fresh, empty state directory under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn allowed_follows_the_tier_rules() {
        // allowed_is_per_tier_and_canonical: each tier admits its own entries, names are canonical, and an unparseable name fails with the parser text.
        {
            let dir = temp_dir("allowed");
            let store = Store::load(&dir, false).unwrap();
            // Worker tier: the effort-suffixed entry passes, the plain one does not.
            assert!(
                store
                    .allowed(Tier::Worker, "anthropic/claude-opus-5-5@low")
                    .is_ok()
            );
            assert!(
                store
                    .allowed(Tier::Worker, "deepseek/deepseek-flash")
                    .is_ok()
            );
            assert!(
                store
                    .allowed(Tier::Worker, "  anthropic/claude-haiku-5-5 ")
                    .is_ok()
            );
            // Workers may run any model; their list only holds the presets.
            assert!(
                store
                    .allowed(Tier::Worker, "anthropic/claude-opus-5-5")
                    .is_ok()
            );
            assert!(
                store
                    .allowed(Tier::Worker, "openai/gpt-4-turbo-2024-04-09")
                    .is_ok()
            );
            let err = store
                .allowed(Tier::Orchestrator, "deepseek/deepseek-flash")
                .unwrap_err();
            assert!(
                err.contains("not allowed for the orchestrator tier"),
                "{err}"
            );
            assert!(err.contains("allowed: "), "{err}");
            // Orchestrator tier: only its own list, at any effort when the entry pins none.
            assert!(
                store
                    .allowed(Tier::Orchestrator, "anthropic/claude-opus-5-5")
                    .is_ok()
            );
            assert!(
                store
                    .allowed(Tier::Orchestrator, "openai/gpt-6-astra@high")
                    .is_ok()
            );
            assert!(
                store
                    .allowed(Tier::Orchestrator, "anthropic/claude-opus-5-5@max")
                    .is_ok()
            );
            assert!(
                store
                    .allowed(Tier::Orchestrator, "deepseek/deepseek-flash@high")
                    .is_err()
            );
            assert!(
                store
                    .allowed(Tier::Orchestrator, "deepseek/deepseek-flash")
                    .is_err()
            );
            // Unparseable names fail with the parser's text.
            assert!(store.allowed(Tier::Worker, "no-slash-here").is_err());
            let _ = std::fs::remove_dir_all(&dir);
        }

        // the_orchestrator_is_restricted_and_workers_may_run_any_model: the orchestrator is held to its list, and workers may run any model.
        {
            let dir = temp_dir("tier-rules");
            let store = Store::load(&dir, false).unwrap();
            // The orchestrator is held to its list, and the error names it.
            assert!(
                store
                    .allowed(Tier::Orchestrator, "openai/gpt-6-astra@high")
                    .is_ok()
            );
            let err = store
                .allowed(Tier::Orchestrator, "deepseek/deepseek-flash")
                .unwrap_err();
            assert!(
                err.contains("not allowed for the orchestrator tier")
                    && err.contains("allowed: anthropic/claude-opus-5-5"),
                "{err}"
            );
            // An orchestrator role model off its list is refused.
            let mut orch_role = Settings::default();
            orch_role
                .role_models
                .insert("orchestrator".into(), "deepseek/deepseek-flash".into());
            assert!(store.put(orch_role).is_err());

            // Workers are not restricted: a worker's model or role model may be anything, on or off the list.
            assert!(store.allowed(Tier::Worker, "openai/gpt-6-astra").is_ok());
            assert!(store.allowed(Tier::Worker, "anything/at-all").is_ok());
            let mut worker_role = Settings::default();
            worker_role
                .role_models
                .insert("scout".into(), "anthropic/claude-fable-5-1".into());
            assert!(store.put(worker_role).is_ok());
            let worker_default = Settings {
                default_model: "openai/gpt-6-astra".into(),
                ..Settings::default()
            };
            assert!(store.put(worker_default).is_ok());

            // An empty orchestrator list admits nothing and says so; an empty worker list changes nothing.
            let mut closed = Settings::default();
            closed.model_policy.orchestrator.clear();
            let err = store.put(closed).unwrap_err();
            assert!(err.contains("allowed: none"), "{err}");
            let mut open = Settings::default();
            open.model_policy.worker.clear();
            assert!(store.put(open).is_ok());
            let _ = std::fs::remove_dir_all(&dir);
        }
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
        let s = Settings {
            max_turns: 50,
            ..Settings::default()
        };
        store.put(s.clone()).unwrap();
        let json = store.snapshot();

        let other_dir = temp_dir("snapshot-other");
        let other = Store::load(&other_dir, false).unwrap();
        other.restore(&json).unwrap();
        assert_eq!(other.get(), s);
        assert_eq!(Store::load(&other_dir, false).unwrap().get(), s);

        let bad = Settings {
            max_turns: 0,
            ..Settings::default()
        };
        assert!(
            other
                .restore(&serde_json::to_string(&bad).unwrap())
                .is_err()
        );
        assert_eq!(other.get(), s);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other_dir);
    }

    #[test]
    fn put_rejects_bad_fetch_block_entries() {
        let dir = temp_dir("fetch");
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load(&dir, false).unwrap();
        for bad in [
            "https://example.com",
            "example.com/path",
            "has space.com",
            "host:8080",
            "",
            "a.*.com",
        ] {
            let s = Settings {
                fetch_block: vec![bad.to_string()],
                ..Settings::default()
            };
            assert!(store.put(s).is_err(), "{bad:?} should be rejected");
        }
        let ok = Settings {
            fetch_block: vec!["example.com".into(), "*.example.net".into()],
            ..Settings::default()
        };
        assert!(store.put(ok).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_validates_and_persists() {
        let dir = temp_dir("persist");
        let store = Store::load(&dir, false).unwrap();
        // A worker default may be any model; an orchestrator model outside its list is refused.
        let any = Settings {
            default_model: "anthropic/claude-fable-5-1".into(),
            ..Settings::default()
        };
        assert!(store.put(any).is_ok());
        let bad = Settings {
            orchestrator_model: "deepseek/deepseek-flash".into(),
            ..Settings::default()
        };
        assert!(store.put(bad).is_err());
        let zero = Settings {
            max_turns: 0,
            ..Settings::default()
        };
        assert!(store.put(zero).is_err());

        let s = Settings {
            max_turns: 50,
            effort: Some("low".into()),
            ..Settings::default()
        };
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
            Retention {
                posts_max: 0,
                ..Retention::default()
            },
            Retention {
                posts_days: 4000,
                ..Retention::default()
            },
        ] {
            let s = Settings {
                retention: bad,
                ..Settings::default()
            };
            assert!(store.put(s).is_err(), "{bad:?} should be refused");
        }
        let ok = Settings {
            retention: Retention {
                posts_max: RETENTION_MAX,
                posts_days: RETENTION_DAYS_MAX,
                usage_calls_max: RETENTION_MIN,
                usage_days: 0,
            },
            ..Settings::default()
        };
        assert_eq!(store.put(ok.clone()).unwrap(), ok);

        // Settings saved before retention existed carry no such key: they load the defaults.
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut().unwrap().remove("retention");
        let loaded: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(loaded.retention, Retention::default());

        // The default keeps posts up to their count, whatever their age.
        let policy = Retention::default().policy();
        assert_eq!(policy.posts_max, 1024);
        assert_eq!(policy.posts_older_than_ms, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_contributor_model_is_refused_as_a_default_or_role_model_until_confirmed() {
        let dir = temp_dir("training");
        let store = Store::load(&dir, false).unwrap();
        assert!(!Settings::default().allow_training_models);
        // No default names a contributor model.
        assert!(!Settings::default().default_model.contains("contributor"));

        let mut as_default = Settings {
            default_model: "meta/muse-spark-1.3-contributor".into(),
            ..Settings::default()
        };
        let err = store.put(as_default.clone()).unwrap_err();
        assert!(
            err.contains("contributor model") && err.contains("allow_training_models"),
            "{err}"
        );

        let mut as_role = Settings::default();
        as_role
            .role_models
            .insert("scout".into(), "meta/muse-spark-1.2-contributor@low".into());
        assert!(store.put(as_role.clone()).is_err());

        let mut in_policy = Settings::default();
        in_policy
            .model_policy
            .worker
            .push("meta/muse-spark-1.3-contributor".into());
        assert!(store.put(in_policy.clone()).is_err());

        // The owner's explicit confirmation lets the save through.
        as_default.allow_training_models = true;
        assert!(store.put(as_default.clone()).is_ok());
        assert_eq!(
            Store::load(&dir, false).unwrap().get().default_model,
            "meta/muse-spark-1.3-contributor"
        );

        // Meta's own models are never contributor models: no confirmation needed.
        let plain = Settings {
            default_model: "meta/muse-spark-1.3".into(),
            ..Settings::default()
        };
        assert!(store.put(plain).is_ok());
        // A per-agent pick is an explicit choice of one agent: the tier rules still allow it.
        assert!(
            store
                .allowed(Tier::Worker, "meta/muse-spark-1.3-contributor@low")
                .is_ok()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hand_edited_file_is_repaired_on_load_and_named() {
        let dir = temp_dir("repair");
        let mut edited = serde_json::to_value(Settings::default()).unwrap();
        {
            let obj = edited.as_object_mut().unwrap();
            obj.insert(
                "default_model".into(),
                json!("meta/muse-spark-1.3-contributor"),
            );
            obj.insert(
                "orchestrator_model".into(),
                json!("deepseek/deepseek-flash"),
            );
            obj.insert(
                "role_models".into(),
                json!({"scout": "anthropic/claude-fable-5-1", "writer": "openai/gpt-6-luna", "orchestrator": "deepseek/deepseek-flash"}),
            );
            obj.insert("max_turns".into(), json!(0));
            obj.insert("effort".into(), json!("bogus"));
            obj.insert(
                "fetch_block".into(),
                json!(["https://x.example", "ok.example"]),
            );
            obj.insert(
                "retention".into(),
                json!({"posts_max": 1024, "usage_calls_max": 20000}),
            );
            obj.insert(
                "sanctions".into(),
                json!({"enabled": true, "fines": true, "suspensions": true, "max_fine": 5000}),
            );
            obj["model_policy"]["worker"]
                .as_array_mut()
                .unwrap()
                .push(json!("meta/muse-spark-1.3-contributor"));
        }
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&edited).unwrap(),
        )
        .unwrap();

        let store = Store::load(&dir, false).unwrap();
        let s = store.get();
        assert_eq!(
            s.default_model, "deepseek/deepseek-flash",
            "a contributor default falls back to the default"
        );
        // Workers are not held to a list: their role models stay, on or off the presets.
        assert_eq!(
            s.role_models.get("scout").map(String::as_str),
            Some("anthropic/claude-fable-5-1")
        );
        assert_eq!(
            s.role_models.get("writer").map(String::as_str),
            Some("openai/gpt-6-luna")
        );
        // The orchestrator is held to its list: an orchestrator role model off it is dropped, and the
        // orchestrator model off it falls back to the default, which that list admits.
        assert!(
            !s.role_models.contains_key("orchestrator"),
            "an orchestrator role model off its list is dropped"
        );
        assert_eq!(s.orchestrator_model, "anthropic/claude-opus-5-5");
        assert!(
            !s.model_policy
                .worker
                .iter()
                .any(|m| m.contains("contributor")),
            "the contributor entry is dropped"
        );
        assert_eq!(s.max_turns, 24);
        assert_eq!(s.effort, None);
        assert_eq!(s.fetch_block, vec!["ok.example".to_string()]);
        assert!(
            check_settings(&s, false).is_ok(),
            "what loads passes the save checks"
        );

        // The notes name what changed.
        let mut edited_again =
            serde_json::from_slice::<Settings>(&std::fs::read(dir.join("settings.json")).unwrap())
                .unwrap();
        edited_again.allow_training_models = false;
        edited_again
            .model_policy
            .worker
            .push("meta/muse-spark-1.2-contributor".into());
        let (_, notes) = repair(edited_again, false);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("meta/muse-spark-1.2-contributor")),
            "{notes:?}"
        );

        // A file that is not settings JSON loads the defaults rather than failing.
        std::fs::write(dir.join("settings.json"), b"{ not json").unwrap();
        assert_eq!(Store::load(&dir, false).unwrap().get(), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_settings_load_their_defaults() {
        // review_timeout_defaults_to_thirty_minutes_and_old_settings_load_it: the review timeout defaults to 30 minutes, and old settings load it.
        {
            assert_eq!(Settings::default().review_timeout_minutes, 30);
            assert_eq!(DEFAULT_REVIEW_TIMEOUT_MINUTES, 30);

            // Settings saved before the review timeout existed carry no such key: they load the default.
            let mut old = serde_json::to_value(Settings::default()).unwrap();
            old.as_object_mut()
                .unwrap()
                .remove("review_timeout_minutes");
            let loaded: Settings = serde_json::from_value(old).unwrap();
            assert_eq!(
                loaded.review_timeout_minutes,
                DEFAULT_REVIEW_TIMEOUT_MINUTES
            );

            // A saved value is kept as it is.
            let set: Settings =
                serde_json::from_value(serde_json::json!({"review_timeout_minutes": 5})).unwrap();
            assert_eq!(set.review_timeout_minutes, 5);
        }

        // acp_v2_is_off_by_default_and_old_settings_keep_v1: ACP v2 is off by default, and old settings keep v1.
        {
            assert!(!Settings::default().acp_v2);
            // Settings saved before the draft existed carry no key: editors keep speaking v1.
            let old: Settings = serde_json::from_str("{\"max_turns\": 24}").unwrap();
            assert!(!old.acp_v2);
            let on: Settings = serde_json::from_str("{\"acp_v2\": true}").unwrap();
            assert!(on.acp_v2);
            assert!(
                !on.acp_write_back,
                "the other ACP switches keep their defaults"
            );
        }

        // payments_default_and_settings_saved_without_them_load_the_defaults: payments default, and settings saved without them load the defaults.
        {
            assert_eq!(
                Payments::default(),
                Payments {
                    approve_over: 500,
                    charge_models: true
                }
            );
            assert_eq!(Settings::default().payments, Payments::default());

            // Settings saved before payments existed carry no such key: they load the defaults.
            let mut old = serde_json::to_value(Settings::default()).unwrap();
            old.as_object_mut().unwrap().remove("payments");
            let loaded: Settings = serde_json::from_value(old).unwrap();
            assert_eq!(loaded.payments, Payments::default());

            // A partial block keeps the fields it names and defaults the rest.
            let partial: Settings =
                serde_json::from_value(serde_json::json!({"payments": {"approve_over": 0}}))
                    .unwrap();
            assert_eq!(
                partial.payments,
                Payments {
                    approve_over: 0,
                    charge_models: true
                }
            );
        }
    }

    #[test]
    fn engines_default_to_all_three_over_acp_and_refusals_are_named() {
        let mut s = Settings::default();
        assert_eq!(s.engines_enabled, ["claude", "codex", "opencode"]);
        assert_eq!(s.engines_mode, "acp");
        assert!(check_engines(&s).is_ok());

        s.engines_enabled = vec!["gemini".into()];
        assert!(check_engines(&s).is_ok(), "Gemini CLI may be enabled");
        s.engines_enabled = vec!["qwen".into(), "kilo".into(), "cline".into()];
        assert!(
            check_engines(&s).is_ok(),
            "Qwen Code, Kilo Code and Cline may be enabled"
        );
        s.engines_enabled = vec!["bash".into()];
        assert!(check_engines(&s).unwrap_err().contains("bash"));
        s.engines_enabled = Vec::new();
        assert!(check_engines(&s).unwrap_err().contains("at least one"));
        s.engines_enabled = vec!["claude".into(), "claude".into()];
        assert!(check_engines(&s).unwrap_err().contains("twice"));

        s.engines_enabled = vec!["codex".into()];
        s.engines_mode = "pipes".into();
        assert!(check_engines(&s).unwrap_err().contains("pipes"));
        s.engines_mode = "headless".into();
        assert!(check_engines(&s).unwrap_err().contains("headless"));
        s.engines_mode = ENGINE_MODE.into();
        assert!(check_engines(&s).is_ok());
    }

    #[test]
    fn settings_repair_on_load() {
        // engines_repair_on_load_and_old_settings_load_the_defaults: old settings load the engine defaults, and a hand-edited engine list is repaired on load.
        {
            // Settings saved before the engines existed carry neither key: they load the defaults.
            let old: Settings = serde_json::from_str("{\"max_turns\": 24}").unwrap();
            assert_eq!(old.engines_enabled, DEFAULT_ENGINE_IDS);
            assert_eq!(old.engines_mode, "acp");

            // A hand-edited file: an unknown or repeated id is dropped, a bad mode is reset.
            let s = Settings {
                engines_enabled: vec!["claude".into(), "bash".into(), "claude".into()],
                engines_mode: "pipes".into(),
                ..Settings::default()
            };
            let (fixed, notes) = repair(s, false);
            assert_eq!(fixed.engines_enabled, ["claude"]);
            assert_eq!(fixed.engines_mode, "acp");
            assert!(notes.iter().any(|n| n.contains("bash")), "{notes:?}");
            assert!(notes.iter().any(|n| n.contains("pipes")), "{notes:?}");

            // The headless run is gone: a file saved under it loads as ACP, with a note.
            let saved: Settings = serde_json::from_str("{\"engines_mode\": \"headless\"}").unwrap();
            let (fixed, notes) = repair(saved, false);
            assert_eq!(fixed.engines_mode, "acp");
            assert!(notes.iter().any(|n| n.contains("headless")), "{notes:?}");

            // A list with no engine left is the defaults.
            let none = Settings {
                engines_enabled: vec!["bash".into()],
                ..Settings::default()
            };
            let (fixed, _) = repair(none, false);
            assert_eq!(fixed.engines_enabled, DEFAULT_ENGINE_IDS);
        }

        // idle_sleep_settings_saved_without_them_load_the_defaults_and_bad_ones_are_repaired: idle-sleep settings saved without them load the defaults, and bad ones are repaired.
        {
            let mut old = serde_json::to_value(Settings::default()).unwrap();
            let obj = old.as_object_mut().unwrap();
            obj.remove("idle_sleep_min");
            obj.remove("sleep_never_roles");
            obj.remove("doze_idle_s");
            let loaded: Settings = serde_json::from_value(old).unwrap();
            assert_eq!(loaded.idle_sleep_min, DEFAULT_IDLE_SLEEP_MIN);
            assert_eq!(loaded.sleep_never_roles, vec!["orchestrator".to_string()]);
            assert_eq!(loaded.doze_idle_s, DEFAULT_DOZE_IDLE_S);

            let bad = Settings {
                idle_sleep_min: MAX_IDLE_SLEEP_MIN + 5,
                sleep_never_roles: vec!["".into(), "builder".into()],
                doze_idle_s: MAX_DOZE_IDLE_S + 1,
                ..Settings::default()
            };
            let (fixed, notes) = repair(bad, false);
            assert_eq!(fixed.idle_sleep_min, DEFAULT_IDLE_SLEEP_MIN);
            assert_eq!(fixed.sleep_never_roles, vec!["builder".to_string()]);
            assert_eq!(fixed.doze_idle_s, DEFAULT_DOZE_IDLE_S);
            assert!(
                notes.iter().any(|n| n.contains("idle_sleep_min")),
                "{notes:?}"
            );
            assert!(notes.iter().any(|n| n.contains("doze_idle_s")), "{notes:?}");
            assert!(check_settings(&fixed, false).is_ok());
        }
    }

    #[test]
    fn idle_sleep_defaults_and_validation() {
        let d = Settings::default();
        assert_eq!(d.idle_sleep_min, 10);
        assert_eq!(d.sleep_never_roles, vec!["orchestrator".to_string()]);
        assert_eq!(d.doze_idle_s, 0);

        let dir = temp_dir("idle-sleep");
        let store = Store::load(&dir, false).unwrap();
        let too_long = Settings {
            idle_sleep_min: MAX_IDLE_SLEEP_MIN + 1,
            ..Settings::default()
        };
        assert!(store.put(too_long).is_err(), "above a day is refused");
        let dozes_too_long = Settings {
            doze_idle_s: MAX_DOZE_IDLE_S + 1,
            ..Settings::default()
        };
        assert!(store.put(dozes_too_long).is_err(), "above a day is refused");
        let dozing_never = Settings {
            doze_idle_s: 0,
            ..Settings::default()
        };
        let saved_dozing = store.put(dozing_never.clone()).unwrap();
        assert_eq!(saved_dozing.doze_idle_s, 0, "0 (agents never doze) is kept");
        let blank_role = Settings {
            sleep_never_roles: vec!["  ".into()],
            ..Settings::default()
        };
        assert!(store.put(blank_role).is_err(), "a blank role is refused");
        let never = Settings {
            idle_sleep_min: 0,
            sleep_never_roles: vec![],
            ..Settings::default()
        };
        let saved = store.put(never.clone()).unwrap();
        assert_eq!(
            saved, never,
            "0 (agents never sleep) and no never-roles are kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn payments_round_trip_through_the_store() {
        let dir = temp_dir("payments");
        let store = Store::load(&dir, false).unwrap();
        let s = Settings {
            payments: Payments {
                approve_over: 0,
                charge_models: false,
            },
            ..Settings::default()
        };
        assert_eq!(store.put(s.clone()).unwrap(), s);
        assert_eq!(Store::load(&dir, false).unwrap().get().payments, s.payments);

        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
