//! The orchestrator proposes new agents — only when the owner told it to — and the owner
//! approves. `plaza_agent_propose` is offered to the orchestrator (law T-012) and refused unless
//! the work it runs in is the owner's own chat. A proposal holds a full role definition (or an
//! existing role), how many agents, and why; it waits in the console's Approvals until the owner
//! approves (optionally after editing it in the agent wizard) or rejects it. Approval saves the
//! role (when new) and spawns the agents. Saved in Postgres ("agent_proposals").
//!
//! GET  /v1/agent-proposals                    → [AgentProposal], newest first
//! POST /v1/agent-proposals/{id}/approve  body {role?: RoleDef, count?, model?} → the proposal (spawned)
//! POST /v1/agent-proposals/{id}/reject   body {reason}                         → the proposal

use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::{Params, Request, Response, Router};
use apz_proto::wire::WorkKind;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::{spawn_agents, Agent};
use crate::roles_custom::RoleDef;
use crate::scheduler::WorkItem;
use crate::settings::tier_of;
use crate::state::App;

pub const TOOLS: &[&str] = &["plaza_agent_propose"];

const PENDING: &str = "pending";
const APPROVED: &str = "approved";
const REJECTED: &str = "rejected";
const MAX_COUNT: u32 = 8;
const MAX_TEXT_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProposal {
    /// `ap<seq>`.
    pub id: String,
    /// The role: a new definition, or an existing role's id with the other fields empty.
    pub role: RoleDef,
    /// True when `role` defines a new role.
    pub new_role: bool,
    pub count: u32,
    #[serde(default)]
    pub model: String,
    /// Why these agents are needed.
    pub reason: String,
    /// The owner's words that asked for it (quoted from the chat).
    pub instruction: String,
    pub by_cid: u64,
    pub by: String,
    pub created_ms: u64,
    /// "pending" | "approved" | "rejected".
    pub state: String,
    #[serde(default)]
    pub decided_ms: u64,
    #[serde(default)]
    pub note: String,
    /// Agents spawned on approval (cids, when known).
    #[serde(default)]
    pub spawned: Vec<u64>,
}

#[derive(Default)]
pub struct Store {
    items: Mutex<Vec<AgentProposal>>,
    /// Serialises approve and reject, so a proposal is decided once.
    deciding: Mutex<()>,
}

impl Store {
    pub fn list(&self) -> Vec<AgentProposal> {
        self.items.lock().map(|i| i.clone()).unwrap_or_default()
    }
    pub fn get(&self, id: &str) -> Option<AgentProposal> {
        self.list().into_iter().find(|p| p.id == id)
    }
    /// Stores a new proposal under the next `ap<n>` id, newest first; returns it with the id.
    pub fn add(&self, mut p: AgentProposal) -> AgentProposal {
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        let next = items.iter().filter_map(|i| i.id.strip_prefix("ap")?.parse::<u64>().ok()).max().unwrap_or(0) + 1;
        p.id = format!("ap{next}");
        items.insert(0, p.clone());
        p
    }
    /// Changes the proposal `id` in place; returns it as changed.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut AgentProposal)) -> Option<AgentProposal> {
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        let item = items.iter_mut().find(|i| i.id == id)?;
        change(item);
        Some(item.clone())
    }
    fn deciding(&self) -> MutexGuard<'_, ()> {
        self.deciding.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&self.list()).unwrap_or_default()
    }
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let items: Vec<AgentProposal> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.items.lock().map_err(|e| e.to_string())? = items;
        Ok(())
    }
}

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_agent_propose" => (
            "Propose new agents for the owner to approve — only when the owner asked you for them in this chat. Define the role properly: id (a-z, 0-9, '-'), title, purpose, responsibilities, deliverables, how it works with the others, its boundaries, extra tools it needs, a default task budget; or give `existing_role` to add agents of a role the plaza has. Quote the owner's request in `instruction`. Nothing is spawned until the owner approves.",
            obj(json!({
                "existing_role": {"type": "string"},
                "id": {"type": "string"}, "title": {"type": "string"}, "purpose": {"type": "string"},
                "responsibilities": {"type": "array", "items": {"type": "string"}},
                "deliverables": {"type": "array", "items": {"type": "string"}},
                "collaboration": {"type": "string"},
                "boundaries": {"type": "array", "items": {"type": "string"}},
                "tools": {"type": "array", "items": {"type": "string"}},
                "budget": {"type": "integer"},
                "count": {"type": "integer", "minimum": 1, "maximum": 8},
                "model": {"type": "string"},
                "reason": {"type": "string"},
                "instruction": {"type": "string"}
            }), &["count", "reason", "instruction"]),
        ),
        _ => return None,
    })
}

