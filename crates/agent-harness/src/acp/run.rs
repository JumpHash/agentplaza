//! `code_engine` over ACP: one prompt turn of an engine session.

use serde_json::Value;

use super::link::EngineLink;
use crate::files::Workspace;

/// Runs one `code_engine` call as an ACP prompt turn and returns its summary for the model.
pub fn run(
    _workspace: &Workspace,
    _input: &Value,
    _link: &dyn EngineLink,
) -> Result<String, String> {
    Err("code_engine over ACP is not built yet".to_string())
}
