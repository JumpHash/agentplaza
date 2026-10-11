//! `code_engine`: a full coding agent (Claude Code, Codex, OpenCode) run headless inside
//! the VM, pointed at the plaza's model gateway so no key ever enters the VM.

use serde_json::Value;

use crate::files::Workspace;

/// Runs one job through the chosen engine and returns its final report.
pub fn run(_workspace: &Workspace, _input: &Value) -> Result<String, String> {
    Err("code_engine is not available yet".to_string())
}
