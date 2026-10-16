//! The agent's plan for its current work: `todo_write` keeps a todo list.
//!
//! Each call replaces the whole list. The list is saved at
//! `<workspace>/.plaza/todos.json`, so a later turn can read back where the plan
//! stands.

use std::fs;

use serde_json::{Value, json};

use crate::files::Workspace;

/// The most todos one list holds.
const MAX_TODOS: usize = 50;

/// The longest a todo's content may be, in characters.
const MAX_CONTENT: usize = 300;

/// Replaces the todo list, saves it, and shows it back.
///
/// Input: `{"todos": [{"content": "...", "status": "pending" | "in_progress" | "completed"}]}`,
/// with 1 to 50 todos of 1 to 300 characters and at most one `in_progress`. The
/// reply has one line per todo (`[x]` done, `[>]` current, `[ ]` pending) and an
/// `N/M done` footer.
pub fn todo_write(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let todos = parse(input)?;
    save(workspace, &todos)?;
    Ok(render(&todos))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pending,
    InProgress,
    Completed,
}

impl Status {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[>]",
            Self::Completed => "[x]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Todo {
    content: String,
    status: Status,
}

/// Checks the input and reads it into todos.
fn parse(input: &Value) -> Result<Vec<Todo>, String> {
    let items = input
        .get("todos")
        .and_then(Value::as_array)
        .ok_or("\"todos\" must be a list")?;
    if items.is_empty() || items.len() > MAX_TODOS {
        return Err(format!(
            "\"todos\" must hold 1 to {MAX_TODOS} items, got {}",
            items.len()
        ));
    }
    let mut todos = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let number = index + 1;
        let content = item
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("todo {number} needs a string \"content\""))?;
        let length = content.chars().count();
        if content.trim().is_empty() || length > MAX_CONTENT {
            return Err(format!(
                "todo {number} content must be 1 to {MAX_CONTENT} characters, got {length}"
            ));
        }
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .and_then(Status::parse)
            .ok_or_else(|| {
                format!("todo {number} status must be pending, in_progress or completed")
            })?;
        todos.push(Todo {
            content: content.to_string(),
            status,
        });
    }
    let current = todos
        .iter()
        .filter(|todo| todo.status == Status::InProgress)
        .count();
    if current > 1 {
        return Err(format!("only one todo may be in_progress, got {current}"));
    }
    Ok(todos)
}

/// The list as lines, with the `N/M done` footer.
fn render(todos: &[Todo]) -> String {
    let mut lines: Vec<String> = todos
        .iter()
        .map(|todo| format!("{} {}", todo.status.mark(), todo.content))
        .collect();
    let done = todos
        .iter()
        .filter(|todo| todo.status == Status::Completed)
        .count();
    lines.push(format!("{done}/{} done", todos.len()));
    lines.join("\n")
}

/// Writes the list to `<workspace>/.plaza/todos.json`, creating the directory.
fn save(workspace: &Workspace, todos: &[Todo]) -> Result<(), String> {
    let dir = workspace.dir().join(".plaza");
    fs::create_dir_all(&dir).map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    let items: Vec<Value> = todos
        .iter()
        .map(|todo| json!({"content": todo.content, "status": todo.status.name()}))
        .collect();
    let text = serde_json::to_string_pretty(&json!({"todos": items}))
        .map_err(|err| format!("cannot encode the todo list: {err}"))?;
    let path = dir.join("todos.json");
    fs::write(&path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh scratch workspace under the system temp directory, named by the
    /// test so that parallel tests do not share one.
    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("apz-plan-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    fn list(todos: Value) -> Value {
        json!({"todos": todos})
    }

    #[test]
    fn the_list_renders_with_marks_and_a_footer() {
        let workspace = scratch("render");
        let input = list(json!([
            {"content": "read the spec", "status": "completed"},
            {"content": "write the parser", "status": "in_progress"},
            {"content": "run the tests", "status": "pending"},
        ]));
        let output = todo_write(&workspace, &input).expect("the list is valid");
        assert_eq!(
            output,
            "[x] read the spec\n[>] write the parser\n[ ] run the tests\n1/3 done"
        );
    }

    #[test]
    fn invalid_todo_lists_are_refused() {
        // Only one todo may be in progress; a list with two is refused and nothing is saved.
        {
            let workspace = scratch("one-current");
            let input = list(json!([
                {"content": "a", "status": "in_progress"},
                {"content": "b", "status": "in_progress"},
            ]));
            let err = todo_write(&workspace, &input).expect_err("two current todos are refused");
            assert!(err.contains("only one todo may be in_progress"), "{err}");
            assert!(!workspace.dir().join(".plaza").join("todos.json").exists());
        }
        // Empty lists, over-long content, unknown statuses and too many todos are all refused.
        {
            let workspace = scratch("refuse");
            assert!(todo_write(&workspace, &list(json!([]))).is_err());
            let long = "x".repeat(MAX_CONTENT + 1);
            assert!(
                todo_write(
                    &workspace,
                    &list(json!([{"content": long, "status": "pending"}]))
                )
                .is_err()
            );
            assert!(
                todo_write(
                    &workspace,
                    &list(json!([{"content": "ok", "status": "later"}]))
                )
                .is_err()
            );
            let many: Vec<Value> = (0..=MAX_TODOS)
                .map(|i| json!({"content": format!("t{i}"), "status": "pending"}))
                .collect();
            assert!(todo_write(&workspace, &list(Value::Array(many))).is_err());
        }
    }

    #[test]
    fn the_list_is_saved_and_replaced_whole() {
        let workspace = scratch("persist");
        todo_write(
            &workspace,
            &list(json!([{"content": "first", "status": "pending"}])),
        )
        .expect("the first list is saved");
        todo_write(
            &workspace,
            &list(json!([{"content": "second", "status": "completed"}])),
        )
        .expect("the second list is saved");
        let saved = fs::read_to_string(workspace.dir().join(".plaza").join("todos.json"))
            .expect("the saved file");
        let parsed: Value = serde_json::from_str(&saved).expect("the file is JSON");
        assert_eq!(
            parsed,
            list(json!([{"content": "second", "status": "completed"}]))
        );
    }
}