/// Runs `plaza_agent_propose` for the orchestrator.
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    owner_chat(app.work.current(agent.cid).as_ref())?;
    let count = input
        .get("count")
        .and_then(Value::as_u64)
        .filter(|n| (1..=u64::from(MAX_COUNT)).contains(n))
        .ok_or_else(|| format!("count must be 1..={MAX_COUNT}"))? as u32;
    let reason = text(input, "reason");
    let instruction = text(input, "instruction");
    for (name, value) in [("reason", &reason), ("instruction", &instruction)] {
        if value.is_empty() {
            return Err(format!("{name} is required"));
        }
        if value.chars().count() > MAX_TEXT_CHARS {
            return Err(format!("{name} is over {MAX_TEXT_CHARS} characters"));
        }
    }
    let model = text(input, "model");
    let existing = text(input, "existing_role");
    let (role, new_role) = if !existing.is_empty() {
        if !crate::roles_custom::exists(app, &existing) {
            return Err(format!("unknown role {existing}"));
        }
        let title = app.custom_roles.get(&existing).map(|d| d.title).unwrap_or_else(|| existing.clone());
        (RoleDef { id: existing, title, ..RoleDef::default() }, false)
    } else {
        let def = RoleDef {
            id: text(input, "id"),
            title: text(input, "title"),
            purpose: text(input, "purpose"),
            responsibilities: list(input, "responsibilities"),
            deliverables: list(input, "deliverables"),
            collaboration: text(input, "collaboration"),
            boundaries: list(input, "boundaries"),
            tools: list(input, "tools"),
            budget: input.get("budget").and_then(Value::as_u64).unwrap_or(0),
            created_by: agent.name.clone(),
            created_ms: now_ms(),
            ..RoleDef::default()
        };
        check_role_shape(&def)?;
        if crate::roles_custom::exists(app, &def.id) {
            return Err(format!("role {} already exists; use existing_role", def.id));
        }
        (def, true)
    };
    check_model(app, &role.id, &model)?;
    let proposal = app.agent_proposals.add(AgentProposal {
        id: String::new(),
        role,
        new_role,
        count,
        model,
        reason,
        instruction,
        by_cid: agent.cid,
        by: agent.name.clone(),
        created_ms: now_ms(),
        state: PENDING.into(),
        decided_ms: 0,
        note: String::new(),
        spawned: Vec::new(),
    });
    app.bus.publish("agent_proposal", &proposal);
    Ok(format!(
        "Proposal {} is waiting for the owner's approval in the console (Approvals). Nothing is spawned until the owner approves.",
        proposal.id
    ))
}

/// Saves the role when new and spawns `count` agents of it. `edited` replaces the proposal's
/// role (saved as a new or replaced custom role); `count` and `model` override the proposal's.
pub fn approve(app: &App, id: &str, edited: Option<RoleDef>, count: Option<u32>, model: Option<String>) -> Result<AgentProposal, String> {
    let _decide = app.agent_proposals.deciding();
    let p = pending(app, id)?;
    let (role, new_role) = match edited {
        Some(role) => {
            check_role_shape(&role)?;
            if app.roles.tools.contains_key(&role.id) {
                return Err(format!("{} is a built-in role", role.id));
            }
            (role, true)
        }
        None => (p.role.clone(), p.new_role),
    };
    let count = count.unwrap_or(p.count);
    if !(1..=MAX_COUNT).contains(&count) {
        return Err(format!("count must be 1..={MAX_COUNT}"));
    }
    let model = model.unwrap_or_else(|| p.model.clone());
    check_model(app, &role.id, &model)?;
    if new_role {
        app.custom_roles.save(role.clone()).map_err(|e| format!("could not save role {}: {e}", role.id))?;
    }
    let spawned = spawn_agents(app, &role.id, count, (!model.is_empty()).then_some(model.as_str()))?;
    let cids: Vec<u64> = spawned
        .get("cids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    let decided = app
        .agent_proposals
        .update(id, |item| {
            item.role = role;
            item.new_role = new_role;
            item.count = count;
            item.model = model;
            item.state = APPROVED.into();
            item.decided_ms = now_ms();
            item.spawned = cids;
        })
        .ok_or_else(|| format!("no proposal {id}"))?;
    app.bus.publish("agent_proposal", &decided);
    Ok(decided)
}

/// Rejects a pending proposal; `reason` is kept as its note.
pub fn reject(app: &App, id: &str, reason: &str) -> Result<AgentProposal, String> {
    let _decide = app.agent_proposals.deciding();
    pending(app, id)?;
    let decided = app
        .agent_proposals
        .update(id, |item| {
            item.state = REJECTED.into();
            item.decided_ms = now_ms();
            item.note = reason.to_string();
        })
        .ok_or_else(|| format!("no proposal {id}"))?;
    app.bus.publish("agent_proposal", &decided);
    Ok(decided)
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/agent-proposals", |app, _, _| Response::json(200, &app.agent_proposals.list()))
        .post("/v1/agent-proposals/{id}/approve", approve_route)
        .post("/v1/agent-proposals/{id}/reject", reject_route)
}

fn approve_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let v: Value = req.json::<Value>().unwrap_or(Value::Null);
    let edited = match v.get("role").filter(|r| !r.is_null()) {
        Some(r) => match serde_json::from_value::<RoleDef>(r.clone()) {
            Ok(def) => Some(def),
            Err(e) => return Response::error(400, &format!("bad role: {e}")),
        },
        None => None,
    };
    let count = v.get("count").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
    let model = v.get("model").and_then(Value::as_str).map(str::to_string);
    match approve(app, params.get("id"), edited, count, model) {
        Ok(p) => Response::json(200, &p),
        Err(e) => Response::error(409, &e),
    }
}

