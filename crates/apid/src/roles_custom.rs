//! Roles the owner defines (the spawn wizard) or approves (an orchestrator's proposal), beside
//! the built-in roles from the manifest. A role is what an agent is for: its purpose,
//! responsibilities, what it delivers, how it works with others, its tools and its budget.
//! The agent's prompt is written from those fields (and may be edited). Saved in Postgres
//! ("custom_roles").
//!
//! GET    /v1/roles        → [RoleView] — built-in and custom, with their definitions
//! POST   /v1/roles        body RoleDef → the saved RoleDef (create or update a custom role)
//! DELETE /v1/roles/{id}   → refused while agents of that role run

use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};

use crate::state::App;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDef {
    /// a-z, 0-9 and '-', 2..=32 characters; not a built-in role's id.
    pub id: String,
    pub title: String,
    /// One or two sentences: what this agent is for.
    pub purpose: String,
    pub responsibilities: Vec<String>,
    /// What it hands in when its work is done.
    pub deliverables: Vec<String>,
    /// Who it works with and how (who it takes work from, who reviews it).
    #[serde(default)]
    pub collaboration: String,
    /// What it must not do, beyond the plaza's laws.
    #[serde(default)]
    pub boundaries: Vec<String>,
    /// Tool names granted beyond what every agent gets (the laws still decide).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Default task budget for its work, credits.
    #[serde(default)]
    pub budget: u64,
    /// Its default model (`provider/model[@effort]`); empty = the plaza default.
    #[serde(default)]
    pub model: String,
    /// The role prompt (markdown). Empty → written from the fields above.
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_ms: u64,
}

/// A role as GET /v1/roles shows it.
#[derive(Debug, Clone, Serialize)]
pub struct RoleView {
    pub id: String,
    pub title: String,
    /// "builtin" | "custom".
    pub source: String,
    /// The definition (custom roles; built-in roles get purpose from their prompt's first lines).
    pub def: RoleDef,
    /// How many agents of this role are connected.
    pub agents: u32,
}

#[derive(Default)]
pub struct Store {
    roles: Mutex<Vec<RoleDef>>,
}

impl Store {
    pub fn list(&self) -> Vec<RoleDef> {
        self.roles.lock().map(|r| r.clone()).unwrap_or_default()
    }
    pub fn get(&self, id: &str) -> Option<RoleDef> {
        self.list().into_iter().find(|r| r.id == id)
    }
    /// Validates and saves (create or replace by id).
    pub fn save(&self, _def: RoleDef) -> Result<RoleDef, String> {
        Err("contract: B3".into())
    }
    pub fn remove(&self, _id: &str) -> Result<(), String> {
        Err("contract: B3".into())
    }
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&self.list()).unwrap_or_default()
    }
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let roles: Vec<RoleDef> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.roles.lock().map_err(|e| e.to_string())? = roles;
        Ok(())
    }
}

/// The role prompt written from a definition's fields (markdown, with the same sections the
/// built-in role prompts use: Purpose, Responsibilities, Deliverables, Working with others,
/// Boundaries).
pub fn prompt_of(_def: &RoleDef) -> String {
    String::new() // contract: B3
}

/// Whether `role` exists (built-in or custom).
pub fn exists(app: &App, role: &str) -> bool {
    app.roles.tools.contains_key(role) || app.custom_roles.get(role).is_some()
}

/// The tools a role is granted (built-in manifest, else its definition).
pub fn tools_of(app: &App, role: &str) -> Vec<String> {
    match app.roles.tools.get(role) {
        Some(tools) => tools.clone(),
        None => app.custom_roles.get(role).map(|r| r.tools).unwrap_or_default(),
    }
}

/// The role's prompt (built-in prompt file, else the definition's prompt or one written from it).
pub fn prompt_for(app: &App, role: &str) -> String {
    if let Some(prompt) = app.roles.prompts.get(role) {
        return prompt.clone();
    }
    match app.custom_roles.get(role) {
        Some(def) if !def.prompt.trim().is_empty() => def.prompt,
        Some(def) => prompt_of(&def),
        None => String::new(),
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/roles", list_route)
        .post("/v1/roles", save_route)
        .delete("/v1/roles/{id}", remove_route)
}

fn list_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B3")
}

fn save_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B3")
}

fn remove_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B3")
}
