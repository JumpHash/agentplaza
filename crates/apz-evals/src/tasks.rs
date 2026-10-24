//! The suite's tasks: each a folder under `tasks/` with its `task.json` (the goal the hub's task carries and the check
//! that decides the outcome, a shell command run in the workspace), its `fixture/` (copied into the workspace before
//! the task runs), and its turns in `script.json`, the one file (with `roles.json`) that names the tools.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

/// A task as its folder says it.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub name: String,
    /// The goal the task is created with.
    pub goal: String,
    /// The shell command that decides the outcome: it must exit 0 in the workspace.
    pub check: String,
    /// The folder copied into the workspace.
    pub fixture: PathBuf,
    /// The builder's and the reviewer's scripted turns (see `apz-gateway`'s mock).
    pub builder: Vec<Value>,
    pub reviewer: Vec<Value>,
}

#[derive(Deserialize)]
struct Manifest {
    goal: String,
    check: String,
}

#[derive(Deserialize)]
struct Seats {
    builder: Vec<Value>,
    reviewer: Vec<Value>,
}

/// Every task under `dir`, by name.
pub fn load(dir: &Path) -> Result<Vec<Task>, String> {
    let script_path = dir.join("script.json");
    let script: BTreeMap<String, Seats> = read_json(&script_path)?;
    let mut tasks = Vec::new();
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join("task.json").is_file())
        .collect();
    folders.sort();
    for folder in folders {
        let name = folder
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let manifest: Manifest = read_json(&folder.join("task.json"))?;
        let seats = script
            .get(&name)
            .ok_or_else(|| format!("{name}: no turns in {}", script_path.display()))?;
        tasks.push(Task {
            name,
            goal: manifest.goal,
            check: manifest.check,
            fixture: folder.join("fixture"),
            builder: seats.builder.clone(),
            reviewer: seats.reviewer.clone(),
        });
    }
    Ok(tasks)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))
}

/// The mock's script for `task`: one entry per seat, answering the conversation whose prompt names the task's goal and
/// the seat. The seats' prompts are apid's (`work_loop.rs`, `review_graph`): "this task's builder", "one of this task's
/// sceptic reviewers" and "You are this task's verifier". The verifier proves findings, and the scripted change has none,
/// so it passes with the reviewers' turns.
pub fn script_for(task: &Task) -> Value {
    json!([
        {
            "when": [task.goal, "this task's builder"],
            "turns": task.builder,
        },
        {
            "when": [task.goal, "this task's sceptic reviewers"],
            "turns": task.reviewer,
        },
        {
            "when": [task.goal, "You are this task's verifier"],
            "turns": task.reviewer,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tasks_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tasks")
    }

    #[test]
    fn every_task_has_a_goal_a_check_a_fixture_and_turns_for_both_seats() {
        let tasks = load(&tasks_dir()).unwrap();
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "add-function",
                "answer-question",
                "fix-test",
                "fix-typo",
                "rename-symbol",
                "survey"
            ]
        );
        for task in &tasks {
            assert!(
                !task.goal.is_empty() && !task.check.is_empty(),
                "{}",
                task.name
            );
            assert!(task.fixture.is_dir(), "{} has no fixture", task.name);
            assert!(
                !task.builder.is_empty() && !task.reviewer.is_empty(),
                "{}",
                task.name
            );
        }
    }

    #[test]
    fn a_tasks_script_answers_each_seat_by_its_goal_and_role() {
        let task = Task {
            name: "t".into(),
            goal: "Do it.".into(),
            check: "true".into(),
            fixture: PathBuf::new(),
            builder: vec![json!({"text": "built"})],
            reviewer: vec![json!({"text": "reviewed"})],
        };
        let script = script_for(&task);
        assert_eq!(script[0]["when"], json!(["Do it.", "this task's builder"]));
        assert_eq!(script[0]["turns"], json!([{"text": "built"}]));
        assert_eq!(
            script[1]["when"],
            json!(["Do it.", "this task's sceptic reviewers"])
        );
        assert_eq!(script[1]["turns"], json!([{"text": "reviewed"}]));
        assert_eq!(
            script[2]["when"],
            json!(["Do it.", "You are this task's verifier"])
        );
        assert_eq!(script[2]["turns"], json!([{"text": "reviewed"}]));
    }
}
