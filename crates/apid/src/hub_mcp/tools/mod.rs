//! The tools the hub serves, and their static registry.

pub mod observe;
pub mod operate;
pub mod govern;

use std::time::{SystemTime, UNIX_EPOCH};

use apz_prompt::Kind;
use serde_json::{json, Value};

use super::auth::{Scope, TokenRecord};
use crate::prompts::{Prompts, StoreError};

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
    /// The prompt library the prompt tools read and write.
    fn prompts(&self) -> &Prompts;
    /// The skill library the skill tools read and write.
    fn skills(&self) -> &crate::skills::Skills;
    /// Publishes a library change on the event stream: the kind (`prompt` or `skill`), the id and the action taken.
    /// Only the app has a bus; a plaza without one (a test fake) publishes nothing.
    fn announce(&self, _kind: &str, _id: &str, _action: &str) {}

    /// The live skills, by id. Their live version's score; the skill tools never publish.
    fn skills_list(&self) -> Result<Vec<Value>, String> {
        let store = self.skills().store();
        let mut rows = Vec::new();
        for skill in store.list(false)? {
            let score = match skill.active_version {
                Some(live) => store.versions(&skill.id)?.into_iter().find(|meta| meta.version == live).map(|meta| meta.score),
                None => None,
            };
            rows.push(json!({
                "id": skill.id,
                "name": skill.name,
                "title": skill.title,
                "description": skill.description,
                "bindings": skill.bindings,
                "active_version": skill.active_version,
                "latest_version": skill.latest_version,
                "score": score,
            }));
        }
        Ok(rows)
    }

    /// One version of a skill (default: the live one, else the latest): SKILL.md, its files, score and findings.
    fn skill_get(&self, id: &str, version: Option<u32>) -> Result<Value, String> {
        let store = self.skills().store();
        let skill = store.get(id)?.ok_or_else(|| format!("no skill {id}"))?;
        let wanted = version.or(skill.active_version).unwrap_or(skill.latest_version);
        let found = store.version(id, wanted)?.ok_or_else(|| format!("{id} has no version {wanted}"))?;
        let mut out = serde_json::to_value(&found).map_err(|err| err.to_string())?;
        out["id"] = json!(id);
        Ok(out)
    }

    /// Adds a skill version authored over MCP. Each file is `{path, content_b64}` (new bytes) or `{path, sha256}`
    /// (bytes already in the library). It never publishes.
    fn skill_propose(&self, id: &str, skill_md: &str, files: &[Value], note: &str, base_version: u32) -> Result<Value, String> {
        let inputs = files.iter().map(skill_file_in).collect::<Result<Vec<_>, _>>()?;
        let skills = self.skills();
        let next = skills.prepare(skill_md.to_string(), inputs, note.to_string(), None).map_err(skill_store_text)?;
        let meta = skills.store().add_version(id, base_version, &next, MCP_ACTOR, now_ms()).map_err(skill_store_text)?;
        self.announce("skill", id, "version");
        Ok(json!({ "id": id, "version": meta.version, "score": meta.score, "errors": meta.errors, "warnings": meta.warnings }))
    }

    /// Publishes one skill version as the live one; refused while it has error-level findings.
    fn skill_publish(&self, id: &str, version: u32) -> Result<Value, String> {
        let skill = self.skills().publish(id, version, MCP_ACTOR, now_ms()).map_err(skill_store_text)?;
        self.announce("skill", id, "publish");
        Ok(json!({ "id": id, "active_version": skill.active_version, "latest_version": skill.latest_version }))
    }

    /// The live prompts, by id. `kind` (`system`, `agents_md` or `fragment`) narrows them.
    fn prompts_list(&self, kind: Option<&str>) -> Result<Vec<Value>, String> {
        let kind = match kind {
            Some(text) => Some(Kind::parse(text).ok_or_else(|| "kind must be system, agents_md or fragment".to_string())?),
            None => None,
        };
        let store = self.prompts().store();
        let mut rows = Vec::new();
        for prompt in store.list(false)?.into_iter().filter(|prompt| kind.map_or(true, |wanted| prompt.kind == wanted)) {
            let score = match prompt.active_version {
                Some(live) => store.versions(&prompt.id)?.into_iter().find(|meta| meta.version == live).map(|meta| meta.score),
                None => None,
            };
            rows.push(json!({
                "id": prompt.id,
                "kind": prompt.kind.as_str(),
                "title": prompt.title,
                "binding": prompt.binding,
                "active_version": prompt.active_version,
                "latest_version": prompt.latest_version,
                "score": score,
            }));
        }
        Ok(rows)
    }

    /// One version (default: the live one, else the latest), with its body, score and findings.
    fn prompt_get(&self, id: &str, version: Option<u32>) -> Result<Value, String> {
        let store = self.prompts().store();
        let prompt = store.get(id)?.ok_or_else(|| format!("no prompt {id}"))?;
        let wanted = version.or(prompt.active_version).unwrap_or(prompt.latest_version);
        let found = store.version(id, wanted)?.ok_or_else(|| format!("{id} has no version {wanted}"))?;
        let mut out = serde_json::to_value(&found).map_err(|err| err.to_string())?;
        out["id"] = json!(id);
        Ok(out)
    }

    /// Adds a version authored over MCP. It never publishes.
    fn prompt_propose(&self, id: &str, body: &str, note: &str, base_version: u32) -> Result<Value, String> {
        let (meta, created) = self
            .prompts()
            .add_version(id, base_version, body.to_string(), note.to_string(), MCP_ACTOR, now_ms())
            .map_err(store_text)?;
        if created {
            self.announce("prompt", id, "version");
        }
        Ok(json!({ "id": id, "version": meta.version, "created": created, "score": meta.score, "errors": meta.errors }))
    }

    /// Publishes one version as the live one; refused while it has error-level findings.
    fn prompt_publish(&self, id: &str, version: u32) -> Result<Value, String> {
        let prompt = self.prompts().publish(id, version, MCP_ACTOR, now_ms()).map_err(store_text)?;
        self.announce("prompt", id, "publish");
        Ok(json!({ "id": id, "active_version": prompt.active_version, "latest_version": prompt.latest_version }))
    }
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
    &observe::PromptsList,
    &observe::PromptGet,
    &observe::SkillsList,
    &observe::SkillGet,
    &operate::MessageOrchestrator,
    &operate::TellAgent,
    &operate::StopAgent,
    &operate::RestartAgent,
    &operate::CreateTask,
    &operate::PromptPropose,
    &operate::SkillPropose,
    &govern::RequestDecision,
    &govern::PromptPublish,
    &govern::SkillPublish,
];

