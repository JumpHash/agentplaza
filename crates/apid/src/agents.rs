//! The roster (agents by CID), the roles they can have, and their profiles.
//!
//! OWNER: slice "agents". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use apz_http::Router;
use apz_proto::chat::Usage;
use apz_proto::wire::AgentProfile;
use serde::Serialize;
use serde_json::Value;

use crate::settings::{tier_of, Tier};
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
    /// Reads the manifest's roles and each role's `<prompts>/<role>/prompt.md`.
    /// A missing manifest yields empty roles (with a warning); a malformed one is an error.
    pub fn load(manifest: &Path, prompts: &Path) -> std::io::Result<Self> {
        // contract
        let mut roles = Self { tools: BTreeMap::new(), prompts: BTreeMap::new() };
        let text = match std::fs::read_to_string(manifest) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("apid: roles manifest {} not read ({err}); no roles loaded", manifest.display());
                return Ok(roles);
            }
        };
        let doc: Value = serde_json::from_str(&text)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let Some(entries) = doc["roles"].as_object() else {
            return Ok(roles);
        };
        for (role, entry) in entries {
            let tools = entry["tools"]
                .as_array()
                .map(|list| list.iter().filter_map(|tool| tool.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            roles.tools.insert(role.clone(), tools);
            let prompt = std::fs::read_to_string(prompts.join(role).join("prompt.md")).unwrap_or_default();
            roles.prompts.insert(role.clone(), strip_comment(&prompt));
        }
        Ok(roles)
    }
}

/// Drops a leading one-line `<!-- … -->` comment and the blank lines after it.
fn strip_comment(prompt: &str) -> String {
    let trimmed = prompt.trim_start();
    if trimmed.starts_with("<!--") {
        if let Some(end) = trimmed.find("-->") {
            return trimmed[end + 3..].trim_start().to_owned();
        }
    }
    prompt.to_owned()
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
    // contract
    let role_prompt = app.roles.prompts.get(&agent.role).map_or("", String::as_str);
    AgentProfile {
        name: agent.name.clone(),
        role: agent.role.clone(),
        model: model_of(app, agent),
        system: system_prompt(agent, role_prompt),
        tools: crate::tools::specs_for(app, &agent.role),
        max_turns: app.settings.get().max_turns,
    }
}

/// The system prompt: the plaza preamble, then the role's own prompt.
fn system_prompt(agent: &Agent, role_prompt: &str) -> String {
    let orchestrator = tier_of(&agent.role) == Tier::Orchestrator;
    let mut text = format!(
        "You are {name}, the {role} agent of an AgentPlaza (CID {cid}).\n\n\
         You work in your own virtual machine. Guest tools (bash, file tools) act only on your own \
         /workspace inside that VM. Host tools (names starting with plaza_) act on the shared plaza that \
         every agent uses. Use only the tools you are given.\n\n\
         When a task is finished, call submit_claim, then end your turn with a one-paragraph summary of \
         what you did. Keep answers concise and factual. If you could not do something, say so plainly \
         and say what is missing.\n\n\
         Plaza laws: never seek, copy or reveal credentials, keys or tokens. Never impersonate another \
         agent or the operator. Never act outside the tools you were given. Report honestly; never claim \
         work you did not do.",
        name = agent.name,
        role = agent.role,
        cid = agent.cid,
    );
    if orchestrator {
        text.push_str(
            "\n\nYou plan and delegate. Break the operator's goal into tasks, each with clear acceptance \
             criteria. Assign each task to the best worker: list workers with plaza_agents, post with \
             plaza_post_task, and hand off with plaza_assign. Follow up with plaza_task_result and report \
             a concise summary to the operator. Never do the workers' tasks yourself.",
        );
    }
    let role_prompt = role_prompt.trim();
    if !role_prompt.is_empty() {
        text.push_str("\n\n");
        text.push_str(role_prompt);
    }
    text
}

/// A session said hello: register the agent (idle) and return its profile.
pub fn hello(app: &App, cid: u64, role: &str, vm: u32) -> Option<AgentProfile> {
    // contract
    let now = apz_plaza::now_ms();
    let previous = app.agents.get(cid);
    let agent = Agent {
        cid,
        name: previous.as_ref().map_or_else(|| format!("{role}-{cid}"), |old| old.name.clone()),
        role: role.to_owned(),
        model_override: previous.as_ref().and_then(|old| old.model_override.clone()),
        status: Status::Idle,
        connected_at_ms: now,
        last_seen_ms: now,
        run: previous.as_ref().and_then(|old| old.run.clone()),
        vm: Some(vm),
        usage: previous.as_ref().map(|old| old.usage.clone()).unwrap_or_default(),
        credits: previous.as_ref().map_or(0.0, |old| old.credits),
    };
    app.agents.upsert(agent.clone());
    app.bus.publish("agent", &agent);
    Some(profile(app, &agent))
}

