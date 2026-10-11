//! The roster (agents by CID), the roles they can have, and their profiles.
//!
//! OWNER: slice "agents". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use apz_http::Router;
use apz_proto::chat::Usage;
use apz_proto::wire::AgentProfile;
use serde::Serialize;

use crate::state::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Offline,
    Idle,
    Working,
}

/// One agent. `cid` is its identity.
#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// The operator's per-agent model, if any (else role/tier default).
    pub model_override: Option<String>,
    pub status: Status,
    pub connected_at_ms: u64,
    pub last_seen_ms: u64,
    pub run: Option<String>,
    pub vm: Option<u32>,
    pub usage: Usage,
    pub credits: f64,
}

#[derive(Default)]
pub struct Roster {
    agents: Mutex<BTreeMap<u64, Agent>>,
}

impl Roster {
    pub fn get(&self, cid: u64) -> Option<Agent> {
        // contract
        self.lock().get(&cid).cloned()
    }

    pub fn list(&self) -> Vec<Agent> {
        // contract
        self.lock().values().cloned().collect()
    }

    /// Applies `f` to the agent, if it exists; returns whether it did.
    pub fn update(&self, cid: u64, f: impl FnOnce(&mut Agent)) -> bool {
        // contract
        match self.lock().get_mut(&cid) {
            Some(agent) => {
                f(agent);
                true
            }
            None => false,
        }
    }

    /// Inserts or replaces an agent.
    pub fn upsert(&self, agent: Agent) {
        // contract
        self.lock().insert(agent.cid, agent);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Agent>> {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The roles manifest and the role prompts.
pub struct Roles {
    /// role → granted tool names (manifest names, e.g. `bash`, `submit_claim`).
    pub tools: BTreeMap<String, Vec<String>>,
    /// role → prompt text (`<prompts>/<role>/prompt.md`, empty if absent).
    pub prompts: BTreeMap<String, String>,
}

impl Roles {
    pub fn load(manifest: &Path, prompts: &Path) -> std::io::Result<Self> {
        // contract — TODO(agents slice)
        let _ = (manifest, prompts);
        Ok(Self { tools: BTreeMap::new(), prompts: BTreeMap::new() })
    }
}

/// The plaza's view of an agent, for apz-plaza calls.
pub fn connected_agent(agent: &Agent) -> apz_plaza::ConnectedAgent {
    // contract
    apz_plaza::ConnectedAgent {
        cid: agent.cid,
        role: Some(agent.role.clone()),
        run: agent.run.clone().unwrap_or_default(),
        vm: agent.vm.unwrap_or(0),
    }
}

/// The agent's resolved model.
pub fn model_of(app: &App, agent: &Agent) -> String {
    // contract
    app.settings.resolve(&agent.role, agent.model_override.as_deref())
}

/// The agent's profile: name, role, model, system prompt, tools, limits.
pub fn profile(app: &App, agent: &Agent) -> AgentProfile {
    // contract — TODO(agents slice): system prompt = preamble + role prompt
    AgentProfile {
        name: agent.name.clone(),
        role: agent.role.clone(),
        model: model_of(app, agent),
        system: String::new(),
        tools: crate::tools::specs_for(app, &agent.role),
        max_turns: app.settings.get().max_turns,
    }
}

/// A session said hello: register the agent (idle) and return its profile.
pub fn hello(app: &App, cid: u64, role: &str, vm: u32) -> Option<AgentProfile> {
    // contract — TODO(agents slice)
    let _ = (app, cid, role, vm);
    None
}

/// A session ended: mark the agent offline and fail its work.
pub fn gone(app: &App, cid: u64, reason: &str) {
    // contract — TODO(agents slice)
    let _ = (app, cid, reason);
}

/// Every 10 s: reconcile the roster with the hub's sessions and noded's runs.
pub fn spawn_reconcile(app: Arc<App>) {
    // contract — TODO(agents slice)
    let _ = app;
}

/// `/v1/agents…` (roster, spawn, detail, patch, delete, chat, assign, cancel,
/// stop, tell, transcript).
pub fn routes(router: Router<App>) -> Router<App> {
    // TODO(agents slice)
    router
}
