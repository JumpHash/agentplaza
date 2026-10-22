//! The terminal UI's project switcher: the Sessions and Tasks tabs show every project's, or one project's. `p` and `P`
//! step through All projects and each project still open (active or paused), in the hub's order; the header names the
//! one shown. Pure: the projects come in as the hub's JSON (`GET /v1/projects`), so these rules are tested without a
//! terminal.

use serde_json::Value;

use crate::cmd::projects::state_of;

/// The projects the switcher offers, in the hub's order: the ones still open (active or paused), by id.
pub fn choices(projects: &[Value]) -> Vec<&str> {
    projects
        .iter()
        .filter(|p| matches!(state_of(p), "active" | "paused"))
        .filter_map(|p| p.get("id")?.as_str())
        .collect()
}

/// The scope `delta` steps from `scope` through All (None) and the choices, around the ends. A scope that is no
/// longer offered (its project was archived meanwhile) steps as if from All.
pub fn step(scope: Option<&str>, choices: &[&str], delta: isize) -> Option<String> {
    let len = choices.len() as isize + 1;
    let at = scope
        .and_then(|id| choices.iter().position(|c| *c == id))
        .map_or(0, |i| i as isize + 1);
    let next = (at + delta).rem_euclid(len);
    (next > 0).then(|| choices[next as usize - 1].to_string())
}

/// The project of the scope, from the list.
pub fn project_of<'a>(scope: Option<&str>, projects: &'a [Value]) -> Option<&'a Value> {
    let id = scope?;
    projects
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
}

/// Whether a session shows in the scope: every session in All, else the project's.
pub fn session_in(session: &Value, scope: Option<&str>) -> bool {
    scope.is_none_or(|id| session.get("project").and_then(Value::as_str) == Some(id))
}

/// Whether a task shows in the scope: every task in All, else the tasks that name the project, or that the project
/// lists (`project`; a task posted before tasks named their project is only listed by it).
pub fn task_in(task: &Value, scope: Option<&str>, project: Option<&Value>) -> bool {
    let Some(id) = scope else {
        return true;
    };
    if task.get("project").and_then(Value::as_str) == Some(id) {
        return true;
    }
    let Some(task_id) = task.get("id").and_then(Value::as_str) else {
        return false;
    };
    project
        .and_then(|p| p.get("tasks"))
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|t| t.as_str() == Some(task_id)))
}

/// The scope as the header names it: All projects, or the project's id and title, and its state when it is not
/// active.
pub fn label(scope: Option<&str>, projects: &[Value]) -> String {
    let Some(id) = scope else {
        return "All projects".to_string();
    };
    let Some(project) = project_of(scope, projects) else {
        return id.to_string();
    };
    let title = project.get("title").and_then(Value::as_str).unwrap_or("");
    let mut text = if title.is_empty() {
        id.to_string()
    } else {
        format!("{id} {title}")
    };
    match state_of(project) {
        "active" => {}
        state => text.push_str(&format!(" ({state})")),
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn projects() -> Vec<Value> {
        vec![
            json!({ "id": "pr1", "title": "Old", "stage": "delivered", "tasks": [] }),
            json!({ "id": "pr2", "title": "Release 0.9", "stage": "implementation", "tasks": ["t1", "t9"] }),
            json!({ "id": "pr3", "title": "Docs", "stage": "implementation", "paused": true, "tasks": [] }),
            json!({ "id": "pr4", "title": "Gone", "stage": "review", "archived": true, "tasks": [] }),
        ]
    }

    #[test]
    fn the_switcher_offers_the_open_projects_and_wraps_around() {
        let projects = projects();
        let open = choices(&projects);
        assert_eq!(open, ["pr2", "pr3"]);
        assert_eq!(step(None, &open, 1).as_deref(), Some("pr2"));
        assert_eq!(step(Some("pr2"), &open, 1).as_deref(), Some("pr3"));
        assert_eq!(step(Some("pr3"), &open, 1), None, "past the last is All");
        assert_eq!(step(None, &open, -1).as_deref(), Some("pr3"));
        assert_eq!(step(Some("pr2"), &open, -1), None);
        // An archived project's scope steps as if from All.
        assert_eq!(step(Some("pr4"), &open, 1).as_deref(), Some("pr2"));
        assert_eq!(step(None, &[], 1), None, "with no project, All stays");
    }

    #[test]
    fn a_scope_shows_its_projects_sessions_and_tasks() {
        let ada = json!({ "id": "s7", "project": "pr2" });
        let ben = json!({ "id": "s8", "project": null });
        assert!(session_in(&ada, None) && session_in(&ben, None));
        assert!(session_in(&ada, Some("pr2")));
        assert!(
            !session_in(&ben, Some("pr2")),
            "a session with no project is in no project's scope"
        );
        let projects = projects();
        let pr2 = project_of(Some("pr2"), &projects);
        let named = json!({ "id": "t5", "project": "pr2" });
        let listed = json!({ "id": "t9", "project": null });
        let other = json!({ "id": "t6", "project": "pr3" });
        assert!(task_in(&named, Some("pr2"), pr2));
        assert!(
            task_in(&listed, Some("pr2"), pr2),
            "a task from before projects that the project lists"
        );
        assert!(!task_in(&other, Some("pr2"), pr2));
        assert!(task_in(&other, None, None));
    }

    #[test]
    fn the_header_names_the_scope_and_its_state() {
        let projects = projects();
        assert_eq!(label(None, &projects), "All projects");
        assert_eq!(label(Some("pr2"), &projects), "pr2 Release 0.9");
        assert_eq!(label(Some("pr3"), &projects), "pr3 Docs (paused)");
        assert_eq!(label(Some("pr4"), &projects), "pr4 Gone (archived)");
        assert_eq!(label(Some("pr9"), &projects), "pr9");
    }
}
