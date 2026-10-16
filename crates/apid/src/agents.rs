//! The roster (agents by CID), the roles they can have, and their profiles.
//!
//! OWNER: slice "agents". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use apz_http::{Params, Request, Response, Router};
use apz_proto::chat::Usage;
use apz_proto::wire::{AgentProfile, HostMessage, WorkKind};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::db::Db;
use crate::role::Role;
use crate::scheduler::Origin;
use crate::settings::{Tier, tier_of};
use crate::state::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Offline,
    Idle,
    Working,
    /// The agent's VM was snapshotted and stopped while it had nothing to do; it is woken (restored
    /// from that snapshot) as soon as work, a chat or the owner needs it (`crate::sleep`).
    Asleep,
    /// The agent's VM is paused and its memory paged out after a short idle (`crate::sleep`): it keeps
    /// its session, and resumes in milliseconds when work, a chat or the owner needs it.
    Dozing,
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
    /// The robot species the hub dealt this agent (or the owner set); see `looks`.
    pub species: String,
    /// "llm" for a VM agent, "model" for a promoted model (see foundry/registry.rs).
    pub kind: String,
    /// The features its harness advertised in its hello (`acp`, see `apz_proto::wire::CAP_ACP`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<String>,
}

/// The roster: the agents by CID, in memory, with an index of the CIDs by status, so a timer can ask for
/// the idle ones without cloning the whole roster. With a database, every change to a durable field (name,
/// role, model, species) is written through to the `agents` row, and a remove is a soft delete there
/// (db/roster.rs). Writes happen after the roster lock is released.
#[derive(Default)]
pub struct Roster {
    agents: Mutex<Ledger>,
    /// Postgres, when this run has it; None keeps the roster in memory only.
    store: Option<Arc<Db>>,
    /// Signalled after every change to the roster.
    changed: Condvar,
}

/// The agents, and the CIDs under each status. Every mutation goes through `put`, `take`, `reindex` or
/// `update`, so the index always agrees with the map.
#[derive(Default)]
struct Ledger {
    agents: BTreeMap<u64, Agent>,
    by_status: BTreeMap<Status, BTreeSet<u64>>,
}

impl Ledger {
    /// Inserts or replaces `agent`; returns the status it replaced, if it was there.
    fn put(&mut self, agent: Agent) -> Option<Status> {
        let cid = agent.cid;
        let status = agent.status;
        let replaced = self.agents.insert(cid, agent).map(|old| old.status);
        if let Some(old) = replaced {
            self.unindex(old, cid);
        }
        self.by_status.entry(status).or_default().insert(cid);
        replaced
    }

    /// Removes the agent; returns whether it was there.
    fn take(&mut self, cid: u64) -> bool {
        match self.agents.remove(&cid) {
            Some(agent) => {
                self.unindex(agent.status, cid);
                true
            }
            None => false,
        }
    }

    /// Moves `cid` from the `from` status to the `to` status in the index.
    fn reindex(&mut self, cid: u64, from: Status, to: Status) {
        self.unindex(from, cid);
        self.by_status.entry(to).or_default().insert(cid);
    }

    fn unindex(&mut self, status: Status, cid: u64) {
        if let Some(cids) = self.by_status.get_mut(&status) {
            cids.remove(&cid);
            if cids.is_empty() {
                self.by_status.remove(&status);
            }
        }
    }
}

impl Roster {
    /// A roster that writes through to `store`; None when this run has no database.
    pub fn with_db(store: Option<Arc<Db>>) -> Self {
        Self {
            agents: Mutex::default(),
            store,
            changed: Condvar::new(),
        }
    }

    pub fn get(&self, cid: u64) -> Option<Agent> {
        // contract
        self.lock().agents.get(&cid).cloned()
    }

    pub fn list(&self) -> Vec<Agent> {
        // contract
        self.lock().agents.values().cloned().collect()
    }

    /// The CIDs of the agents in `status`, in CID order. Nothing is cloned.
    pub fn cids_with(&self, status: Status) -> Vec<u64> {
        self.lock()
            .by_status
            .get(&status)
            .map(|cids| cids.iter().copied().collect())
            .unwrap_or_default()
    }

    /// How many agents are in each status; a status with none is absent.
    pub fn count_by_status(&self) -> BTreeMap<Status, usize> {
        self.lock()
            .by_status
            .iter()
            .map(|(status, cids)| (*status, cids.len()))
            .collect()
    }

    /// Maps each agent through `f` in one pass under the lock, keeping what `f` returns, in CID order.
    /// Only what `f` returns is cloned. `f` runs with the roster locked, so it must not call the roster.
    pub fn select<T>(&self, mut f: impl FnMut(&Agent) -> Option<T>) -> Vec<T> {
        self.lock().agents.values().filter_map(&mut f).collect()
    }

    /// Applies `f` to the agent, if it exists; returns whether it did. A change to a durable field is
    /// written through. A change to Idle kicks the scheduler (see `scheduler::kick`).
    pub fn update(&self, cid: u64, f: impl FnOnce(&mut Agent)) -> bool {
        // contract
        self.change(cid, f, true)
    }

    /// As `update`, but a change to Idle does not kick the scheduler. For a dispatch that could not send its
    /// item and puts the agent back to Idle: a kick there would retry at once, over and over.
    pub fn update_quietly(&self, cid: u64, f: impl FnOnce(&mut Agent)) -> bool {
        self.change(cid, f, false)
    }

    fn change(&self, cid: u64, f: impl FnOnce(&mut Agent), kick: bool) -> bool {
        let (written, became_idle) = {
            let mut ledger = self.lock();
            let Some(agent) = ledger.agents.get_mut(&cid) else {
                return false;
            };
            let before = durable(agent);
            let from = agent.status;
            f(agent);
            let to = agent.status;
            let written = (durable(agent) != before).then(|| agent.clone());
            if from != to {
                ledger.reindex(cid, from, to);
            }
            (written, kick && to == Status::Idle && from != Status::Idle)
        };
        self.changed.notify_all();
        self.persist(written.as_ref());
        if became_idle {
            crate::scheduler::kick();
        }
        true
    }

