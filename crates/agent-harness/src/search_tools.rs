//! Code search: `glob` (files by pattern, newest first) and `grep` (contents by regex,
//! ripgrep-like), both confined to the workspace.

use serde_json::Value;

use crate::files::Workspace;

/// Files under the workspace matching a glob pattern, newest first.
pub fn glob(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("glob is not available yet".to_string())
}

/// Lines of workspace files matching a regex.
pub fn grep(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("grep is not available yet".to_string())
}
