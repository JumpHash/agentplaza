//! The tools the hub serves, and their static registry.

pub mod observe;
pub mod operate;
pub mod govern;

use serde_json::Value;

use super::auth::{Scope, TokenRecord};

/// The read model and the owner's writes the tools call. Each method goes through the same service the HTTP API uses.
pub trait Plaza: Sync {
    fn status(&self) -> Value;
    fn agents(&self) -> Vec<Value>;
    fn agent(&self, cid: u64) -> Option<Value>;
    fn transcript(&self, cid: u64, after: u64, limit: usize) -> Vec<Value>;
    fn tasks(&self) -> Vec<Value>;
    fn task(&self, id: &str) -> Option<Value>;
    fn board(&self, limit: usize) -> Value;
    fn projects(&self) -> Vec<Value>;
    fn project(&self, id: &str) -> Option<Value>;
    fn approvals(&self) -> Vec<Value>;
    fn machines(&self) -> Vec<Value>;
    fn treasury(&self, journal_limit: usize) -> Value;
    fn usage(&self, from_ms: u64, to_ms: u64) -> Value;
    fn laws(&self) -> Vec<Value>;
    fn message_orchestrator(&self, text: &str) -> Result<Value, String>;
    fn tell_agent(&self, cid: u64, text: &str) -> Result<Value, String>;
    fn stop_agent(&self, cid: u64, reason: &str) -> Result<Value, String>;
    fn search_library(&self, query: &str, limit: usize) -> Vec<Value>;
    fn restart_agent(&self, cid: u64) -> Result<Value, String>;
    fn record_decision(&self, approval_id: &str, decision: &str, note: &str) -> Result<Value, String>;
    fn create_task(&self, input: &Value) -> Result<Value, String>;
}

/// What a tool runs with.
pub struct Ctx<'a> {
    pub plaza: &'a dyn Plaza,
}

/// One tool. Its name is `snake_case` with no prefix; the client namespaces it.
pub trait Tool: Sync {
    fn name(&self) -> &'static str;
    fn scope(&self) -> Scope;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    fn output_schema(&self) -> Value;
    /// Runs the tool. An `Err` is the message the client sees, with `isError` set.
    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String>;
    /// A one-line text summary of a result, for clients that read only text.
    fn summary(&self, out: &Value) -> String;
}

/// Every tool the server knows.
pub static REGISTRY: &[&dyn Tool] = &[
    &observe::PlazaStatus,
    &observe::ListAgents,
    &observe::GetAgent,
    &observe::GetTranscript,
    &observe::ListTasks,
    &observe::GetTask,
    &observe::ListBoard,
    &observe::ListProjects,
    &observe::GetProject,
    &observe::ListApprovals,
    &observe::ListMachines,
    &observe::TreasurySummary,
    &observe::UsageSummary,
    &observe::ListLaws,
    &observe::SearchLibrary,
    &operate::MessageOrchestrator,
    &operate::TellAgent,
    &operate::StopAgent,
    &operate::RestartAgent,
    &operate::CreateTask,
    &govern::RequestDecision,
];

pub fn find(name: &str) -> Option<&'static dyn Tool> {
    REGISTRY.iter().copied().find(|tool| tool.name() == name)
}

/// Whether a token may call a tool: its scope permits the tool's, and its allowlist (when it has one) names it.
pub fn allowed(token: &TokenRecord, tool: &dyn Tool) -> bool {
    token.scope.permits(tool.scope())
        && (token.allowlist.is_empty() || token.allowlist.iter().any(|name| name.as_str() == tool.name()))
}

/// The tools of `registry` that a token may see.
pub fn visible(registry: &[&'static dyn Tool], token: &TokenRecord) -> Vec<&'static dyn Tool> {
    registry.iter().copied().filter(|tool| allowed(token, *tool)).collect()
}
