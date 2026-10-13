//! The govern tools: they record a recommendation on an approval, and never decide it themselves.

use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::{version_arg, Ctx, Tool};
use crate::hub_mcp::auth::Scope;

/// The recommendations clients have filed, oldest first. They are advice the owner reads; the approval
/// stays pending until the owner decides it in the console or the CLI.
static RECOMMENDATIONS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Files a recommendation on an approval and returns the row.
pub fn record(approval_id: &str, decision: &str, note: &str) -> Value {
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64);
    let row = json!({ "approval_id": approval_id, "decision": decision, "note": note, "at": at });
    RECOMMENDATIONS.lock().unwrap_or_else(PoisonError::into_inner).push(row.clone());
    row
}

/// The newest recommendation filed on an approval, if any.
pub fn recommendation_for(approval_id: &str) -> Option<Value> {
    RECOMMENDATIONS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|row| row["approval_id"] == approval_id)
        .cloned()
}

pub struct PromptPublish;
pub struct RequestDecision;
pub struct SkillPublish;

impl Tool for SkillPublish {
    fn name(&self) -> &'static str {
        "skill_publish"
    }

    fn scope(&self) -> Scope {
        Scope::Govern
    }

    fn description(&self) -> &'static str {
        "Publishes one version of a skill as its live version. Refused while the version has error-level findings."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "version": { "type": "integer", "minimum": 1 },
            },
            "required": ["id", "version"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "active_version": { "type": "integer" },
                "latest_version": { "type": "integer" },
            },
            "required": ["id", "active_version", "latest_version"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id must be a string")?;
        ctx.plaza.skill_publish(id, version_arg(args, "version")?)
    }

    fn summary(&self, out: &Value) -> String {
        format!("Published version {} of skill {}.", out["active_version"], out["id"].as_str().unwrap_or("?"))
    }
}

impl Tool for PromptPublish {
    fn name(&self) -> &'static str {
        "prompt_publish"
    }

    fn scope(&self) -> Scope {
        Scope::Govern
    }

    fn description(&self) -> &'static str {
        "Publishes one version of a prompt as its live version. Refused while the version has error-level findings."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "version": { "type": "integer", "minimum": 1 },
            },
            "required": ["id", "version"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "active_version": { "type": "integer" },
                "latest_version": { "type": "integer" },
            },
            "required": ["id", "active_version", "latest_version"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id must be a string")?;
        ctx.plaza.prompt_publish(id, version_arg(args, "version")?)
    }

    fn summary(&self, out: &Value) -> String {
        format!("Published version {} of {}.", out["active_version"], out["id"].as_str().unwrap_or("?"))
    }
}

impl Tool for RequestDecision {
    fn name(&self) -> &'static str {
        "request_decision"
    }

    fn scope(&self) -> Scope {
        Scope::Govern
    }

    fn description(&self) -> &'static str {
        "Records the client's recommendation on a pending approval and notifies the owner. It does not decide the approval; the owner confirms in the console or the CLI."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "approval_id": { "type": "string", "minLength": 1 },
                "decision": { "type": "string", "enum": ["approve", "deny"] },
                "note": { "type": "string" },
            },
            "required": ["approval_id", "decision", "note"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "recorded": { "type": "boolean" },
                "decided": { "type": "boolean" },
                "approval_id": { "type": "string" },
                "decision": { "type": "string" },
            },
            "required": ["recorded", "decided"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let approval_id = args.get("approval_id").and_then(Value::as_str).ok_or("approval_id must be a string")?;
        let decision = args
            .get("decision")
            .and_then(Value::as_str)
            .filter(|decision| *decision == "approve" || *decision == "deny")
            .ok_or("decision must be approve or deny")?;
        let note = args.get("note").and_then(Value::as_str).unwrap_or("");
        ctx.plaza.record_decision(approval_id, decision, note)
    }

    fn summary(&self, out: &Value) -> String {
        format!("Recommendation recorded on approval {}; the owner decides.", out["approval_id"].as_str().unwrap_or("?"))
    }
}