pub fn find(name: &str) -> Option<&'static dyn Tool> {
    REGISTRY.iter().copied().find(|tool| tool.name() == name)
}

/// Whether a token may call a tool: its scope permits the tool's, and its allowlist (when it has one) names it.
pub fn allowed(token: &TokenRecord, tool: &dyn Tool) -> bool {
    token.scope.permits(tool.scope())
        && (token.allowlist.is_empty() || token.allowlist.iter().any(|name| name.as_str() == tool.name()))
}

/// Who the prompt tools record as author and publisher: a tool does not see the caller's identity.
pub const MCP_ACTOR: &str = "mcp";

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// A skill-library refusal, as the message a tool returns.
fn skill_store_text(err: crate::skills::StoreError) -> String {
    use crate::skills::StoreError as Skill;
    match err {
        Skill::NotFound => "no such skill".to_string(),
        Skill::Conflict(text) | Skill::Invalid(text) | Skill::Failed(text) => text,
    }
}

/// One file of a `skill_propose` call, as the library takes it.
fn skill_file_in(file: &Value) -> Result<crate::skills::FileIn, String> {
    use base64::Engine as _;
    let path = file.get("path").and_then(Value::as_str).ok_or("each file needs a path")?.to_string();
    let executable = file.get("executable").and_then(Value::as_bool).unwrap_or(false);
    let body = match (file.get("content_b64").and_then(Value::as_str), file.get("sha256").and_then(Value::as_str)) {
        (Some(encoded), None) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|err| format!("{path}: content_b64 is not base64: {err}"))?;
            crate::skills::FileBody::Bytes(bytes)
        }
        (None, Some(sha256)) => crate::skills::FileBody::Known(sha256.to_string()),
        _ => return Err(format!("{path}: give content_b64 or sha256, not both or neither")),
    };
    Ok(crate::skills::FileIn { path, body, executable })
}

/// A store refusal, as the message a tool returns.
fn store_text(err: StoreError) -> String {
    match err {
        StoreError::NotFound => "no such prompt".to_string(),
        StoreError::Conflict(text) | StoreError::Invalid(text) | StoreError::Failed(text) => text,
    }
}

/// A whole-number argument from 1 up (a prompt version).
pub fn version_arg(args: &Value, key: &str) -> Result<u32, String> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number >= 1)
        .ok_or_else(|| format!("{key} must be a whole number from 1"))
}

/// The tools of `registry` that a token may see.
pub fn visible(registry: &[&'static dyn Tool], token: &TokenRecord) -> Vec<&'static dyn Tool> {
    registry.iter().copied().filter(|tool| allowed(token, *tool)).collect()
}
