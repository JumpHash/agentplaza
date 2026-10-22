//! Scopes on the hub: the tree the consoles show, the entities under a scope, and which series of a metric fall
//! under it. Built from the roster (agents, roles, placement), the nodes and their runs and VMs (the latest
//! reports), workstations, sandboxes, the providers and their models.
//!
//! Scope ids: `global`, `node:<id>`, `run:<id>`, `vm:<run>/<index>`, `ws:<id>`, `sb:<id>`, `agent:<cid>`,
//! `role:<role>`, `provider:<id>`, `model:<provider>/<model>`. Series entities: a node's id (`n1`), a run's id, a
//! VM's `<run>/<index>`, an agent's cid, a volume's `<node>:<mount>`, a service's `<node>:<name>`, a tool's or a
//! route's name.

use std::collections::{BTreeSet, HashMap};

use apz_metrics::{EntityKind, SeriesKey};
use apz_metrics_view::source::{AgentInfo, Entity};
use apz_metrics_view::{ScopeNode, Status};
use serde_json::Value;

use crate::agents::{self, Status as AgentStatus};
use crate::fleet;
use crate::state::App;
use crate::usage::provider_of;

/// The tree group of the nodes and what sits under them.
const NODES: &str = "Nodes";
/// The tree group of the agents, one role at a time.
const AGENTS: &str = "Agents · by role";
/// The tree group of the providers and their models.
const PROVIDERS: &str = "Providers";
/// A node not heard from for this long is down.
const SILENT_MS: u64 = 60_000;
/// A volume or an agent's context past this share is full.
const FULL_PCT: f64 = 90.0;

/// The latest raw point of each series, as the metrics system keeps them.
type Latest = HashMap<SeriesKey, (u64, f64)>;

/// The scope tree in order (Global; Nodes with their runs, VMs, workstations and sandboxes; Agents by role;
/// Providers with their models), with statuses (a node offline or a volume over 90 % is crit; an agent over 90 %
/// of its window is warn; an asleep VM is mute).
pub fn tree(app: &App) -> Vec<ScopeNode> {
    let now = apz_plaza::now_ms();
    let facts = Facts::gather(app);
    let latest = super::lock(&app.metrics.latest);
    let mut out = vec![entry(
        "global".into(),
        "Global".into(),
        "global",
        "",
        0,
        "",
        None,
    )];

    for node in &facts.nodes {
        let status = node_status(app, &latest, node, now);
        out.push(entry(
            format!("node:{node}"),
            label(app, EntityKind::Node, node),
            "node",
            NODES,
            1,
            "node",
            Some(status),
        ));
        for run in facts.runs.iter().filter(|run| run.node == *node) {
            out.push(entry(
                format!("run:{}", run.id),
                label(app, EntityKind::Run, &run.id),
                "run",
                NODES,
                2,
                "run",
                None,
            ));
            for vm in &run.vms {
                let entity = format!("{}/{}", run.id, vm.index);
                let status = if vm.state == "asleep" {
                    Status::Mute
                } else {
                    Status::Ok
                };
                out.push(entry(
                    format!("vm:{entity}"),
                    label(app, EntityKind::Vm, &entity),
                    "vm",
                    NODES,
                    3,
                    "vm",
                    Some(status),
                ));
            }
        }
        for sandbox in facts.sandboxes.iter().filter(|sb| sb.node == *node) {
            out.push(sandbox_entry(app, sandbox, 2));
        }
    }
    // Sandboxes whose run the hub does not place on any node sit at the top of the group.
    for sandbox in facts.sandboxes.iter().filter(|sb| sb.node.is_empty()) {
        out.push(sandbox_entry(app, sandbox, 1));
    }
    for (id, _) in &facts.stations {
        out.push(entry(
            format!("ws:{id}"),
            label(app, EntityKind::Workstation, id),
            "workstation",
            NODES,
            1,
            "workstation",
            None,
        ));
    }

    let roles: BTreeSet<&str> = facts.agents.iter().map(|a| a.role.as_str()).collect();
    for role in roles {
        out.push(entry(
            format!("role:{role}"),
            role.to_string(),
            "role",
            AGENTS,
            1,
            "group",
            None,
        ));
        for agent in facts.agents.iter().filter(|a| a.role == role) {
            out.push(entry(
                format!("agent:{}", agent.cid),
                label(app, EntityKind::Agent, &agent.cid.to_string()),
                "agent",
                AGENTS,
                2,
                "",
                Some(agent_status(agent, &latest)),
            ));
        }
    }

    let models = models(app, &facts);
    for provider in app.providers.gateway().providers() {
        out.push(entry(
            format!("provider:{}", provider.id),
            provider.id.clone(),
            "provider",
            PROVIDERS,
            1,
            "group",
            None,
        ));
        for (p, model) in models.iter().filter(|(p, _)| *p == provider.id) {
            let entity = format!("{p}/{model}");
            out.push(entry(
                format!("model:{entity}"),
                label(app, EntityKind::Model, &entity),
                "model",
                PROVIDERS,
                2,
                "",
                None,
            ));
        }
    }
    out
}