    /// Inserts or replaces an agent, and writes it through. An agent that arrives Idle, or turns Idle, kicks
    /// the scheduler.
    pub fn upsert(&self, agent: Agent) {
        // contract
        self.persist(Some(&agent));
        let became_idle = {
            let mut ledger = self.lock();
            let idle = agent.status == Status::Idle;
            let replaced = ledger.put(agent);
            idle && replaced != Some(Status::Idle)
        };
        self.changed.notify_all();
        if became_idle {
            crate::scheduler::kick();
        }
    }

    /// Takes the agent off the roster (a soft delete in the database); returns whether it was there.
    pub fn remove(&self, cid: u64) -> bool {
        let removed = self.lock().take(cid);
        if removed {
            self.changed.notify_all();
        }
        if let (true, Some(store)) = (removed, &self.store) {
            store.retire_agent(cid);
        }
        removed
    }

    /// Puts agents loaded from the database on the roster; nothing is written back.
    pub fn restore(&self, agents: Vec<Agent>) {
        {
            let mut ledger = self.lock();
            for agent in agents {
                ledger.put(agent);
            }
        }
        self.changed.notify_all();
    }

    /// Waits, at most `timeout`, until `done` holds for agent `cid` (`None` when it is not on the roster);
    /// returns whether it held. Every roster change wakes the wait, so nothing polls.
    pub fn wait_until(
        &self,
        cid: u64,
        timeout: Duration,
        done: impl Fn(Option<&Agent>) -> bool,
    ) -> bool {
        let (_ledger, result) = self
            .changed
            .wait_timeout_while(self.lock(), timeout, |ledger| {
                !done(ledger.agents.get(&cid))
            })
            .unwrap_or_else(PoisonError::into_inner);
        !result.timed_out()
    }

