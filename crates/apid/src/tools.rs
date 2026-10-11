//! The tools agents are offered: their specs, who runs them, and the host
//! tools `apid` runs itself.
//!
//! OWNER: slice "loop". Signatures used by other modules are marked
//! `// contract` — keep them.

use apz_proto::chat::ToolSpec;
use serde_json::Value;

use crate::agents::Agent;
use crate::state::App;

/// Where a tool runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// In the agent's VM (the harness).
    Guest,
    /// In apid, with the agent's identity.
    Host,
}

/// Where `name` runs, or `None` for an unknown tool.
pub fn runs(name: &str) -> Option<Runs> {
    // contract — TODO(loop slice)
    let _ = name;
    None
}

/// The tool specs an agent of `role` is offered (its grants + plaza tools).
pub fn specs_for(app: &App, role: &str) -> Vec<ToolSpec> {
    // contract — TODO(loop slice)
    let _ = (app, role);
    Vec::new()
}

/// Runs host tool `name` for `agent`; `(is_error, output)`.
pub fn run_host(app: &App, agent: &Agent, work: u64, name: &str, input: &Value) -> (bool, String) {
    // contract — TODO(loop slice)
    let _ = (app, agent, work, input);
    (true, format!("{name} is not implemented"))
}