/// The entities of `kind` under `scope` (kinds: node, run, vm, agent, role, provider, model, volume, service, tool,
/// route), with labels and, for VMs and agents, their state.
pub fn entities(app: &App, scope: &str, kind: &str) -> Vec<Entity> {
    let Some(kind) = kind_of(kind) else {
        return Vec::new();
    };
    let facts = Facts::gather(app);
    let scope = parse_scope(scope);
    let latest = super::lock(&app.metrics.latest);
    candidates(app, &facts, &latest, kind)
        .into_iter()
        .filter(|c| covers(&facts, &scope, kind, &c.entity, &c.labels))
        .map(|c| Entity {
            id: scope_id(kind, &c.entity),
            label: label(app, kind, &c.entity),
            state: c.state,
        })
        .collect()
}

/// What the view needs of an agent.
pub fn agent(app: &App, cid: u64) -> Option<AgentInfo> {
    let agent = app.agents.get(cid)?;
    let model = agents::model_of(app, &agent);
    Some(AgentInfo {
        cid,
        name: agent.label(),
        role: agent.role.clone(),
        run: agent.run.clone().unwrap_or_default(),
        node: fleet::node_of(app, cid),
        provider: provider_of(&model).to_string(),
        model: model_name(&model).to_string(),
        window: crate::providers::context_window(app, &model),
        status: status_word(agent.status).to_string(),
    })
}

/// A filter for the series of a metric keyed by `kind` that fall under `scope`. Built once per read (it resolves
/// the scope's members up front), then called per series.
pub fn filter(
    app: &App,
    scope: &str,
    kind: apz_metrics::EntityKind,
) -> Box<dyn Fn(&SeriesKey) -> bool + Send> {
    let facts = Facts::gather(app);
    let scope = parse_scope(scope);
    Box::new(move |key: &SeriesKey| {
        key.kind == kind && covers(&facts, &scope, key.kind, &key.entity, &key.labels)
    })
}

/// The group a series falls in for an agent property (`role`, `run`, `node`, `engine`), with that group's scope id.
pub fn agent_prop(app: &App, key: &SeriesKey, prop: &str) -> Option<(String, Option<String>)> {
    if prop == "engine" {
        let engine = key
            .labels
            .iter()
            .find(|(name, _)| name.as_str() == "engine")
            .map_or("plaza harness", |(_, value)| value.as_str());
        return Some((engine.to_string(), None));
    }
    let cid: u64 = key.entity.parse().ok()?;
    let agent = app.agents.get(cid)?;
    match prop {
        "role" => Some((agent.role.clone(), Some(format!("role:{}", agent.role)))),
        "run" => match agent.run.filter(|run| !run.is_empty()) {
            Some(run) => Some((
                label(app, EntityKind::Run, &run),
                Some(format!("run:{run}")),
            )),
            None => Some(("not placed".to_string(), None)),
        },
        "node" => {
            let node = fleet::node_of(app, cid);
            Some((
                label(app, EntityKind::Node, &node),
                Some(format!("node:{node}")),
            ))
        }
        _ => None,
    }
}

