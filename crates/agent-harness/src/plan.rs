//! The agent's plan for its current work: `todo_write` keeps a todo list.

use serde_json::Value;

use crate::files::Workspace;

/// Replaces the todo list and shows it back.
pub fn todo_write(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("todo_write is not available yet".to_string())
}
