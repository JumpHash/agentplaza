//! Agent-made tools: `tool_package` bundles a tool the agent built in its workspace (its
//! files, command, schema and needs) for the plaza's library, and [`run`] unpacks and runs
//! a library program apid sends with a tool decision — always inside this VM.

use apz_proto::wire::Program;
use serde_json::Value;

use crate::files::Workspace;

/// `tool_package`: reads the tool's directory and returns its bundle as JSON (apid files it
/// in the library when the call is reported).
pub fn package(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("tool_package is not available yet".to_string())
}

/// Runs a library program with the call's input; its stdout is the tool's output.
pub fn run(_workspace: &Workspace, _program: &Program, _input: &Value) -> Result<String, String> {
    Err("library tools are not available yet".to_string())
}