/// A label for an entity of a kind ("n1 · rack-a", "ada (4417)", "VM r12/3").
pub fn label(app: &App, kind: apz_metrics::EntityKind, entity: &str) -> String {
    match kind {
        EntityKind::Node => match app.nodes.get(entity) {
            Some(node) => {
                let host = node
                    .machine
                    .host
                    .as_deref()
                    .filter(|host| !host.is_empty())
                    .unwrap_or(node.name.as_str());
                format!("{entity} · {host}")
            }
            None if entity == fleet::LOCAL => format!("{entity} · hub"),
            None => entity.to_string(),
        },
        EntityKind::Run => format!("run {entity}"),
        EntityKind::Vm => format!("VM {entity}"),
        EntityKind::Agent => match entity
            .parse::<u64>()
            .ok()
            .and_then(|cid| app.agents.get(cid).map(|a| (cid, a)))
        {
            Some((cid, agent)) => agent_label(&agent.role, cid),
            None => entity.to_string(),
        },
        EntityKind::Model => model_name(entity).to_string(),
        EntityKind::Volume | EntityKind::Service => match entity.split_once(':') {
            Some((node, rest)) if node == fleet::LOCAL || app.nodes.get(node).is_some() => {
                format!("{node} · {rest}")
            }
            // The hub's own stores ("store:postgres · plaza") are named by what follows.
            Some(("store", rest)) => rest.to_string(),
            _ => entity.to_string(),
        },
        EntityKind::Workstation => app
            .stations
            .list()
            .into_iter()
            .find(|station| station.id == entity)
            .map_or_else(|| entity.to_string(), |station| station.name),
        EntityKind::Sandbox => format!("sandbox {entity}"),
        _ => entity.to_string(),
    }
}

/// The node a volume or service lives on: the node its id starts with (`n1:/var/lib/plaza`), else the hub's own
/// node, which runs the plaza's stores, the metrics module and the MCP clients (`store:postgres · plaza`,
/// `metrics`, `MCP: linear`).
fn owner_node<'a>(nodes: &[String], entity: &'a str) -> &'a str {
    match entity.split_once(':') {
        Some((owner, _)) if nodes.iter().any(|n| n == owner) => owner,
        _ => fleet::LOCAL,
    }
}

/// The hub's facts a scope needs, read once per call: the nodes, their live runs and VMs, the agents with where
/// they are placed, the sandboxes and the workstations.
#[derive(Default)]
struct Facts {
    /// Every node the hub knows: `n0` first, then the joined nodes that are not revoked.
    nodes: Vec<String>,
    /// The runs that have not ended, with the node that hosts them.
    runs: Vec<RunFact>,
    agents: Vec<AgentFact>,
    sandboxes: Vec<SandboxFact>,
    /// (id, name) of each workstation.
    stations: Vec<(String, String)>,
}

struct RunFact {
    node: String,
    id: String,
    vms: Vec<VmFact>,
}

struct VmFact {
    index: u32,
    state: String,
}

struct AgentFact {
    cid: u64,
    role: String,
    status: AgentStatus,
    run: String,
    node: String,
    provider: String,
    model: String,
}

struct SandboxFact {
    id: String,
    /// The node hosting the sandbox's run, or empty when no node reports that run.
    node: String,
}

