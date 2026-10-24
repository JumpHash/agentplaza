//! The console's scope: one project, or All projects (the web's `scope.ts`). The top bar's switcher sets it
//! ([`super::shell`]), every screen reads it from [`super::Ctx::project`], and it is remembered for the next run,
//! hub by hub. Under a project the screens show its agents, tasks, board and approvals; All projects is the whole
//! plaza, every agent with where it works.

use serde_json::Value;

/// The `desktop.json` key holding the remembered scope of each hub: `{ "<hub url>": "<project id>" }`.
const KEY: &str = "scope";

/// The projects' (id, title), as the shell last read them: the screens' headings name the project they show.
static TITLES: std::sync::RwLock<Vec<(String, String)>> = std::sync::RwLock::new(Vec::new());

/// Keeps the projects' titles for [`title`] (the shell calls it on each load).
pub fn remember_titles(list: &[Value]) {
    let titles = list
        .iter()
        .filter_map(|p| {
            let id = p["id"].as_str()?;
            Some((id.to_string(), title_in(list, id)))
        })
        .collect();
    if let Ok(mut held) = TITLES.write() {
        *held = titles;
    }
}

/// A project's title, or its id while the shell has not read it.
pub fn title(id: &str) -> String {
    TITLES
        .read()
        .ok()
        .and_then(|t| {
            t.iter()
                .find(|(pid, _)| pid == id)
                .map(|(_, title)| title.clone())
        })
        .unwrap_or_else(|| id.to_string())
}

/// A screen's heading: its name, and under a project the project's title (`Approvals · Release 0.9`).
pub fn heading(screen: &str, project: Option<&str>) -> String {
    match project {
        Some(p) => format!("{screen} · {}", title(p)),
        None => screen.to_string(),
    }
}

/// Whether a project still has work: not archived, not delivered.
pub fn is_live(project: &Value) -> bool {
    !project["archived"].as_bool().unwrap_or(false)
        && project["stage"].as_str() != Some("delivered")
}

/// The projects the switcher offers: the live ones, newest first.
pub fn live_projects(list: &[Value]) -> Vec<&Value> {
    let mut live: Vec<&Value> = list.iter().filter(|p| is_live(p)).collect();
    live.sort_by_key(|p| std::cmp::Reverse(p["created_ms"].as_u64().unwrap_or(0)));
    live
}

/// A project as a picker lists it: its title, carrying its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub id: String,
    pub title: String,
}

impl std::fmt::Display for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.title)
    }
}

/// The live projects as picker entries, newest first.
pub fn picks(list: &[Value]) -> Vec<Pick> {
    live_projects(list)
        .into_iter()
        .filter_map(|p| {
            let id = p["id"].as_str()?;
            Some(Pick {
                id: id.to_string(),
                title: title_in(list, id),
            })
        })
        .collect()
}

/// A project's title from `list`, or its id while the list does not have it.
pub fn title_in(list: &[Value], id: &str) -> String {
    list.iter()
        .find(|p| p["id"].as_str() == Some(id))
        .and_then(|p| p["title"].as_str())
        .filter(|t| !t.trim().is_empty())
        .map_or_else(|| id.to_string(), str::to_string)
}

/// Whether an agent is the orchestrator, which works on every project and needs none.
pub fn is_orchestrator(agent: &Value) -> bool {
    agent["tier"].as_str() == Some("orchestrator")
}

/// Whether `agent` shows under `project`: one working on it, or the orchestrator; All projects (None) shows every agent.
pub fn in_scope(agent: &Value, project: Option<&str>) -> bool {
    match project {
        None => true,
        Some(p) => agent["project"].as_str() == Some(p) || is_orchestrator(agent),
    }
}

/// Where an agent works, in words: its project's title, "Every project" for the orchestrator, else "No project".
pub fn where_word(agent: &Value, projects: &[Value]) -> String {
    match agent["project"].as_str().filter(|p| !p.is_empty()) {
        Some(p) => title_in(projects, p),
        None if is_orchestrator(agent) => "Every project".into(),
        None => "No project".into(),
    }
}

/// The project remembered for the hub at `hub` (not from tests).
pub fn remembered(hub: &str) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    super::boot::saved_setting(KEY)?
        .get(hub)?
        .as_str()
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

/// Remembers `project` (None: All projects) for the hub at `hub` (not from tests).
pub fn remember(hub: &str, project: Option<&str>) {
    if cfg!(test) {
        return;
    }
    let mut all = super::boot::saved_setting(KEY)
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    match project {
        Some(p) => {
            all.insert(hub.to_string(), Value::from(p));
        }
        None => {
            all.remove(hub);
        }
    }
    if let Err(err) = super::boot::save_setting(KEY, Value::Object(all)) {
        eprintln!("agentplaza-desktop: the project scope was not saved: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_switcher_offers_the_live_projects_newest_first() {
        let list = vec![
            json!({ "id": "pr1", "title": "Old", "stage": "build", "created_ms": 1 }),
            json!({ "id": "pr2", "title": "Done", "stage": "delivered", "created_ms": 2 }),
            json!({ "id": "pr3", "title": "Gone", "stage": "build", "archived": true, "created_ms": 3 }),
            json!({ "id": "pr4", "title": "New", "stage": "brief", "created_ms": 4 }),
        ];
        let ids: Vec<&str> = live_projects(&list)
            .iter()
            .filter_map(|p| p["id"].as_str())
            .collect();
        assert_eq!(ids, ["pr4", "pr1"]);
        assert_eq!(title_in(&list, "pr4"), "New");
        assert_eq!(
            title_in(&list, "pr9"),
            "pr9",
            "an unknown project shows its id"
        );
    }

    #[test]
    fn a_heading_names_the_project_once_its_title_is_known() {
        assert_eq!(heading("Board", None), "Board");
        assert_eq!(heading("Board", Some("pr-unknown")), "Board · pr-unknown");
        remember_titles(&[json!({ "id": "pr-h", "title": "Docs refresh" })]);
        assert_eq!(heading("Board", Some("pr-h")), "Board · Docs refresh");
    }

    #[test]
    fn who_is_in_a_scope_and_where_each_agent_works() {
        let working = json!({ "cid": 300, "project": "pr2", "tier": "worker", "kind": "vm" });
        let idle = json!({ "cid": 301, "project": null, "tier": "worker", "kind": "vm" });
        let orchestrator = json!({ "cid": 109, "tier": "orchestrator", "kind": "vm" });
        assert!(in_scope(&working, Some("pr2")) && !in_scope(&working, Some("pr1")));
        assert!(!in_scope(&idle, Some("pr2")) && in_scope(&idle, None));
        assert!(
            in_scope(&orchestrator, Some("pr1")),
            "the orchestrator works on every project"
        );
        let projects = vec![json!({ "id": "pr2", "title": "Release 0.9" })];
        assert_eq!(where_word(&working, &projects), "Release 0.9");
        assert_eq!(where_word(&orchestrator, &projects), "Every project");
        assert_eq!(where_word(&idle, &projects), "No project");
    }
}
