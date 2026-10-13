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
use apz_plaza::tasks::MIN_TASK_BUDGET;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agents::{self, Status};
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
    pub fn save(&self, mut def: RoleDef) -> Result<RoleDef, String> {
        validate(&def)?;
        let mut roles = self.roles.lock().map_err(|e| e.to_string())?;
        match roles.iter().position(|r| r.id == def.id) {
            Some(i) => {
                def.created_by = roles[i].created_by.clone();
                def.created_ms = roles[i].created_ms;
                roles[i] = def.clone();
            }
            None => roles.push(def.clone()),
        }
        Ok(def)
    }
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut roles = self.roles.lock().map_err(|e| e.to_string())?;
        let i = roles.iter().position(|r| r.id == id).ok_or_else(|| format!("no role {id}"))?;
        roles.remove(i);
        Ok(())
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

/// The checks a definition must pass before it is saved (the built-in-id check is the route's,
/// since only the route has the app).
fn validate(def: &RoleDef) -> Result<(), String> {
    if !valid_id(&def.id) {
        return Err("role id: 2..=32 characters of a-z, 0-9 and '-', starting with a letter".into());
    }
    check_text("title", &def.title, 60)?;
    check_text("purpose", &def.purpose, 600)?;
    if def.responsibilities.is_empty() {
        return Err("a role needs at least one responsibility".into());
    }
    for item in &def.responsibilities {
        check_text("responsibility", item, 300)?;
    }
    if def.deliverables.is_empty() {
        return Err("a role needs at least one deliverable".into());
    }
    for item in &def.deliverables {
        check_text("deliverable", item, usize::MAX)?;
    }
    let min_budget = u64::try_from(MIN_TASK_BUDGET).unwrap_or(0);
    if def.budget != 0 && !(min_budget..=50_000).contains(&def.budget) {
        return Err(format!("budget: 0 (none) or {min_budget}..=50000 credits"));
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    (2..=32).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_text(what: &str, value: &str, max: usize) -> Result<(), String> {
    let n = value.trim().chars().count();
    if n == 0 {
        return Err(format!("{what} is empty"));
    }
    if n > max {
        return Err(format!("{what} is longer than {max} characters"));
    }
    Ok(())
}

/// The role prompt written from a definition's fields (markdown, with the same sections the
/// built-in role prompts use: Purpose, Responsibilities, Deliverables, Working with others,
/// Boundaries).
pub fn prompt_of(def: &RoleDef) -> String {
    let mut out = format!("# {}\n\n## Purpose\n{}\n\n", def.title.trim(), def.purpose.trim());
    out.push_str("## Responsibilities\n");
    for item in &def.responsibilities {
        out.push_str(&format!("- {}\n", item.trim()));
    }
    out.push_str("\n## Deliverables\n");
    for item in &def.deliverables {
        out.push_str(&format!("- {}\n", item.trim()));
    }
    if !def.collaboration.trim().is_empty() {
        out.push_str(&format!("\n## Working with others\n{}\n", def.collaboration.trim()));
    }
    if !def.boundaries.is_empty() {
        out.push_str("\n## Boundaries\n");
        for item in &def.boundaries {
            out.push_str(&format!("- {}\n", item.trim()));
        }
    }
    if def.budget > 0 {
        out.push_str(&format!("\n## Budget\nDefault task budget: {} credits.\n", def.budget));
    }
    out
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

/// The role's prompt: the prompt library's published prompt bound to the role (docs/design/prompt-library.md),
/// else the built-in prompt file, else the definition's prompt or one written from it.
pub fn prompt_for(app: &App, role: &str) -> String {
    if let Some(prompt) = app.prompts.bound_body(&format!("role:{role}")) {
        return prompt;
    }
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

/// The text under a built-in prompt's `## Purpose` heading, up to the next `## `.
fn purpose_of(prompt: &str) -> String {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in prompt.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if inside {
                break;
            }
            inside = heading.trim() == "Purpose";
            continue;
        }
        if inside {
            lines.push(line);
        }
    }
    lines.join("\n").trim().to_string()
}

/// Connected (not offline) agents of a role.
fn agent_count(app: &App, role: &str) -> u32 {
    app.agents.list().iter().filter(|a| a.role == role && a.status != Status::Offline).count() as u32
}

fn list_route(app: &App, _req: &Request, _: &Params<'_>) -> Response {
    let mut builtin: Vec<RoleView> = app
        .roles
        .tools
        .iter()
        .map(|(id, tools)| {
            let prompt = app.roles.prompts.get(id).cloned().unwrap_or_default();
            let title = agents::role_label(id).to_string();
            let def = RoleDef {
                id: id.clone(),
                title: title.clone(),
                purpose: purpose_of(&prompt),
                tools: tools.clone(),
                prompt,
                ..Default::default()
            };
            RoleView { id: id.clone(), title, source: "builtin".into(), agents: agent_count(app, id), def }
        })
        .collect();
    builtin.sort_by(|a, b| a.title.cmp(&b.title));
    let mut custom: Vec<RoleView> = app
        .custom_roles
        .list()
        .into_iter()
        .map(|def| RoleView {
            id: def.id.clone(),
            title: def.title.clone(),
            source: "custom".into(),
            agents: agent_count(app, &def.id),
            def,
        })
        .collect();
    custom.sort_by(|a, b| a.title.cmp(&b.title));
    builtin.extend(custom);
    Response::json(200, &builtin)
}

fn save_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let mut def: RoleDef = match req.json() {
        Ok(def) => def,
        Err(e) => return Response::error(400, &format!("bad role: {e}")),
    };
    if app.roles.tools.contains_key(&def.id) {
        return Response::error(409, "that id is a built-in role");
    }
    if app.custom_roles.get(&def.id).is_none() {
        def.created_by = "owner".into();
        def.created_ms = apz_plaza::now_ms();
    }
    let saved = match app.custom_roles.save(def) {
        Ok(saved) => saved,
        Err(e) => return Response::error(400, &e),
    };
    app.bus.publish("role", &saved);
    if agent_count(app, &saved.id) > 0 {
        agents::push_profiles(app);
    }
    Response::json(200, &saved)
}

fn remove_route(app: &App, _req: &Request, params: &Params<'_>) -> Response {
    let id = params.get("id");
    if agent_count(app, id) > 0 {
        return Response::error(409, "agents of that role are running");
    }
    match app.custom_roles.remove(id) {
        Ok(()) => {
            app.bus.publish("role", &json!({"id": id, "removed": true}));
            Response::json(200, &json!({"id": id, "removed": true}))
        }
        Err(e) => Response::error(404, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> RoleDef {
        RoleDef {
            id: id.into(),
            title: "Scout".into(),
            purpose: "Finds sources for the team.".into(),
            responsibilities: vec!["Search the web".into()],
            deliverables: vec!["A source list".into()],
            collaboration: "Takes work from the planner.".into(),
            boundaries: vec!["No purchases".into()],
            budget: 500,
            ..Default::default()
        }
    }

    #[test]
    fn save_validates_the_definition() {
        let store = Store::default();
        assert!(store.save(sample("scout")).is_ok());
        assert!(store.save(sample("s")).is_err(), "too short");
        assert!(store.save(sample("1scout")).is_err(), "starts with a digit");
        assert!(store.save(sample("Scout")).is_err(), "upper case");
        let mut no_resp = sample("noresp");
        no_resp.responsibilities.clear();
        assert!(store.save(no_resp).is_err(), "no responsibilities");
        let mut no_deliv = sample("nodeliv");
        no_deliv.deliverables.clear();
        assert!(store.save(no_deliv).is_err(), "no deliverables");
        let mut long_resp = sample("longresp");
        long_resp.responsibilities = vec!["x".repeat(301)];
        assert!(store.save(long_resp).is_err(), "responsibility too long");
        let mut bad_budget = sample("budget");
        bad_budget.budget = 100;
        assert!(store.save(bad_budget).is_err(), "budget below 200");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn save_replaces_by_id_and_keeps_the_creator() {
        let store = Store::default();
        let mut first = sample("scout");
        first.created_by = "owner".into();
        first.created_ms = 7;
        store.save(first).unwrap();
        let mut second = sample("scout");
        second.title = "Scout Two".into();
        let saved = store.save(second).unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(saved.created_ms, 7);
        assert_eq!(saved.created_by, "owner");
        assert_eq!(store.get("scout").unwrap().title, "Scout Two");
    }

    #[test]
    fn remove_reports_a_missing_role() {
        let store = Store::default();
        store.save(sample("scout")).unwrap();
        assert!(store.remove("scout").is_ok());
        assert_eq!(store.remove("scout").unwrap_err(), "no role scout");
    }

    #[test]
    fn prompt_has_the_sections() {
        let prompt = prompt_of(&sample("scout"));
        for section in [
            "# Scout",
            "## Purpose",
            "## Responsibilities",
            "## Deliverables",
            "## Working with others",
            "## Boundaries",
            "## Budget",
        ] {
            assert!(prompt.contains(section), "missing {section}");
        }
        assert!(prompt.contains("Default task budget: 500 credits."));
    }

    #[test]
    fn purpose_is_read_from_a_builtin_prompt() {
        let prompt = "# Builder\n\n## Purpose\nYou build.\nAnd test.\n\n## Output\nthings\n";
        assert_eq!(purpose_of(prompt), "You build.\nAnd test.");
    }
}