impl Facts {
    fn gather(app: &App) -> Facts {
        let mut facts = Facts::default();
        let mut reports = vec![(fleet::LOCAL.to_string(), local_runs(app))];
        facts.nodes.push(fleet::LOCAL.to_string());
        for node in app.nodes.list().into_iter().filter(|node| !node.revoked) {
            if let Some((_, runs)) = app.nodes.report(&node.id) {
                reports.push((node.id.clone(), runs));
            }
            facts.nodes.push(node.id);
        }
        for (node, runs) in reports {
            for run in runs["runs"].as_array().into_iter().flatten() {
                if !run["ended_at"].is_null() {
                    continue;
                }
                let Some(id) = run["id"].as_str() else {
                    continue;
                };
                let vms = run["vms"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|vm| {
                        Some(VmFact {
                            index: vm["id"].as_u64()? as u32,
                            state: vm["state"].as_str().unwrap_or_default().to_string(),
                        })
                    })
                    .collect();
                facts.runs.push(RunFact {
                    node: node.clone(),
                    id: id.to_string(),
                    vms,
                });
            }
        }
        for agent in app.agents.list() {
            let model = agents::model_of(app, &agent);
            facts.agents.push(AgentFact {
                cid: agent.cid,
                role: agent.role.clone(),
                status: agent.status,
                run: agent.run.clone().unwrap_or_default(),
                node: fleet::node_of(app, agent.cid),
                provider: provider_of(&model).to_string(),
                model: model_name(&model).to_string(),
            });
        }
        for sandbox in crate::e2b::sandboxes(app).list() {
            let node = facts
                .runs
                .iter()
                .find(|run| run.id == sandbox.run)
                .map_or_else(String::new, |run| run.node.clone());
            facts.sandboxes.push(SandboxFact {
                id: sandbox.id,
                node,
            });
        }
        facts.stations = app
            .stations
            .list()
            .into_iter()
            .map(|station| (station.id, station.name))
            .collect();
        facts
    }
}

/// A scope id, parsed.
#[derive(Debug, PartialEq)]
enum Scope {
    Global,
    Node(String),
    Run(String),
    Vm(String),
    Workstation(String),
    Sandbox(String),
    Agent(String),
    Role(String),
    Provider(String),
    Model {
        provider: String,
        model: String,
    },
    Volume(String),
    Service(String),
    Tool(String),
    Route(String),
    /// An id this module does not resolve (a malformed or unknown prefix).
    Other,
}

fn parse_scope(scope: &str) -> Scope {
    if scope == "global" {
        return Scope::Global;
    }
    let Some((prefix, id)) = scope.split_once(':') else {
        return Scope::Other;
    };
    match prefix {
        "node" => Scope::Node(id.to_string()),
        "run" => Scope::Run(id.to_string()),
        "vm" => Scope::Vm(id.to_string()),
        "ws" => Scope::Workstation(id.to_string()),
        "sb" => Scope::Sandbox(id.to_string()),
        "agent" => Scope::Agent(id.to_string()),
        "role" => Scope::Role(id.to_string()),
        "provider" => Scope::Provider(id.to_string()),
        "volume" => Scope::Volume(id.to_string()),
        "service" => Scope::Service(id.to_string()),
        "tool" => Scope::Tool(id.to_string()),
        "route" => Scope::Route(id.to_string()),
        "model" => match id.split_once('/') {
            Some((provider, model)) => Scope::Model {
                provider: provider.to_string(),
                model: model.to_string(),
            },
            None => Scope::Other,
        },
        _ => Scope::Other,
    }
}