fn reject_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let v: Value = req.json::<Value>().unwrap_or(Value::Null);
    let reason = v.get("reason").and_then(Value::as_str).unwrap_or("").trim();
    match reject(app, params.get("id"), reason) {
        Ok(p) => Response::json(200, &p),
        Err(e) => Response::error(409, &e),
    }
}

/// Refuses unless the work the agent runs now is the owner's own chat.
fn owner_chat(current: Option<&WorkItem>) -> Result<(), String> {
    match current {
        Some(item) if item.kind == WorkKind::Chat && item.origin.owner => Ok(()),
        _ => Err("you may propose agents only when the owner asked for them in your chat; ask the owner".into()),
    }
}

/// The proposal `id`, if it is still pending.
fn pending(app: &App, id: &str) -> Result<AgentProposal, String> {
    let p = app.agent_proposals.get(id).ok_or_else(|| format!("no proposal {id}"))?;
    if p.state != PENDING {
        return Err("already decided".into());
    }
    Ok(p)
}

/// The role shape the custom role store requires (the id's uniqueness is checked by the caller).
fn check_role_shape(def: &RoleDef) -> Result<(), String> {
    let id_ok = (2..=32).contains(&def.id.len()) && def.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !id_ok {
        return Err("role id must be 2-32 characters: a-z, 0-9, '-'".into());
    }
    if def.title.trim().is_empty() {
        return Err("role needs a title".into());
    }
    if def.purpose.trim().is_empty() {
        return Err("role needs a purpose".into());
    }
    if def.responsibilities.is_empty() {
        return Err("role needs at least one responsibility".into());
    }
    if def.deliverables.is_empty() {
        return Err("role needs at least one deliverable".into());
    }
    Ok(())
}

/// The model, when given, must be allowed for the role's tier.
fn check_model(app: &App, role: &str, model: &str) -> Result<(), String> {
    if model.is_empty() {
        return Ok(());
    }
    app.settings.allowed(tier_of(role), model).map(|_| ())
}

fn text(input: &Value, key: &str) -> String {
    input.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn list(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::Origin;

    fn work(kind: WorkKind, owner: bool) -> WorkItem {
        WorkItem {
            id: 1,
            cid: 42,
            kind,
            thread: "op-42".into(),
            prompt: "hi".into(),
            origin: Origin { owner, ..Origin::default() },
            created_ms: 0,
            started_ms: None,
        }
    }

    fn role() -> RoleDef {
        RoleDef {
            id: "pricing-analyst".into(),
            title: "Pricing analyst".into(),
            purpose: "Checks prices.".into(),
            responsibilities: vec!["check".into()],
            deliverables: vec!["a note".into()],
            ..RoleDef::default()
        }
    }

    fn draft(role_id: &str) -> AgentProposal {
        AgentProposal {
            id: String::new(),
            role: RoleDef { id: role_id.into(), ..RoleDef::default() },
            new_role: false,
            count: 1,
            model: String::new(),
            reason: "r".into(),
            instruction: "i".into(),
            by_cid: 42,
            by: "orchestrator-42".into(),
            created_ms: 0,
            state: PENDING.into(),
            decided_ms: 0,
            note: String::new(),
            spawned: vec![],
        }
    }

    #[test]
    fn proposals_are_refused_outside_the_owners_chat() {
        assert!(owner_chat(None).is_err());
        assert!(owner_chat(Some(&work(WorkKind::Chat, false))).is_err());
        assert!(owner_chat(Some(&work(WorkKind::Task, true))).is_err());
        assert!(owner_chat(Some(&work(WorkKind::Chat, true))).is_ok());
    }

    #[test]
    fn role_shape_follows_the_custom_role_rules() {
        assert!(check_role_shape(&role()).is_ok());
        assert!(check_role_shape(&RoleDef { id: "A".into(), ..role() }).is_err());
        assert!(check_role_shape(&RoleDef { responsibilities: vec![], ..role() }).is_err());
        assert!(check_role_shape(&RoleDef { deliverables: vec![], ..role() }).is_err());
    }

    #[test]
    fn store_adds_newest_first_and_updates_in_place() {
        let store = Store::default();
        let first = store.add(draft("a-role"));
        let second = store.add(draft("b-role"));
        assert_eq!((first.id.as_str(), second.id.as_str()), ("ap1", "ap2"));
        assert_eq!(store.list()[0].id, "ap2");
        let rejected = store.update("ap1", |p| p.state = REJECTED.into()).unwrap();
        assert_eq!(rejected.state, REJECTED);
        assert!(store.update("ap9", |_| {}).is_none());
    }
}
