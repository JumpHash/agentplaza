//! The operate tools: they act within the plaza's own rules. Each goes through the same service the HTTP API uses.

use serde_json::{Value, json};

use super::{Ctx, Tool, version_arg};
use crate::hub_mcp::auth::Scope;

pub struct MessageOrchestrator;
pub struct TellAgent;
pub struct StopAgent;
pub struct PromptPropose;
/// A workstation tool for owner-token clients, by its name: the same input as the agents' (`stations::tools`), run as
/// the owner through the same choke point (`stations::act`).
pub struct StationTool(pub &'static str);

impl Tool for StationTool {
    fn name(&self) -> &'static str {
        self.0
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        crate::stations::tools::spec(self.0).map_or("", |(description, _)| description)
    }

    fn input_schema(&self) -> Value {
        crate::stations::tools::spec(self.0)
            .map_or_else(|| json!({"type": "object"}), |(_, schema)| schema)
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        ctx.plaza.station_call(self.0, args)
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "{} returned {} bytes of JSON.",
            self.0,
            out.to_string().len()
        )
    }
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn cid_arg(args: &Value) -> Result<u64, String> {
    args.get("cid")
        .and_then(Value::as_u64)
        .ok_or_else(|| "cid must be a whole number".to_string())
}

impl Tool for PromptPropose {
    fn name(&self) -> &'static str {
        "prompt_propose"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "Adds a version to a prompt, authored over MCP. It never publishes; a version goes live only through a govern client or the owner."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "body": { "type": "string", "minLength": 1 },
                "note": { "type": "string" },
                "base_version": { "type": "integer", "minimum": 1 },
            },
            "required": ["id", "body", "base_version"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "version": { "type": "integer" },
                "created": { "type": "boolean" },
                "score": { "type": "integer" },
                "errors": { "type": "integer" },
            },
            "required": ["version", "created", "score", "errors"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let note = match args.get("note") {
            None | Some(Value::Null) => "",
            Some(value) => value.as_str().ok_or("note must be a string")?,
        };
        ctx.plaza.prompt_propose(
            text_arg(args, "id")?,
            text_arg(args, "body")?,
            note,
            version_arg(args, "base_version")?,
        )
    }

    fn summary(&self, out: &Value) -> String {
        if out["created"] == true {
            format!(
                "Added version {} with score {}.",
                out["version"], out["score"]
            )
        } else {
            format!("Same body as version {}; nothing added.", out["version"])
        }
    }
}

pub struct SkillPropose;

impl Tool for SkillPropose {
    fn name(&self) -> &'static str {
        "skill_propose"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "Adds a version to a skill, authored over MCP. It never publishes; a version goes live only through a govern client or the owner."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "skill_md": { "type": "string", "minLength": 1 },
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "minLength": 1 },
                            "content_b64": { "type": "string" },
                            "sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
                            "executable": { "type": "boolean" },
                        },
                        "required": ["path"],
                        "additionalProperties": false,
                    },
                },
                "base_version": { "type": "integer", "minimum": 1 },
                "note": { "type": "string" },
            },
            "required": ["id", "skill_md", "files", "base_version"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "version": { "type": "integer" },
                "score": { "type": "integer" },
                "errors": { "type": "integer" },
                "warnings": { "type": "integer" },
            },
            "required": ["version", "score", "errors"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let note = match args.get("note") {
            None | Some(Value::Null) => "",
            Some(value) => value.as_str().ok_or("note must be a string")?,
        };
        let files = args
            .get("files")
            .and_then(Value::as_array)
            .ok_or("files must be an array")?;
        ctx.plaza.skill_propose(
            text_arg(args, "id")?,
            text_arg(args, "skill_md")?,
            files,
            note,
            version_arg(args, "base_version")?,
        )
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Added skill version {} with score {}.",
            out["version"], out["score"]
        )
    }
}

impl Tool for MessageOrchestrator {
    fn name(&self) -> &'static str {
        "message_orchestrator"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "Sends the owner's message to the orchestrator, as a chat in the owner's thread."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "text": { "type": "string", "minLength": 1 } },
            "required": ["text"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "work": { "type": "integer" }, "cid": { "type": "integer" } },
            "required": ["work", "cid"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        ctx.plaza.message_orchestrator(text_arg(args, "text")?)
    }

    fn summary(&self, out: &Value) -> String {
        format!(
            "Message queued for the orchestrator as work {}.",
            out["work"]
        )
    }
}

impl Tool for TellAgent {
    fn name(&self) -> &'static str {
        "tell_agent"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "Sends a note to one agent. The agent reads it with its next work."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cid": { "type": "integer", "minimum": 0 },
                "text": { "type": "string", "minLength": 1 },
            },
            "required": ["cid", "text"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "the node's answer to the note" })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        ctx.plaza
            .tell_agent(cid_arg(args)?, text_arg(args, "text")?)
    }

    fn summary(&self, _out: &Value) -> String {
        "Note sent to the agent.".to_string()
    }
}

impl Tool for StopAgent {
    fn name(&self) -> &'static str {
        "stop_agent"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "Stops one agent's current work. The agent stays on the roster."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cid": { "type": "integer", "minimum": 0 },
                "reason": { "type": "string", "minLength": 1 },
            },
            "required": ["cid", "reason"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "the node's answer to the stop" })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        ctx.plaza
            .stop_agent(cid_arg(args)?, text_arg(args, "reason")?)
    }

    fn summary(&self, _out: &Value) -> String {
        "The agent's work is stopped.".to_string()
    }
}

pub struct RestartAgent;

impl Tool for RestartAgent {
    fn name(&self) -> &'static str {
        "restart_agent"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "Restarts one agent's VM. Its current work is cancelled first; the agent stays on the roster."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "cid": { "type": "integer", "minimum": 0 } },
            "required": ["cid"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" }, "cid": { "type": "integer" } },
            "required": ["ok"],
        })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        ctx.plaza.restart_agent(cid_arg(args)?)
    }

    fn summary(&self, _out: &Value) -> String {
        "The agent's VM is restarting.".to_string()
    }
}

pub struct CreateTask;

impl Tool for CreateTask {
    fn name(&self) -> &'static str {
        "create_task"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "Files a task, the same as the console's New task: the budget rule applies, and the task is open for the plaza to claim."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "minLength": 1 },
                "acceptance": { "type": "string" },
                "role": { "type": "string" },
                "budget": { "type": "integer", "minimum": 0 },
            },
            "required": ["title"],
            "additionalProperties": false,
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "description": "the task as filed" })
    }

    fn call(&self, ctx: &Ctx<'_>, args: &Value) -> Result<Value, String> {
        let mut request = json!({ "goal": text_arg(args, "title")? });
        for key in ["acceptance", "role", "budget"] {
            if let Some(value) = args.get(key) {
                request[key] = value.clone();
            }
        }
        ctx.plaza.create_task(&request)
    }

    fn summary(&self, _out: &Value) -> String {
        "Task filed.".to_string()
    }
}