/// Whether an entity of `kind` (with its series labels, if any) falls under `scope`. Pure over the facts, so the
/// filter and the entity lists agree. A combination that does not apply covers nothing.
fn covers(
    facts: &Facts,
    scope: &Scope,
    kind: EntityKind,
    entity: &str,
    labels: &[(String, String)],
) -> bool {
    let label_of = |name: &str| {
        labels
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, value)| value.as_str())
    };
    match scope {
        Scope::Global => true,
        Scope::Provider(p) => match kind {
            EntityKind::Provider => entity == p.as_str(),
            _ => label_of("provider") == Some(p.as_str()),
        },
        Scope::Model { provider, model } => match kind {
            EntityKind::Model => {
                entity.split_once('/') == Some((provider.as_str(), model.as_str()))
            }
            _ => {
                label_of("provider") == Some(provider.as_str())
                    && label_of("model") == Some(model.as_str())
            }
        },
        Scope::Role(role) => match kind {
            EntityKind::Role => entity == role.as_str(),
            EntityKind::Agent => agent_of(facts, entity).is_some_and(|a| a.role == *role),
            _ => false,
        },
        Scope::Node(node) => match kind {
            EntityKind::Node => entity == node.as_str(),
            EntityKind::Run => facts
                .runs
                .iter()
                .any(|run| run.node == *node && run.id == entity),
            EntityKind::Vm => facts
                .runs
                .iter()
                .any(|run| run.node == *node && run_of_vm(entity) == Some(run.id.as_str())),
            EntityKind::Agent => agent_of(facts, entity).is_some_and(|a| a.node == *node),
            EntityKind::Sandbox => facts
                .sandboxes
                .iter()
                .any(|sb| sb.node == *node && sb.id == entity),
            EntityKind::Volume | EntityKind::Service => {
                owner_node(&facts.nodes, entity) == node.as_str()
            }
            _ => false,
        },
        Scope::Run(run) => match kind {
            EntityKind::Run => entity == run.as_str(),
            EntityKind::Vm => run_of_vm(entity) == Some(run.as_str()),
            EntityKind::Agent => agent_of(facts, entity).is_some_and(|a| a.run == *run),
            _ => false,
        },
        Scope::Vm(vm) => kind == EntityKind::Vm && entity == vm.as_str(),
        Scope::Agent(cid) => kind == EntityKind::Agent && entity == cid.as_str(),
        Scope::Workstation(id) => kind == EntityKind::Workstation && entity == id.as_str(),
        Scope::Sandbox(id) => kind == EntityKind::Sandbox && entity == id.as_str(),
        Scope::Volume(id) => kind == EntityKind::Volume && entity == id.as_str(),
        Scope::Service(id) => kind == EntityKind::Service && entity == id.as_str(),
        Scope::Tool(id) => kind == EntityKind::Tool && entity == id.as_str(),
        Scope::Route(id) => kind == EntityKind::Route && entity == id.as_str(),
        Scope::Other => false,
    }
}

/// The members of `kind` before any scope is applied: the entity, its series labels, and its state.
struct Candidate {
    entity: String,
    labels: Vec<(String, String)>,
    state: String,
}

