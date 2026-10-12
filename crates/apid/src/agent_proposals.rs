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

use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::Agent;
use crate::roles_custom::RoleDef;
use crate::state::App;

pub const TOOLS: &[&str] = &["plaza_agent_propose"];

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
}

impl Store {
    pub fn list(&self) -> Vec<AgentProposal> {
        self.items.lock().map(|i| i.clone()).unwrap_or_default()
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
pub fn run(_app: &App, _agent: &Agent, _input: &Value) -> Result<String, String> {
    Err("contract: B4".into())
}

/// Saves the role when new and spawns `count` agents of it; returns the spawned cids it can see.
pub fn approve(_app: &App, _id: &str, _edited: Option<RoleDef>, _count: Option<u32>, _model: Option<String>) -> Result<AgentProposal, String> {
    Err("contract: B4".into())
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/agent-proposals", |app, _, _| Response::json(200, &app.agent_proposals.list()))
        .post("/v1/agent-proposals/{id}/approve", approve_route)
        .post("/v1/agent-proposals/{id}/reject", reject_route)
}

fn approve_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B4")
}

fn reject_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B4")
}
