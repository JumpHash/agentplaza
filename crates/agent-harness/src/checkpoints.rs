//! Workspace checkpoints: a snapshot at the start of every piece of work (and on demand),
//! a diff against any of them, and restore — the agent's undo, and the owner's review.

use serde_json::Value;

use crate::files::Workspace;

/// The workspace's changes since a checkpoint, as a unified diff.
pub fn diff(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("checkpoint_diff is not available yet".to_string())
}

/// Rolls the workspace or a path back to a checkpoint.
pub fn restore(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("checkpoint_restore is not available yet".to_string())
}

/// The checkpoints, newest first.
pub fn list(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("checkpoint_list is not available yet".to_string())
}