fn candidates(app: &App, facts: &Facts, latest: &Latest, kind: EntityKind) -> Vec<Candidate> {
    let plain = |entity: String| Candidate {
        entity,
        labels: Vec::new(),
        state: String::new(),
    };
    match kind {
        EntityKind::Node => facts.nodes.iter().cloned().map(plain).collect(),
        EntityKind::Run => facts.runs.iter().map(|run| plain(run.id.clone())).collect(),
        EntityKind::Vm => facts
            .runs
            .iter()
            .flat_map(|run| {
                run.vms.iter().map(move |vm| Candidate {
                    entity: format!("{}/{}", run.id, vm.index),
                    labels: Vec::new(),
                    state: vm.state.clone(),
                })
            })
            .collect(),
        EntityKind::Agent => facts
            .agents
            .iter()
            .map(|agent| Candidate {
                entity: agent.cid.to_string(),
                labels: vec![
                    ("model".to_string(), agent.model.clone()),
                    ("provider".to_string(), agent.provider.clone()),
                ],
                state: status_word(agent.status).to_string(),
            })
            .collect(),
        EntityKind::Role => facts
            .agents
            .iter()
            .map(|agent| agent.role.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(plain)
            .collect(),
        EntityKind::Provider => app
            .providers
            .gateway()
            .providers()
            .into_iter()
            .map(|provider| plain(provider.id))
            .collect(),
        EntityKind::Model => models(app, facts)
            .into_iter()
            .map(|(provider, model)| Candidate {
                entity: format!("{provider}/{model}"),
                labels: vec![
                    ("model".to_string(), model),
                    ("provider".to_string(), provider),
                ],
                state: String::new(),
            })
            .collect(),
        EntityKind::Volume | EntityKind::Service | EntityKind::Tool | EntityKind::Route => {
            latest_entities(latest, kind)
                .into_iter()
                .map(plain)
                .collect()
        }
        EntityKind::Workstation => facts
            .stations
            .iter()
            .map(|(id, _)| plain(id.clone()))
            .collect(),
        EntityKind::Sandbox => facts
            .sandboxes
            .iter()
            .map(|sb| plain(sb.id.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// The (provider, model name) pairs: the models each provider offers, and the models agents use.
fn models(app: &App, facts: &Facts) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for provider in app.providers.gateway().providers() {
        for model in &provider.models {
            let name = model
                .strip_prefix(&format!("{}/", provider.id))
                .unwrap_or(model.as_str());
            out.insert((provider.id.clone(), name.to_string()));
        }
    }
    for agent in &facts.agents {
        out.insert((agent.provider.clone(), agent.model.clone()));
    }
    out
}

/// The kind named by the view (`node`, `vm`, `workstation` …), or by its scope prefix.
fn kind_of(kind: &str) -> Option<EntityKind> {
    match kind {
        "workstation" => Some(EntityKind::Workstation),
        "sandbox" => Some(EntityKind::Sandbox),
        _ => EntityKind::from_prefix(kind),
    }
}

/// The scope id of an entity of a kind: the kind's prefix and the entity (`volume:n1:/var/lib/plaza`, `tool:bash`).
fn scope_id(kind: EntityKind, entity: &str) -> String {
    format!("{}:{entity}", kind.prefix())
}

/// The distinct entities of a kind that the metrics system has a latest point for.
fn latest_entities(latest: &Latest, kind: EntityKind) -> Vec<String> {
    latest
        .keys()
        .filter(|key| key.kind == kind)
        .map(|key| key.entity.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The latest values of a metric over the entities of a kind.
fn latest_of<'a>(latest: &'a Latest, metric: &str, kind: EntityKind) -> Vec<(&'a str, f64)> {
    latest
        .iter()
        .filter(|(key, _)| key.metric == metric && key.kind == kind)
        .map(|(key, (_, value))| (key.entity.as_str(), *value))
        .collect()
}

fn node_status(app: &App, latest: &Latest, node: &str, now: u64) -> Status {
    let silent = match app.nodes.get(node) {
        Some(record) => now.saturating_sub(record.last_seen_ms) >= SILENT_MS,
        None => node != fleet::LOCAL,
    };
    let full = latest_of(latest, "volume.used_pct", EntityKind::Volume)
        .iter()
        .any(|(entity, pct)| {
            *pct > FULL_PCT
                && entity
                    .split_once(':')
                    .is_some_and(|(owner, _)| owner == node)
        });
    if silent || full {
        Status::Crit
    } else {
        Status::Ok
    }
}

fn agent_status(agent: &AgentFact, latest: &Latest) -> Status {
    // Asleep and dozing (paused, paged out) agents are quiet, not unhealthy.
    if matches!(agent.status, AgentStatus::Asleep | AgentStatus::Dozing) {
        return Status::Mute;
    }
    let cid = agent.cid.to_string();
    let full = latest_of(latest, "ctx.fill_pct", EntityKind::Agent)
        .iter()
        .any(|(entity, pct)| *entity == cid && *pct > FULL_PCT);
    if full { Status::Warn } else { Status::Ok }
}

/// A tree entry for a sandbox.
fn sandbox_entry(app: &App, sandbox: &SandboxFact, depth: u8) -> ScopeNode {
    entry(
        format!("sb:{}", sandbox.id),
        label(app, EntityKind::Sandbox, &sandbox.id),
        "sandbox",
        NODES,
        depth,
        "sandbox",
        None,
    )
}

fn entry(
    id: String,
    label: String,
    kind: &str,
    group: &str,
    depth: u8,
    tag: &str,
    status: Option<Status>,
) -> ScopeNode {
    ScopeNode {
        id,
        label,
        kind: kind.to_string(),
        group: group.to_string(),
        depth,
        tag: tag.to_string(),
        status,
    }
}

/// The live runs of the hub's own node, from its noded (`Null` when noded does not answer).
fn local_runs(app: &App) -> Value {
    app.noded
        .get("/v1/runs")
        .ok()
        .filter(|(code, _)| *code == 200)
        .map_or(Value::Null, |(_, body)| body)
}

/// The agent with this entity (a cid), if the hub has it.
fn agent_of<'a>(facts: &'a Facts, entity: &str) -> Option<&'a AgentFact> {
    let cid: u64 = entity.parse().ok()?;
    facts.agents.iter().find(|agent| agent.cid == cid)
}

/// The run of a VM entity (`r12/3` is `r12`).
fn run_of_vm(entity: &str) -> Option<&str> {
    entity.split_once('/').map(|(run, _)| run)
}

/// The model name of a model id: the part after the provider (`anthropic/opus-5.5` is `opus-5.5`).
fn model_name(model: &str) -> &str {
    model.split_once('/').map_or(model, |(_, name)| name)
}

fn agent_label(name: &str, cid: u64) -> String {
    let name = if name.is_empty() { "agent" } else { name };
    format!("{name} ({cid})")
}

fn status_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Offline => "offline",
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Asleep => "asleep",
        AgentStatus::Dozing => "dozing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hubs_stores_and_clients_live_on_its_own_node() {
        let nodes = vec!["n0".to_string(), "n1".to_string()];
        assert_eq!(owner_node(&nodes, "n1:/var/lib/plaza"), "n1");
        assert_eq!(owner_node(&nodes, "store:postgres · plaza"), fleet::LOCAL);
        assert_eq!(owner_node(&nodes, "MCP: linear"), fleet::LOCAL);
        assert_eq!(owner_node(&nodes, "metrics"), fleet::LOCAL);
    }

    fn agent(cid: u64, node: &str, run: &str, provider: &str, model: &str) -> AgentFact {
        AgentFact {
            cid,
            role: "builder".to_string(),
            status: AgentStatus::Idle,
            run: run.to_string(),
            node: node.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn a_node_scope_keeps_only_the_series_of_its_agents() {
        let facts = Facts {
            agents: vec![
                agent(4417, "n1", "r12", "anthropic", "opus-5.5"),
                agent(9, "n2", "r3", "anthropic", "opus-5.5"),
            ],
            ..Facts::default()
        };
        let scope = parse_scope("node:n1");
        let keeps = |cid: &str| {
            let key = SeriesKey::new("tokens_in", EntityKind::Agent, cid);
            covers(&facts, &scope, key.kind, &key.entity, &key.labels)
        };
        assert!(keeps("4417"));
        assert!(!keeps("9"));
    }

    #[test]
    fn a_model_scope_keeps_the_series_labelled_with_its_provider_and_model() {
        let facts = Facts::default();
        let scope = parse_scope("model:anthropic/opus-5.5");
        let keeps = |provider: &str, model: &str| {
            let key = SeriesKey::new("tokens_in", EntityKind::Agent, "4417")
                .label("provider", provider)
                .label("model", model);
            covers(&facts, &scope, key.kind, &key.entity, &key.labels)
        };
        assert!(keeps("anthropic", "opus-5.5"));
        assert!(!keeps("anthropic", "haiku-5.5"));
        assert!(!keeps("openrouter", "opus-5.5"));
    }

    #[test]
    fn a_tool_scope_keeps_only_the_series_of_that_tool() {
        let facts = Facts::default();
        let scope = parse_scope("tool:run");
        let keeps = |kind: EntityKind, entity: &str| {
            let key = SeriesKey::new("tool.calls", kind, entity);
            covers(&facts, &scope, key.kind, &key.entity, &key.labels)
        };
        assert!(keeps(EntityKind::Tool, "run"));
        assert!(!keeps(EntityKind::Tool, "edit"));
        assert!(!keeps(EntityKind::Route, "run"));
    }
}
