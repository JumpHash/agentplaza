//! Background shell commands: `bash_start` runs a command in the background and returns
//! an id, `bash_output` reads what it printed since last time, `bash_kill` stops it.

use serde_json::Value;

use crate::files::Workspace;

/// Starts a background command; returns its id.
pub fn start(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("start is not available yet".to_string())
}

/// New output of a background command and whether it still runs.
pub fn output(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("output is not available yet".to_string())
}

/// Stops a background command.
pub fn kill(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("kill is not available yet".to_string())
}