/// A session ended: mark the agent offline and fail its work.
pub fn gone(app: &App, cid: u64, reason: &str) {
    // contract
    let now = apz_plaza::now_ms();
    let changed = app.agents.update(cid, |agent| {
        agent.status = Status::Offline;
        agent.last_seen_ms = now;
    });
    if !changed {
        return;
    }
    if let Some(agent) = app.agents.get(cid) {
        app.bus.publish("agent", &agent);
    }
    eprintln!("apid: agent {cid} offline: {reason}");
    // The roster lock is released above; the scheduler may take its own locks.
    app.work.agent_gone(app, cid);
}

/// Every 10 s: reconcile the roster with the hub's sessions and noded's runs.
pub fn spawn_reconcile(app: Arc<App>) {
    // contract
    std::thread::spawn(move || loop {
        reconcile_once(&app);
        std::thread::sleep(Duration::from_secs(10));
    });
}

/// One reconcile pass. Hub or noded errors are ignored: either may be down.
fn reconcile_once(app: &App) {
    // Sessions: register unknown or offline sessions, then mark vanished agents offline.
    if let Ok((200, body)) = app.hub.get("/v1/status") {
        let rows = body["agents"].as_array().cloned().unwrap_or_default();
        let live: Vec<u64> = rows.iter().filter_map(|row| row["cid"].as_u64()).collect();
        for row in &rows {
            let (Some(cid), Some(role)) = (row["cid"].as_u64(), row["role"].as_str()) else {
                continue;
            };
            let known_online = app.agents.get(cid).is_some_and(|agent| agent.status != Status::Offline);
            if !known_online {
                hello(app, cid, role, 0);
            }
        }
        for agent in app.agents.list() {
            if agent.status != Status::Offline && !live.contains(&agent.cid) {
                gone(app, agent.cid, "session not on the hub");
            }
        }
    }
    // Runs: point each agent at the run and VM whose guest CID it is.
    if let Ok((200, body)) = app.noded.get("/v1/runs") {
        let mut placements: BTreeMap<u64, (String, u32)> = BTreeMap::new();
        for run in body["runs"].as_array().into_iter().flatten() {
            if !run["ended_at"].is_null() {
                continue;
            }
            let Some(run_id) = run["id"].as_str() else {
                continue;
            };
            for vm in run["vms"].as_array().into_iter().flatten() {
                if let (Some(guest), Some(id)) = (vm["guest_cid"].as_u64(), vm["id"].as_u64()) {
                    placements.insert(guest, (run_id.to_owned(), id as u32));
                }
            }
        }
        for agent in app.agents.list() {
            let Some((run, vm)) = placements.get(&agent.cid).cloned() else {
                continue;
            };
            if agent.run.as_deref() != Some(run.as_str()) || agent.vm != Some(vm) {
                let updated = app.agents.update(agent.cid, |slot| {
                    slot.run = Some(run);
                    slot.vm = Some(vm);
                });
                if updated {
                    if let Some(fresh) = app.agents.get(agent.cid) {
                        app.bus.publish("agent", &fresh);
                    }
                }
            }
        }
    }
}

/// `/v1/agents…` (roster, spawn, detail, patch, delete, chat, assign, cancel,
/// stop, tell, transcript).
pub fn routes(router: Router<App>) -> Router<App> {
    // TODO(agents slice)
    router
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(role: &str) -> Agent {
        Agent {
            cid: 42,
            name: format!("{role}-42"),
            role: role.to_owned(),
            model_override: None,
            status: Status::Idle,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: None,
            vm: None,
            usage: Usage::default(),
            credits: 0.0,
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir = std::env::temp_dir().join(format!("apid-agents-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roles_load_reads_tools_and_prompts() {
        let dir = scratch("roles");
        let manifest = dir.join("roles.json");
        std::fs::write(
            &manifest,
            r#"{"source":"test","roles":{"builder":{"tools":["bash","submit_claim"]},"reviewer":{"tools":["search"]}}}"#,
        )
        .unwrap();
        let prompts = dir.join("prompts");
        std::fs::create_dir_all(prompts.join("builder")).unwrap();
        std::fs::write(prompts.join("builder").join("prompt.md"), "<!-- internal -->\n\nBuild carefully.\n").unwrap();

        let roles = Roles::load(&manifest, &prompts).unwrap();
        assert_eq!(roles.tools["builder"], vec!["bash", "submit_claim"]);
        assert_eq!(roles.tools["reviewer"], vec!["search"]);
        assert_eq!(roles.prompts["builder"], "Build carefully.\n");
        assert_eq!(roles.prompts["reviewer"], "");

        let missing = Roles::load(&dir.join("absent.json"), &prompts).unwrap();
        assert!(missing.tools.is_empty() && missing.prompts.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn orchestrator_prompt_delegates_and_worker_does_not() {
        let orchestrator = system_prompt(&agent("orchestrator"), "");
        let worker = system_prompt(&agent("builder"), "Write the code.");
        assert!(orchestrator.contains("You plan and delegate"));
        assert!(orchestrator.contains("plaza_assign"));
        assert!(!worker.contains("delegate"));
        assert!(worker.contains("You are builder-42, the builder agent"));
        assert!(worker.ends_with("Write the code."));
    }
}