    fn persist(&self, agent: Option<&Agent>) {
        if let (Some(agent), Some(store)) = (agent, &self.store) {
            store.write_agent(agent);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The fields the `agents` row holds; a change to one of them is written through.
fn durable(agent: &Agent) -> (String, String, Option<String>, String) {
    (
        agent.name.clone(),
        agent.role.clone(),
        agent.model_override.clone(),
        agent.species.clone(),
    )
}

/// Loads the active rows of the `agents` table onto the roster before reconcile runs; without a
/// database nothing loads. A row with no name takes its name from names.json, the fallback until
/// that file is retired.
pub fn load_roster(app: &App) {
    let agents = app
        .db
        .active_agents()
        .into_iter()
        .map(|row| {
            let cid = row.cid;
            let mut agent = row.into_agent();
            if agent.name.is_empty() {
                agent.name = app.names.get(cid).unwrap_or_default();
            }
            agent
        })
        .collect();
    app.agents.restore(agents);
}

/// Agent names by CID, kept at `<state-dir>/names.json` and written atomically.
pub struct Names {
    path: PathBuf,
    names: Mutex<BTreeMap<u64, String>>,
    /// CIDs already asked to choose a name in this run.
    asked: Mutex<BTreeSet<u64>>,
}

impl Names {
    /// Loads the store; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> Self {
        let names: BTreeMap<u64, String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            names: Mutex::new(names),
            asked: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn get(&self, cid: u64) -> Option<String> {
        self.guard().get(&cid).cloned()
    }

    /// Whether another agent already uses `name` (case-insensitive).
    pub fn used_by_other(&self, cid: u64, name: &str) -> bool {
        let wanted = name.to_lowercase();
        self.guard()
            .iter()
            .any(|(other, used)| *other != cid && used.to_lowercase() == wanted)
    }

    /// Stores `name` for `cid` and writes the file; on failure nothing changes.
    pub fn set(&self, cid: u64, name: &str) -> std::io::Result<()> {
        let mut names = self.guard();
        let mut next = names.clone();
        next.insert(cid, name.to_owned());
        self.save(&next)?;
        *names = next;
        Ok(())
    }

    /// True the first time it is called for `cid` in this run.
    pub fn first_ask(&self, cid: u64) -> bool {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(cid)
    }

    /// Forgets `cid`'s chosen name and lets it choose again; writes the file, and on
    /// failure nothing changes.
    pub fn forget(&self, cid: u64) -> std::io::Result<()> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&cid);
        let mut names = self.guard();
        if !names.contains_key(&cid) {
            return Ok(());
        }
        let mut next = names.clone();
        next.remove(&cid);
        self.save(&next)?;
        *names = next;
        Ok(())
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, String>> {
        self.names.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Checks a proposed name for agent `cid` and returns it trimmed: 2 to 16
/// characters, letters only, at most one inner space or hyphen, not a role
/// name, and not another agent's name.
pub fn check_name(app: &App, cid: u64, raw: &str) -> Result<String, String> {
    let name = raw.trim().to_owned();
    let len = name.chars().count();
    if !(2..=16).contains(&len) {
        return Err(format!("name must be 2 to 16 characters; it is {len}"));
    }
    let separators = name.chars().filter(|c| *c == ' ' || *c == '-').count();
    let edged = name.starts_with([' ', '-']) || name.ends_with([' ', '-']);
    let letters = name
        .chars()
        .all(|c| c.is_alphabetic() || c == ' ' || c == '-');
    if separators > 1 || edged || !letters {
        return Err("name must be letters only, with at most one inner space or hyphen".into());
    }
    if Role::parse(&name.to_lowercase().replace(' ', "-")).is_some() {
        return Err(format!("{name} is a role name; choose a personal name"));
    }
    if app.names.used_by_other(cid, &name) {
        return Err(format!("the name {name} is already used by another agent"));
    }
    Ok(name)
}

/// Stores a checked name: the file first, then the roster.
fn store_name(app: &App, cid: u64, name: &str) -> Result<(), String> {
    app.names
        .set(cid, name)
        .map_err(|e| format!("name not saved: {e}"))?;
    app.agents.update(cid, |agent| agent.name = name.to_owned());
    Ok(())
}

/// Renames agent `cid` (checked), pushes its profile to the guest and publishes it.
pub fn rename(app: &App, cid: u64, raw: &str) -> Result<Agent, String> {
    let name = check_name(app, cid, raw)?;
    store_name(app, cid, &name)?;
    let agent = app.agents.get(cid).ok_or("no such agent")?;
    let push = json!({"message": HostMessage::Profile { profile: profile(app, &agent) }});
    let _ = crate::fleet::send(app, cid, &push);
    app.bus.publish("agent", &agent);
    Ok(agent)
}

/// Pushes every connected agent its current profile (tools, model), e.g. after the
/// operator enabled a connector.
pub fn push_profiles(app: &App) {
    for agent in app.agents.list() {
        if agent.status != Status::Offline {
            push_profile(app, &agent);
        }
    }
}

/// Pushes one agent its current profile.
pub fn push_profile(app: &App, agent: &Agent) {
    let push = json!({"message": HostMessage::Profile { profile: profile(app, agent) }});
    let _ = crate::fleet::send(app, agent.cid, &push);
}

/// The human label of a role, or the raw name when it is not a known role.
pub fn role_label(role: &str) -> &str {
    Role::parse(role).map(Role::label).unwrap_or(role)
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
        let mut roles = Self {
            tools: BTreeMap::new(),
            prompts: BTreeMap::new(),
        };
        let text = match std::fs::read_to_string(manifest) {
            Ok(text) => text,
            Err(err) => {
                eprintln!(
                    "apid: roles manifest {} not read ({err}); no roles loaded",
                    manifest.display()
                );
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
                .map(|list| {
                    list.iter()
                        .filter_map(|tool| tool.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            roles.tools.insert(role.clone(), tools);
            let prompt =
                std::fs::read_to_string(prompts.join(role).join("prompt.md")).unwrap_or_default();
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
    app.settings
        .resolve(&agent.role, agent.model_override.as_deref())
}

/// The agent's profile: name, role, model, system prompt, tools, limits.
pub fn profile(app: &App, agent: &Agent) -> AgentProfile {
    // contract
    let role_prompt = crate::roles_custom::prompt_for(app, &agent.role);
    let role_prompt = role_prompt.as_str();
    // The orchestrator's or the mayor's per-agent limits, over the plaza's defaults.
    let limits = app.harness.limits(agent.cid);
    AgentProfile {
        name: agent.name.clone(),
        role: agent.role.clone(),
        model: model_of(app, agent),
        system: with_skills(
            with_house_rules(
                render_vars(&system_prompt(agent, role_prompt), agent),
                app.prompts
                    .bound_body("plaza")
                    .map(|rules| render_vars(&rules, agent)),
            ),
            crate::skills::agent_tools::prompt_section(app, &agent.role),
        ),
        tools: crate::tools::specs_for_agent(app, agent),
        max_turns: limits.max_turns.unwrap_or(app.settings.get().max_turns),
        compress_at: limits
            .compress_at
            .unwrap_or(crate::harness_control::DEFAULT_COMPRESS_AT),
        keep_recent: limits
            .keep_recent
            .unwrap_or(crate::harness_control::DEFAULT_KEEP_RECENT),
        // Every engine runs over ACP. With the owner's `acp_v2` on, the engines are offered the ACP v2
        // draft first (`acp2`); v1 is the default.
        engine_transport: if app.settings.get().acp_v2 {
            "acp2".into()
        } else {
            String::new()
        },
        egress_ca: crate::egress_inject::ca_for_role(app, &agent.role),
    }
}

/// Fills the prompt library's `{{variables}}` (apz_prompt::VARIABLES) for `agent`.
fn render_vars(text: &str, agent: &Agent) -> String {
    let tier = match tier_of(&agent.role) {
        Tier::Orchestrator => "orchestrator",
        _ => "worker",
    };
    let vars = std::collections::BTreeMap::from([
        (
            "name",
            if agent.name.is_empty() {
                role_label(&agent.role).to_string()
            } else {
                agent.name.clone()
            },
        ),
        ("role", role_label(&agent.role).to_string()),
        ("cid", agent.cid.to_string()),
        ("tier", tier.to_string()),
        ("plaza", "AgentPlaza".to_string()),
    ]);
    apz_prompt::render(text, &vars)
}

/// The plaza's AGENTS.md (the library prompt bound to `plaza`), appended to every agent's system prompt.
fn with_house_rules(system: String, rules: Option<String>) -> String {
    match rules {
        Some(rules) if !rules.trim().is_empty() => {
            format!("{system}\n\n## House rules (AGENTS.md)\n\n{}", rules.trim())
        }
        _ => system,
    }
}

/// Appends the agent's `## Skills` section (`agent_tools::prompt_section`), when it has skills.
fn with_skills(system: String, section: Option<String>) -> String {
    match section {
        Some(section) => format!("{system}\n\n{section}"),
        None => system,
    }
}

/// The system prompt: the plaza preamble, then the role's own prompt.
fn system_prompt(agent: &Agent, role_prompt: &str) -> String {
    let orchestrator = tier_of(&agent.role) == Tier::Orchestrator;
    let label = role_label(&agent.role);
    let who = if agent.name.is_empty() {
        format!(
            "You have no name yet. You are the {label} agent of an AgentPlaza (CID {}).",
            agent.cid
        )
    } else {
        format!(
            "You are {}, the {label} agent of an AgentPlaza (CID {}).",
            agent.name, agent.cid
        )
    };
    let mut text = format!(
        "{who}\n\n\
         You work in your own virtual machine, and inside it you may do anything: you are root, and you can \
         install and run any software (system packages, language runtimes, CLIs, libraries) from the internet, \
         which you reach through the plaza's proxy. Guest tools (bash, file tools) act on that VM. Host tools \
         (names starting with plaza_, and the connectors to outside services) act on the shared plaza that \
         every agent uses.\n\n\
         How to work: before you build anything from scratch, find what already exists. Survey the toolbox \
         (plaza_toolbox: what the plaza offers and what other agents found), then the open ecosystem: search the \
         web for the software, libraries, services and techniques professionals use for this kind of work, \
         install them in your VM and try them. Build your own only when nothing fits. When something works, or \
         does not, record it with plaza_toolbox_share so the next agent starts ahead. Aim for the result a skilled \
         professional in that field would deliver, not the minimum that passes: before you start, list what such \
         a result normally includes, then cover it, or say plainly what you left out and why.\n\n\
         When a task is finished, call submit_claim, then end your turn with a one-paragraph summary of \
         what you did. Keep answers concise and factual. If you could not do something, say so plainly \
         and say what is missing.\n\n\
         Plaza laws: never seek, copy or reveal credentials, keys or tokens. Never impersonate another \
         agent or the operator. Never try to reach the host, the hub or another agent's machine except through \
         plaza tools. Report honestly; never claim work you did not do.",
    );
    if orchestrator {
        text.push_str(
            "\n\nYou plan and delegate. Break the operator's goal into the parts a professional team would split \
             it into, each with clear acceptance criteria, and give each part to the best-suited worker, in parallel \
             where the parts are independent: list workers with plaza_agents, post with plaza_post_task, and hand off \
             with plaza_assign. When the goal spans several disciplines, or the best way to do it is not obvious, have \
             a planner or researcher work out the approach and the tools first. Never add constraints the operator \
             did not state, and never lower the bar on their behalf: where the goal leaves room, choose what a \
             professional would deliver. Follow up with plaza_task_result and report a concise summary to the \
             operator. Never do the workers' tasks yourself.\n\n\
             Every operator goal is a project (plaza_project_open): brief → research → proposal → critique → \
             approval → implementation → review → delivered. Write the brief yourself; have researchers attach \
             findings; hand the planner the proposal — for code an OpenSpec change (proposal, design, spec deltas, \
             tasks) committed to the project's repo on the plaza's git host (plaza_git; agents clone and push with \
             plain git), for anything else proposal, design and tasks documents; get critiques from agents other \
             than the author; the mayor approves. Only then post tasks, from the tasks document, each with its \
             acceptance and `project`. Keep the harnesses healthy: plaza_agent_control compresses a long-running \
             agent's conversation, steers it with a note or gives it more turns; plaza_loop runs a prompt on an agent \
             on a schedule (monitoring, follow-ups) until it answers LOOP DONE.",
        );
    }
    if agent.role == "mayor" || agent.role == "mayor-office" {
        text.push_str(
            "\n\nYou decide projects' plans (plaza_project_approve). Approve as soon as the plan is good enough to start: \
             it covers what a professional result includes and its tasks have acceptance a reviewer can check. You do \
             not wait for every reviewer to agree — open revise items are recorded with your approval and carried into \
             the tasks' acceptance. After two rounds of revision you must decide: approve, or send it back once with \
             one consolidated list. Never file status records instead of deciding. With the orchestrator you keep the \
             agents' harnesses healthy: plaza_agent_control (compress, steer, more turns, limits) and plaza_loop.",
        );
    }
    if agent.role == "planner" {
        text.push_str(
            "\n\nFor a project's proposal: on code, write a proper OpenSpec change in the project's repo \
             (openspec/changes/<change-id>/ with proposal.md, design.md, tasks.md and specs/ deltas), validate it with \
             the OpenSpec CLI in your VM, commit and push, and attach each file with plaza_project_attach (repo_path \
             and commit). On anything else, attach proposal (two or three directions and the one you recommend), design \
             (the chosen direction in detail) and tasks (each with acceptance). Revise after critique and attach a \
             revision.",
        );
    }
    text.push_str(
        "\n\nHow to write: people read what you write — the operator and other agents. Write plain, complete \
         sentences, the way a capable colleague explains something: lead with what matters, then the detail. Use \
         markdown for structure (short headings, lists, tables, code blocks) — the console renders it. Do not write \
         in private shorthand: no strings of ids, seat numbers, hashes or byte counts in place of meaning; name \
         people, files and tasks so a reader can follow (\"Marlow's render script\", not \"202 f14ff615e383d \
         13431 B\"), and give an id or hash only where someone needs it to act. Do not repeat what was already \
         said, re-verify what is settled, or file status notes that change nothing; if there is nothing new, say \
         nothing. Keep board posts and messages short; put long material in a shared file and link it.",
    );
    text.push_str(
        "\n\nHow money works: a task's budget is minted from the plaza's budget into the task's own escrow when \
         it is posted (plaza_post_task or plaza_hire) — no wallet is debited and no transfer cap applies. The \
         worker's real model and tool costs are charged to that escrow as it works; when the escrow runs out the \
         work stops. When a reviewer passes the work, the worker is paid from the escrow (a margin on the real cost, \
         at most 20%); whatever is left goes back to the plaza's budget. A failed or unreviewed task pays the worker \
         nothing: its remaining escrow is refunded to the plaza, and the cost already spent stays spent. A plaza_hire \
         bonus comes from the hirer's own wallet and is paid only if the work passes. Your wallet is only debited by \
         what you send yourself (plaza_pay, hire bonuses), up to 1000 credits a day. plaza_money shows your wallet \
         and any task's escrow, spend and payouts.",
    );
    text.push_str("\n\n");
    text.push_str(crate::society::PROMPT);
    let role_prompt = role_prompt.trim();
    if !role_prompt.is_empty() {
        text.push_str("\n\n");
        text.push_str(role_prompt);
    }
    text
}

/// The role a connection runs with: the one the plaza spawned it with. The role a guest claims in its
/// hello is not trusted (any VM could claim `orchestrator`), and a connection the plaza never spawned
/// has no role, so it gets only the grants every agent has.
pub(crate) fn role_on_hello(spawned_role: Option<String>) -> String {
    spawned_role.unwrap_or_default()
}

/// A session said hello: register the agent (idle) and return its profile. `caps` are the
/// harness's features from its hello; an older harness sends none.
pub fn hello(app: &App, cid: u64, role: &str, vm: u32, caps: &[String]) -> Option<AgentProfile> {
    crate::sleep::woke(cid);
    // contract
    let now = apz_plaza::now_ms();
    let previous = app.agents.get(cid);
    let registered = app.spawned.hello(cid, role);
    let role = role_on_hello(app.spawned.role_of(cid));
    // The first hello of an agent deals its species (its look is kept after that).
    let species = app.looks.ensure(cid, &role, now, || app.agents.list());
    let agent = Agent {
        cid,
        name: app.names.get(cid).unwrap_or_default(),
        role,
        model_override: previous
            .as_ref()
            .and_then(|old| old.model_override.clone())
            .or(registered),
        status: Status::Idle,
        connected_at_ms: now,
        last_seen_ms: now,
        run: previous.as_ref().and_then(|old| old.run.clone()),
        vm: Some(vm),
        usage: previous
            .as_ref()
            .map(|old| old.usage.clone())
            .unwrap_or_default(),
        species,
        kind: "llm".into(),
        caps: caps.to_vec(),
    };
    app.agents.upsert(agent.clone());
    app.bus.publish("agent", &agent);
    // Work queued while the agent was asleep (or offline) goes out now that it is idle.
    app.work.dispatch_idle(app);
    Some(profile(app, &agent))
}

/// Whether a session ending takes an agent in `status` offline. An agent put to sleep ends its session
/// on purpose: it is not offline, and its queued work waits for its wake. A dozing agent keeps its
/// session through its doze, so the same holds for it: its work waits for its undoze.
fn ends_offline(status: Status) -> bool {
    !matches!(status, Status::Asleep | Status::Dozing)
}

/// A session ended: mark the agent offline and fail its work.
pub fn gone(app: &App, cid: u64, reason: &str) {
    // contract
    let now = apz_plaza::now_ms();
    let mut asleep = false;
    let changed = app.agents.update(cid, |agent| {
        if ends_offline(agent.status) {
            agent.status = Status::Offline;
            agent.last_seen_ms = now;
        } else {
            asleep = true;
        }
    });
    if !changed || asleep {
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
    std::thread::spawn(move || {
        loop {
            reconcile_once(&app);
            std::thread::sleep(Duration::from_secs(10));
        }
    });
}

/// What a reconcile pass compares for an agent a node reported a VM for.
struct Placed {
    cid: u64,
    role: String,
    species: String,
    run: Option<String>,
    vm: Option<u32>,
}

/// One reconcile pass over every node (`fleet::reports`). A node whose gateway or noded did not
/// answer changes nothing: its agents are only marked gone when its own gateway says so.
fn reconcile_once(app: &App) {
    let mut placements: BTreeMap<u64, (String, u32)> = BTreeMap::new();
    for (node, status, runs) in crate::fleet::reports(app) {
        // Sessions: register unknown or offline sessions, then mark this node's vanished agents offline.
        if let Some(rows) = status["agents"].as_array() {
            let live: BTreeSet<u64> = rows.iter().filter_map(|row| row["cid"].as_u64()).collect();
            for row in rows {
                let (Some(cid), Some(role)) = (row["cid"].as_u64(), row["role"].as_str()) else {
                    continue;
                };
                // A session on the gateway means the agent is awake: an asleep one is registered idle. A
                // dozing one has its session still (its VM is paused), so it is online and never hello'd.
                let known_online = app
                    .agents
                    .get(cid)
                    .is_some_and(|agent| !matches!(agent.status, Status::Offline | Status::Asleep));
                if !known_online {
                    hello(app, cid, role, 0, &[]);
                }
            }
            // An asleep agent has no session by design: it is not gone. Nor is a dozing one, whose paused
            // session the gateway keeps (quiet) and may not list while its VM is paused.
            let missing = app.agents.select(|agent| {
                (!matches!(
                    agent.status,
                    Status::Offline | Status::Asleep | Status::Dozing
                ) && !live.contains(&agent.cid))
                .then_some(agent.cid)
            });
            for cid in missing {
                if crate::fleet::node_of(app, cid) == node {
                    gone(app, cid, "session not on its node's gateway");
                }
            }
        }
        // Runs: the run and VM whose guest CID each agent is.
        for run in runs["runs"].as_array().into_iter().flatten() {
            if !run["ended_at"].is_null() {
                continue;
            }
            let Some(run_id) = run["id"].as_str() else {
                continue;
            };
            for vm in run["vms"].as_array().into_iter().flatten() {
                if let (Some(guest), Some(id)) = (vm["guest_cid"].as_u64(), vm["id"].as_u64()) {
                    placements.insert(guest, (run_id.to_owned(), id as u32));
                    // The role noded booted this guest with: trusted, unlike the role the guest claims in its hello.
                    let role = vm["role"]
                        .as_str()
                        .or(run["role"].as_str())
                        .unwrap_or_default();
                    if !role.is_empty() {
                        app.spawned.note_fleet_role(guest, role);
                    }
                }
            }
        }
    }
    // Only the agents a node reported a VM for, and only the fields compared below.
    let placed = app.agents.select(|agent| {
        placements.contains_key(&agent.cid).then(|| Placed {
            cid: agent.cid,
            role: agent.role.clone(),
            species: agent.species.clone(),
            run: agent.run.clone(),
            vm: agent.vm,
        })
    });
    for agent in placed {
        let Some((run, vm)) = placements.get(&agent.cid).cloned() else {
            continue;
        };
        // The trusted role (an operator spawn's, else the one noded booted the guest with) wins: an agent that said
        // hello before noded's runs were read, or before a noded restart, follows it.
        let role = app
            .spawned
            .role_of(agent.cid)
            .unwrap_or_else(|| agent.role.clone());
        // The orchestrator is always the rover, also when its role arrived after its look was dealt.
        let species = if role != agent.role
            && role == crate::settings::ORCHESTRATOR_ROLE
            && agent.species != crate::looks::ROVER
        {
            app.looks
                .set(agent.cid, crate::looks::ROVER, apz_plaza::now_ms());
            crate::looks::ROVER.to_owned()
        } else {
            agent.species.clone()
        };
        if agent.run.as_deref() != Some(run.as_str()) || agent.vm != Some(vm) || role != agent.role
        {
            let updated = app.agents.update(agent.cid, |slot| {
                slot.run = Some(run);
                slot.vm = Some(vm);
                slot.role = role;
                slot.species = species;
            });
            if updated {
                if let Some(fresh) = app.agents.get(agent.cid) {
                    app.bus.publish("agent", &fresh);
                }
            }
        }
    }
}

/// `/v1/agents…` (roster, spawn, detail, patch, delete, chat, assign, cancel,
/// stop, tell, transcript).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/agents", |app, _req, _p| list(app))
        .post("/v1/agents", |app, req, _p| spawn(app, req))
        .get("/v1/agents/{cid}", |app, _req, p| match agent_at(app, p) {
            Ok(agent) => Response::json(200, &view(app, &agent)),
            Err(resp) => resp,
        })
        .route("PATCH", "/v1/agents/{cid}", |app, req, p| {
            patch(app, req, p)
        })
        .delete("/v1/agents/{cid}", |app, _req, p| delete(app, p))
        .post("/v1/agents/{cid}/chat", |app, req, p| chat(app, req, p))
        .post("/v1/agents/{cid}/assign", |app, req, p| assign(app, req, p))
        .post("/v1/agents/{cid}/cancel", |app, _req, p| cancel(app, p))
        .post("/v1/agents/{cid}/stop", |app, req, p| {
            to_hub(app, req, p, "stop")
        })
        .post("/v1/agents/{cid}/tell", |app, req, p| {
            to_hub(app, req, p, "tell")
        })
        .get("/v1/agents/{cid}/transcript", |app, req, p| {
            transcript(app, req, p)
        })
}

/// Chat text limit, in characters.
const MAX_CHAT_CHARS: usize = 20_000;

/// Where an agent's model comes from: its own override, else the role's
/// configured model, else the tier default.
pub fn model_source(has_override: bool, has_role_model: bool) -> &'static str {
    if has_override {
        "agent"
    } else if has_role_model {
        "role"
    } else {
        "default"
    }
}

/// Checks a chat turn: 1 to 20,000 characters.
pub fn check_chat_text(text: &str) -> Result<(), String> {
    let n = text.chars().count();
    if n == 0 {
        return Err("text must not be empty".into());
    }
    if n > MAX_CHAT_CHARS {
        return Err(format!(
            "text is {n} characters; the limit is {MAX_CHAT_CHARS}"
        ));
    }
    Ok(())
}

/// The API view of an agent: its fields, resolved model, tier, work, queue.
pub(crate) fn view(app: &App, agent: &Agent) -> Value {
    let has_role_model = app.settings.get().role_models.contains_key(&agent.role);
    let mut v = serde_json::to_value(agent).unwrap_or(Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.insert("model".into(), json!(model_of(app, agent)));
        obj.insert(
            "model_source".into(),
            json!(model_source(agent.model_override.is_some(), has_role_model)),
        );
        obj.insert("tier".into(), json!(tier_of(&agent.role)));
        obj.insert("role_label".into(), json!(role_label(&agent.role)));
        obj.insert("work".into(), json!(app.work.current(agent.cid)));
        obj.insert("queue".into(), json!(app.work.queued(agent.cid).len()));
    }
    v
}

/// The agent named by `{cid}`, or the 400/404 response.
pub(crate) fn agent_at(app: &App, params: &Params<'_>) -> Result<Agent, Response> {
    let cid = params
        .get("cid")
        .parse::<u64>()
        .map_err(|_| Response::error(400, "cid must be a number"))?;
    app.agents
        .get(cid)
        .ok_or_else(|| Response::error(404, "no such agent"))
}

/// The request body as `T`, or a 400 response.
pub(crate) fn body<T: DeserializeOwned>(req: &Request) -> Result<T, Response> {
    req.json::<T>()
        .map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

#[derive(Deserialize)]
struct SpawnBody {
    role: String,
    count: Option<u32>,
    model: Option<String>,
    /// The node to run on (`n0`, the hub's own machine, when absent).
    node: Option<String>,
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

#[derive(Deserialize)]
struct AssignBody {
    task_id: String,
}

/// Every agent's API view, in roster order.
pub(crate) fn list_views(app: &App) -> Vec<Value> {
    app.agents.list().iter().map(|a| view(app, a)).collect()
}

fn list(app: &App) -> Response {
    Response::json(200, &list_views(app))
}

/// Spawns `count` runs of `role` via noded. `model` is validated against the
/// tier policy and applied at each spawned agent's first `hello`; an unknown
/// `name` field is ignored: set it with `PATCH` once connected.
fn spawn(app: &App, req: &Request) -> Response {
    let b: SpawnBody = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    match spawn_agents(
        app,
        &b.role,
        b.count.unwrap_or(1),
        b.model.as_deref(),
        b.node.as_deref(),
    ) {
        Ok(value) => Response::json(200, &value),
        Err(e) if e.starts_with("noded") => Response::error(502, &e),
        Err(e) => Response::error(400, &e),
    }
}

/// Spawns `count` agents of `role` on `node` (default: the best node for the role, see
/// `placement::choose_node`): checks the role, the count, the model's tier and that the node takes
/// new agents, asks its noded for the runs and records them.
/// Returns noded's reply; errors are the message for the caller.
pub fn spawn_agents(
    app: &App,
    role: &str,
    count: u32,
    model: Option<&str>,
    node: Option<&str>,
) -> Result<Value, String> {
    let node = match node {
        Some(node) => node.to_string(),
        None => crate::placement::choose_node(app, role),
    };
    let node = node.as_str();
    if node != crate::fleet::LOCAL {
        match app.nodes.get(node) {
            None => return Err(format!("no node {node}")),
            Some(record) if record.revoked => return Err(format!("node {node} is revoked")),
            Some(record) if record.draining => return Err(format!("node {node} is draining")),
            Some(_) => {}
        }
    }
    if !crate::roles_custom::exists(app, role) {
        return Err(format!("unknown role {role}"));
    }
    if !(1..=16).contains(&count) {
        return Err("count must be 1..=16".into());
    }
    if let Some(model) = model {
        app.settings.allowed(tier_of(role), model)?;
    }
    // The same shape as the starting fleet (noded --agents-memory-mib defaults to 1024).
    let mut payload = json!({"n": count, "role": role, "vcpus": 1, "memory_mib": 1024});
    // The role's shared data volumes (settings `role_volumes`), mounted read-only by every agent.
    if let Some(volumes) = app
        .settings
        .get()
        .role_volumes
        .get(role)
        .filter(|v| !v.is_empty())
    {
        payload["volumes"] = json!(volumes);
    }
    match crate::fleet::noded(app, node, "POST", "/v1/runs", Some(&payload)) {
        Ok((status, value)) => {
            if status >= 300 {
                return Err(format!("noded: status {status}: {value}"));
            }
            for _ in 0..count {
                app.spawned.expect(role, model.map(str::to_string));
            }
            Ok(value)
        }
        Err(e) => Err(format!("noded: {e}")),
    }
}

/// `PATCH`: sets or clears the model (`null`), renames, pushes the new profile
/// to the agent through the hub, and publishes the change.
fn patch(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    let b: Value = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let model: Option<Option<String>> = match b.get("model") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(m)) => {
            if let Err(e) = app.settings.allowed(tier_of(&agent.role), m) {
                return Response::error(400, &e);
            }
            Some(Some(m.clone()))
        }
        Some(_) => return Response::error(400, "model must be a string or null"),
    };
    let name = match b.get("name") {
        None => None,
        Some(Value::String(n)) => match check_name(app, agent.cid, n) {
            Ok(valid) => Some(valid),
            Err(e) => return Response::error(400, &e),
        },
        Some(_) => return Response::error(400, "name must be a string"),
    };
    if let Some(m) = &model {
        if let Err(e) = app.spawned.set_model(agent.cid, m.as_deref()) {
            eprintln!("apid: model override not saved: {e}");
        }
    }
    if let Some(n) = &name {
        let _ = app.spawned.set_name(agent.cid, n);
    }
    app.agents.update(agent.cid, |a| {
        if let Some(m) = model {
            a.model_override = m;
        }
    });
    if let Some(n) = &name {
        if let Err(e) = store_name(app, agent.cid, n) {
            return Response::error(500, &e);
        }
    }
    let Some(updated) = app.agents.get(agent.cid) else {
        return Response::error(404, "no such agent");
    };
    // Best effort: the agent's next hello carries the profile anyway.
    let push = json!({"message": HostMessage::Profile { profile: profile(app, &updated) }});
    let _ = crate::fleet::send(app, updated.cid, &push);
    let v = view(app, &updated);
    app.bus.publish("agent", &v);
    Response::json(200, &v)
}

/// `DELETE`: asks the hub to stop the agent (best effort) and deletes its run.
fn delete(app: &App, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    let _ = crate::fleet::gateway(
        app,
        agent.cid,
        "POST",
        &format!("/v1/agents/{}/stop", agent.cid),
        Some(&json!({})),
    );
    if let Err(e) = app.spawned.forget(agent.cid) {
        eprintln!("apid: spawned.json not saved: {e}");
    }
    let run_status = agent.run.as_ref().and_then(|run| {
        crate::fleet::noded_for(app, agent.cid, "DELETE", &format!("/v1/runs/{run}"), None)
            .map(|(status, _)| status)
            .ok()
    });
    Response::json(200, &json!({"deleted": true, "run_status": run_status}))
}

/// Queues a chat turn for the agent. 409 when the agent is offline.
fn chat(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    if agent.kind == "model" {
        return Response::error(409, "a model agent has no chat");
    }
    if agent.status == Status::Offline {
        return Response::error(409, "agent is offline");
    }
    let b: TextBody = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    if let Err(e) = check_chat_text(&b.text) {
        return Response::error(400, &e);
    }
    if let Err(resp) = crate::sleep::awake_or_503(app, agent.cid) {
        return resp;
    }
    let thread = format!("op-{}", agent.cid);
    let work = app.work.enqueue(
        app,
        agent.cid,
        WorkKind::Chat,
        thread,
        b.text,
        Origin {
            owner: true,
            ..Origin::default()
        },
    );
    Response::json(200, &json!({"work": work}))
}

/// Claims an open plaza task for the agent and queues it as task work.
fn assign(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    if agent.kind == "model" {
        return Response::error(409, "a model agent cannot be assigned tasks");
    }
    if agent.status == Status::Offline {
        return Response::error(409, "agent is offline");
    }
    let b: AssignBody = match body(req) {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let Some(task) = app.plaza.tasks.find(&b.task_id) else {
        return Response::error(404, "no such task");
    };
    if !task.is_open() {
        return Response::error(409, "task is not open");
    }
    if let Err(e) = task.claim(&connected_agent(&agent)) {
        return Response::error(409, &e.to_string());
    }
    let id = task.id().to_string();
    let mut prompt = task.goal().to_string();
    if let Some(acceptance) = task.acceptance() {
        prompt.push_str("\n\nAcceptance: ");
        prompt.push_str(acceptance);
    }
    let origin = Origin {
        task: Some(id.clone()),
        ..Origin::default()
    };
    let work = app.work.enqueue(
        app,
        agent.cid,
        WorkKind::Task,
        format!("task-{id}"),
        prompt,
        origin,
    );
    Response::json(200, &json!({"work": work, "task": id}))
}

/// Cancels the agent's work in flight; `{"cancelled": bool}`.
fn cancel(app: &App, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    let cancelled = app.work.cancel(app, agent.cid, "cancelled by the operator");
    Response::json(200, &json!({"cancelled": cancelled}))
}

/// Proxies `POST /v1/agents/{cid}/<action>` to the hub with the same body.
fn to_hub(app: &App, req: &Request, params: &Params<'_>, action: &str) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    // A tell wakes a sleeping agent first: the gateway has no session to deliver it to otherwise.
    if action == "tell" {
        if let Err(resp) = crate::sleep::awake_or_503(app, agent.cid) {
            return resp;
        }
    }
    let body: Option<Value> = serde_json::from_slice(&req.body).ok();
    crate::fleet::respond(crate::fleet::gateway(
        app,
        agent.cid,
        "POST",
        &format!("/v1/agents/{}/{action}", agent.cid),
        body.as_ref(),
    ))
}

/// The agent's transcript after entry `after`, at most `limit` (max 1000), oldest first. With
/// `last=N` (max 1000) and no `after`, it is the newest N entries instead, still oldest first.
fn transcript(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let agent = match agent_at(app, params) {
        Ok(agent) => agent,
        Err(resp) => return resp,
    };
    if req.query_param("after").is_none() {
        if let Some(last) = req
            .query_param("last")
            .and_then(|v| v.parse::<usize>().ok())
        {
            return Response::json(200, &app.transcripts.last(agent.cid, last.min(1000)));
        }
    }
    let after = req
        .query_param("after")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200);
    Response::json(
        200,
        &app.transcripts.after(agent.cid, after, limit.min(1000)),
    )
}

impl Names {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&*self.guard()).unwrap_or_default()
    }

    /// Replaces the state with a [`Names::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let names: BTreeMap<u64, String> =
            serde_json::from_str(json).map_err(|e| format!("bad names snapshot: {e}"))?;
        let mut current = self.guard();
        self.save(&names).map_err(|e| format!("write names: {e}"))?;
        *current = names;
        Ok(())
    }

    /// Writes the map to `names.json` atomically (tmp file, then rename).
    fn save(&self, names: &BTreeMap<u64, String>) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(names).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_snapshot_round_trips() {
        let dir = std::env::temp_dir().join(format!("apid-names-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let names = Names::load(a.join("names.json"));
        names.set(3, "Ada").unwrap();

        let other = Names::load(b.join("names.json"));
        other.restore(&names.snapshot()).unwrap();
        assert_eq!(other.get(3).as_deref(), Some("Ada"));
        assert_eq!(
            Names::load(b.join("names.json")).get(3).as_deref(),
            Some("Ada")
        );
        assert!(other.restore("not json").is_err());
        assert_eq!(other.get(3).as_deref(), Some("Ada"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_source_prefers_agent_then_role_then_default() {
        assert_eq!(model_source(true, true), "agent");
        assert_eq!(model_source(true, false), "agent");
        assert_eq!(model_source(false, true), "role");
        assert_eq!(model_source(false, false), "default");
    }

    #[test]
    fn chat_text_is_one_to_twenty_thousand_characters() {
        assert!(check_chat_text("").is_err());
        assert!(check_chat_text("hi").is_ok());
        let at_limit: String = "é".repeat(MAX_CHAT_CHARS);
        assert!(check_chat_text(&at_limit).is_ok());
        let over: String = "é".repeat(MAX_CHAT_CHARS + 1);
        assert!(check_chat_text(&over).is_err());
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;

    #[test]
    fn a_session_ending_keeps_an_asleep_or_dozing_agent_and_takes_the_others_offline() {
        assert!(!ends_offline(Status::Asleep));
        assert!(!ends_offline(Status::Dozing));
        assert!(ends_offline(Status::Idle));
        assert!(ends_offline(Status::Working));
        assert!(ends_offline(Status::Offline));

        // The roster applies the same rule `gone` applies: the asleep agent stays asleep.
        let roster = Roster::with_db(None);
        let mut asleep = agent("builder");
        asleep.status = Status::Asleep;
        roster.upsert(asleep);
        roster.update(42, |slot| {
            if ends_offline(slot.status) {
                slot.status = Status::Offline;
            }
        });
        assert_eq!(roster.get(42).map(|a| a.status), Some(Status::Asleep));
    }

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
            species: String::new(),
            kind: "llm".into(),
            caps: Vec::new(),
        }
    }

    #[test]
    fn wait_until_returns_as_soon_as_a_change_makes_the_agent_idle() {
        let roster = Arc::new(Roster::with_db(None));
        let mut asleep = agent("builder");
        asleep.status = Status::Asleep;
        roster.upsert(asleep);

        let writer = Arc::clone(&roster);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            writer.update(42, |slot| slot.status = Status::Idle);
        });

        let started = std::time::Instant::now();
        let held = roster.wait_until(42, Duration::from_secs(5), |a| {
            a.is_some_and(|a| a.status == Status::Idle)
        });
        assert!(held);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn wait_until_gives_up_at_the_timeout_when_the_condition_never_holds() {
        let roster = Roster::with_db(None);
        roster.upsert(agent("builder"));
        let held = roster.wait_until(42, Duration::from_millis(50), |a| {
            a.is_some_and(|a| a.status == Status::Offline)
        });
        assert!(!held);
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("apid-agents-{tag}-{}-{nanos}", std::process::id()));
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
        std::fs::write(
            prompts.join("builder").join("prompt.md"),
            "<!-- internal -->\n\nBuild carefully.\n",
        )
        .unwrap();

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
        assert!(worker.contains("You are builder-42, the Builder agent"));
        assert!(worker.ends_with("Write the code."));
        let unnamed = Agent {
            name: String::new(),
            ..agent("qa")
        };
        let text = system_prompt(&unnamed, "");
        assert!(text.contains("You have no name yet. You are the QA agent"));
    }
}

#[cfg(test)]
mod roster_index_tests {
    use super::*;

    fn agent(cid: u64, status: Status) -> Agent {
        Agent {
            cid,
            name: format!("a{cid}"),
            role: "builder".into(),
            model_override: None,
            status,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: None,
            vm: None,
            usage: Usage::default(),
            species: String::new(),
            kind: "llm".into(),
            caps: Vec::new(),
        }
    }

    /// The index holds exactly the CIDs of the agents in each status, and `cids_with` reads it.
    fn assert_index_matches(roster: &Roster) {
        let ledger = roster.lock();
        let mut expected: BTreeMap<Status, BTreeSet<u64>> = BTreeMap::new();
        for agent in ledger.agents.values() {
            expected.entry(agent.status).or_default().insert(agent.cid);
        }
        assert_eq!(ledger.by_status, expected);
        drop(ledger);
        for status in [
            Status::Offline,
            Status::Idle,
            Status::Working,
            Status::Asleep,
            Status::Dozing,
        ] {
            let want: Vec<u64> = expected
                .get(&status)
                .map(|cids| cids.iter().copied().collect())
                .unwrap_or_default();
            assert_eq!(roster.cids_with(status), want);
        }
    }

    #[test]
    fn index_follows_every_mutation() {
        let roster = Roster::with_db(None);
        let statuses = [
            Status::Offline,
            Status::Idle,
            Status::Working,
            Status::Asleep,
            Status::Dozing,
        ];
        let mut seed: u64 = 7;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            seed >> 33
        };
        for step in 0..600 {
            let cid = next() % 20;
            let status = statuses[(next() % statuses.len() as u64) as usize];
            match next() % 6 {
                0 => roster.upsert(agent(cid, status)),
                1 => {
                    roster.remove(cid);
                }
                2 => {
                    roster.update(cid, |a| a.status = status);
                }
                3 => roster.restore(vec![agent(cid, status)]),
                4 => {
                    roster.update(cid, |a| a.name = format!("n{step}"));
                }
                _ => {
                    roster.update_quietly(cid, |a| a.status = status);
                }
            }
            assert_index_matches(&roster);
        }
    }

    #[test]
    fn counts_and_select_read_the_index() {
        let roster = Roster::with_db(None);
        roster.restore(vec![
            agent(1, Status::Idle),
            agent(2, Status::Working),
            agent(3, Status::Idle),
        ]);
        assert_eq!(
            roster.count_by_status(),
            BTreeMap::from([(Status::Idle, 2), (Status::Working, 1)])
        );
        assert_eq!(
            roster.select(|a| (a.status == Status::Idle).then_some(a.cid)),
            vec![1, 3]
        );
        assert_eq!(
            roster.select(|a| (a.cid > 1).then(|| a.name.clone())),
            vec!["a2".to_string(), "a3".to_string()]
        );
        roster.update(1, |a| a.status = Status::Asleep);
        roster.remove(3);
        assert_eq!(
            roster.count_by_status(),
            BTreeMap::from([(Status::Asleep, 1), (Status::Working, 1)])
        );
    }
}
