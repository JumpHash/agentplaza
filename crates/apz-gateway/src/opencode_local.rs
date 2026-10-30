//! OpenCode v2 on the hub's machine, as a model adapter rather than a host coding agent.
//!
//! Each call owns a fresh `opencode serve` child, private home/config/data/workspace, and one
//! session. Native tools and integrations are denied before a prompt is admitted. The model
//! returns a JSON envelope naming only the request's AgentPlaza tools; the ordinary harness
//! executes those calls and returns their results on the next request. No guest workspace,
//! inherited MCP server, plugin, shell permission, or host login is exposed to the model.

use std::collections::HashSet;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, StopReason, Usage};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::http::{Http, Req};
use crate::local_process::ManagedChild;
use crate::{Ctx, GatewayError, ModelInfo};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(900);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);

const PROTOCOL: &str = "You are an AgentPlaza model behind its tool gateway. Only the AgentPlaza tools listed below exist for this conversation. Do not use OpenCode's native tools. To request an action, return a tool_use block; AgentPlaza will execute it under its own policy and send a tool_result in the next conversation turn. Return exactly one JSON object, without Markdown fences, with this shape: {\"content\":[{\"type\":\"text\",\"text\":\"your response\"},{\"type\":\"tool_use\",\"name\":\"an offered tool\",\"input\":{}}]}. Include only the blocks needed, in their intended order. Tool input must match the offered JSON schema. Never invent results for a requested tool. The conversation below is an ordered JSON transcript; continue its final turn.";

/// Only explicitly configured models are advertised; a listing makes no model call.
pub(crate) fn list_models(ctx: &Ctx<'_>) -> Result<Vec<ModelInfo>, GatewayError> {
    let models = &ctx.p.models;
    check_models(models, None)?;
    for model in models {
        upstream(model)?;
    }
    let service = Service::start(ctx, &models[0], None)?;
    // Starting and checking the private API proves the installed executable speaks v2, without
    // spending provider tokens. Actual provider access is checked by the first requested call.
    service.call("GET", "/api/config", None)?;
    Ok(models.iter().cloned().map(ModelInfo::new).collect())
}

pub(crate) fn complete(
    ctx: &Ctx<'_>,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    check_models(&ctx.p.models, Some(model))?;
    let service = Service::start(ctx, model, Some(req))?;
    run_turn(&service, req, model, sink)
}

fn run_turn(
    service: &Service,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let (provider, upstream_model) = upstream(model)?;
    let session = service.call(
        "POST",
        "/api/session",
        Some(&json!({
            "title": "AgentPlaza model turn",
            "agent": "plaza",
            "model": model_selection(provider, upstream_model, req.effort.as_deref()),
            "location": {"directory": service.root.join("workspace")},
            "permissions": deny_permissions(),
        })),
    )?;
    let id = session["data"]["id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        .ok_or_else(|| GatewayError::decode("local OpenCode did not return a session id"))?;
    // Check enforcement, not just accepted configuration. A changed/older OpenCode build must
    // refuse the call before it can see a prompt, rather than silently regain host tools.
    for action in ["shell", "read", "edit", "subagent", "webfetch"] {
        let permission = service.call(
            "POST",
            &format!("/api/session/{id}/permission"),
            Some(&json!({"action":action,"resources":["*"]})),
        )?;
        if permission["data"]["effect"] != "deny" {
            return Err(GatewayError::Unsupported(format!(
                "local OpenCode did not deny native {action} access"
            )));
        }
    }
    service.call(
        "POST",
        &format!("/api/session/{id}/prompt"),
        Some(&json!({"text": transcript(req)?, "resume": true})),
    )?;
    let response = wait_for_answer(req, &service.nonce, Instant::now() + CALL_TIMEOUT, || {
        service.call(
            "GET",
            &format!("/api/session/{id}/message?limit=100&order=asc"),
            None,
        )
    })?;
    for (index, block) in response.content.iter().enumerate() {
        match block {
            Block::Text { text } => sink(Delta::Text { text: text.clone() }),
            Block::ToolUse {
                id, name, input, ..
            } => {
                sink(Delta::ToolUse {
                    name: name.clone(),
                    index,
                    id: id.clone(),
                });
                sink(Delta::ToolInput {
                    index,
                    json: input.to_string(),
                });
                sink(Delta::BlockEnd { index });
            }
            _ => {}
        }
    }
    Ok(response)
}

fn wait_for_answer(
    req: &ChatRequest,
    nonce: &str,
    deadline: Instant,
    mut fetch: impl FnMut() -> Result<Value, GatewayError>,
) -> Result<ChatResponse, GatewayError> {
    loop {
        if Instant::now() >= deadline {
            return Err(GatewayError::Transport(
                "local OpenCode model turn timed out".into(),
            ));
        }
        if let Some(response) = completed(req, &fetch()?, nonce)? {
            return Ok(response);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Each instance holds one upstream's scoped key. Never forward it to another upstream because
/// a caller chose an unconfigured model or the owner's model list mixed credential domains.
fn check_models(models: &[String], requested: Option<&str>) -> Result<(), GatewayError> {
    let first = models.first().ok_or_else(|| {
        GatewayError::Config(
            "opencode-local needs explicit provider/model entries in models".into(),
        )
    })?;
    let (provider, _) = upstream(first)?;
    for model in models {
        if upstream(model)?.0 != provider {
            return Err(GatewayError::Config(
                "each opencode-local instance must use only one upstream provider and its scoped key".into()
            ));
        }
    }
    if requested.is_some_and(|model| !models.iter().any(|offered| offered == model)) {
        return Err(GatewayError::Config(
            "the requested local OpenCode model is not configured for this instance".into(),
        ));
    }
    Ok(())
}

fn upstream(model: &str) -> Result<(&str, &str), GatewayError> {
    match model.split_once('/') {
        Some((provider @ ("deepseek" | "opencode"), model))
            if !model.is_empty()
                && model
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c)) =>
        {
            Ok((provider, model))
        }
        _ => Err(GatewayError::Config(
            "opencode-local models must name deepseek/<model> or opencode/<model>".into(),
        )),
    }
}

fn deny_permissions() -> Value {
    json!([{"action":"*","resource":"*","effect":"deny"}])
}

fn model_selection(provider: &str, model: &str, effort: Option<&str>) -> Value {
    let mut value = json!({"providerID":provider,"id":model});
    if let Some(effort) = effort {
        value["variant"] = json!(effort);
    }
    value
}

fn configuration(req: Option<&ChatRequest>) -> Value {
    let mut system = PROTOCOL.to_string();
    if let Some(req) = req {
        if let Some(prompt) = req.system_text() {
            system.push_str("\n\nAgentPlaza system instructions:\n");
            system.push_str(&prompt);
        }
        system.push_str("\n\nOffered AgentPlaza tools:\n");
        system.push_str(&json!(req.tools).to_string());
    }
    let mut config = json!({
        "permissions": deny_permissions(),
        "policies": [
            {"action":"tool.use","resource":"*","effect":"deny"},
            {"action":"integration.use","resource":"*","effect":"deny"}
        ],
        "default_agent":"plaza",
        // A limit of one makes OpenCode inject a conflicting "maximum steps reached, text
        // only" assistant message before the first request. With no native tools available,
        // the ordinary first answer still ends the turn; two leaves that first turn intact.
        "agents":{"plaza":{"system":system,"mode":"primary","steps":2,"permissions":deny_permissions()}},
        "plugins":[],"mcp":{"servers":{}},"skills":[],"instructions":[],
        "snapshots":false,"formatter":false,"lsp":false,"warming":false,
        "update":"disable","share":"disabled"
    });
    if let Some(req) = req {
        // Both supported upstreams speak OpenAI-compatible JSON mode. Transport syntax must
        // not depend on whether the user requested a plain-text answer (provider health checks
        // request a single word). The strict envelope parser still validates its shape/tools.
        let mut body = json!({"response_format":{"type":"json_object"}});
        if req.max_tokens > 0 {
            body["max_tokens"] = json!(req.max_tokens);
        }
        if let Some(temperature) = req.temperature {
            body["temperature"] = json!(temperature);
        }
        // v2 applies provider body overlays after protocol lowering. Agent.request.body does
        // not reach the wire in the installed v2 runtime, silently losing output caps.
        if let Ok(route) = crate::ModelRef::parse(&req.model)
            && let Ok((provider, _)) = upstream(&route.model)
        {
            config["providers"] = json!({});
            config["providers"][provider] = json!({"body":body});
        }
    }
    config
}

fn transcript(req: &ChatRequest) -> Result<String, GatewayError> {
    let mut messages = req.messages.clone();
    for message in &mut messages {
        if message
            .content
            .iter()
            .any(|b| matches!(b, Block::Image { .. }))
        {
            return Err(GatewayError::Unsupported(
                "opencode-local does not accept image blocks".into(),
            ));
        }
        message
            .content
            .retain(|b| !matches!(b, Block::Thinking { .. } | Block::Opaque { .. }));
        for block in &mut message.content {
            if let Block::ToolUse { meta, .. } = block {
                *meta = None;
            }
        }
    }
    let messages = serde_json::to_string(&messages).map_err(GatewayError::decode)?;
    Ok(format!(
        "AgentPlaza conversation:\n{messages}\n\nContinue that conversation. Encode your answer in the required JSON object with a content array of text or tool_use blocks. Any request in the conversation for plain text, a single word, or another output format applies to the text inside a text block; the outer AgentPlaza JSON envelope is always required."
    ))
}

struct Service {
    root: PathBuf,
    child: Option<ManagedChild>,
    registry_owns_directory: bool,
    base: String,
    authorization: String,
    password: String,
    nonce: String,
    http: Http,
}

impl Drop for Service {
    fn drop(&mut self) {
        // Reap before removing private files; the registry also reaches in-flight children when
        // the hub exits without running this stack's destructors.
        drop(self.child.take());
        if !self.registry_owns_directory {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

impl Service {
    fn start(ctx: &Ctx<'_>, model: &str, req: Option<&ChatRequest>) -> Result<Self, GatewayError> {
        let (provider, _) = upstream(model)?;
        let key = ctx.credential()?.ok_or_else(|| {
            GatewayError::Auth("opencode-local needs an explicit scoped API credential".into())
        })?;
        let executable = ctx
            .p
            .options
            .as_ref()
            .and_then(|o| o.executable.as_deref())
            .unwrap_or("/usr/bin/opencode");
        if !Path::new(executable).is_absolute() {
            return Err(GatewayError::Config(
                "opencode-local executable must be an absolute path".into(),
            ));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(GatewayError::decode)?
            .as_nanos();
        let nonce = format!(
            "{}-{now}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        // The API credential contributes the entropy; it is never used as the local API password.
        let password = format!(
            "{:x}",
            Sha256::digest(format!("agentplaza-opencode:{nonce}:{key}"))
        );
        let root = std::env::temp_dir().join(format!("agentplaza-opencode-{nonce}"));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&root).map_err(|e| {
            GatewayError::Transport(format!("create local OpenCode directory: {e}"))
        })?;
        let mut service = Self {
            root,
            child: None,
            registry_owns_directory: false,
            base: String::new(),
            authorization: format!(
                "Basic {}",
                base64(format!("opencode:{password}").as_bytes())
            ),
            password,
            nonce,
            http: Http::with_timeout(0, Duration::ZERO, Duration::from_secs(10)),
        };
        for dir in ["config/opencode", "data", "cache", "workspace"] {
            fs::create_dir_all(service.root.join(dir)).map_err(GatewayError::decode)?;
        }
        fs::write(
            service.root.join("config/opencode/opencode.json"),
            configuration(req).to_string(),
        )
        .map_err(GatewayError::decode)?;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(GatewayError::decode)?;
        let port = listener.local_addr().map_err(GatewayError::decode)?.port();
        drop(listener);
        service.base = format!("http://127.0.0.1:{port}");
        let mut command = Command::new(executable);
        command
            .args([
                "serve",
                "--hostname",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .current_dir(service.root.join("workspace"))
            .env_clear()
            .env(
                "PATH",
                ctx.env("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .env("HOME", &service.root)
            .env("XDG_CONFIG_HOME", service.root.join("config"))
            .env("XDG_DATA_HOME", service.root.join("data"))
            .env("XDG_CACHE_HOME", service.root.join("cache"))
            .env("OPENCODE_SERVER_PASSWORD", &service.password)
            .env(
                if provider == "deepseek" {
                    "DEEPSEEK_API_KEY"
                } else {
                    "OPENCODE_API_KEY"
                },
                key,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command
            .spawn()
            .map_err(|e| GatewayError::Transport(format!("start local OpenCode: {e}")))?;
        // This is the fresh private temp root above, never options.working_directory.
        // Transfer even on a shutdown rejection: only the registry knows whether reaping succeeded.
        service.registry_owns_directory = true;
        service.child = Some(
            ctx.gw
                .local_processes
                .track_owned(child, service.root.clone())?,
        );
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(schema) = service.call("GET", "/openapi.json", None)
                && schema["paths"]["/api/session/{sessionID}/prompt"].is_object()
            {
                break;
            }
            if service
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok())
                .flatten()
                .is_some()
            {
                return Err(GatewayError::Transport(
                    "local OpenCode exited before its API was ready".into(),
                ));
            }
            if Instant::now() >= deadline {
                return Err(GatewayError::Transport(
                    "local OpenCode v2 API did not become ready".into(),
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(service)
    }

    fn call(
        &self,
        method: &'static str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, GatewayError> {
        let mut request = Req::raw(
            method,
            format!("{}{path}", self.base),
            body.map_or_else(Vec::new, |b| b.to_string().into_bytes()),
        );
        request.set_header("content-type", "application/json");
        request = request.secret_header("authorization", &self.authorization, &self.password);
        self.http.json(&request).map_err(|error| match error {
            // The local service can report upstream credentials in errors. Its error body is not
            // part of the model protocol and must not escape into an agent transcript.
            GatewayError::Http {
                status, retryable, ..
            } => GatewayError::Http {
                status,
                retryable,
                body: "local OpenCode API rejected the request".into(),
            },
            _ => GatewayError::Transport(
                "local OpenCode API response was unavailable or invalid".into(),
            ),
        })
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for part in bytes.chunks(3) {
        let a = part[0];
        let b = part.get(1).copied().unwrap_or(0);
        let c = part.get(2).copied().unwrap_or(0);
        out.push(TABLE[(a >> 2) as usize] as char);
        out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        out.push(if part.len() > 1 {
            TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if part.len() > 2 {
            TABLE[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    content: Vec<AnswerBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum AnswerBlock {
    Text { text: String },
    ToolUse { name: String, input: Value },
}

fn completed(
    req: &ChatRequest,
    messages: &Value,
    nonce: &str,
) -> Result<Option<ChatResponse>, GatewayError> {
    let messages = messages["data"]
        .as_array()
        .ok_or_else(|| GatewayError::decode("local OpenCode messages must be an array"))?;
    let Some(idle) = messages.iter().rev().find(|m| m["type"] == "idle") else {
        return Ok(None);
    };
    if idle["outcome"] != "succeeded" {
        return Err(GatewayError::Provider {
            message: "local OpenCode model turn failed or was interrupted".into(),
            retryable: false,
        });
    }
    let mut usage = Usage::default();
    let mut text = String::new();
    for message in messages.iter().filter(|m| m["type"] == "assistant") {
        if !message["error"].is_null() {
            return Err(GatewayError::Provider {
                message: "local OpenCode returned a model error".into(),
                retryable: false,
            });
        }
        if message["finish"] == "length" {
            return Err(GatewayError::decode(
                "local OpenCode exhausted its output limit before completing the tool envelope",
            ));
        }
        let tokens = &message["tokens"];
        usage.input_tokens += tokens["input"].as_u64().unwrap_or(0);
        // OpenCode's session API splits visible output and reasoning into disjoint
        // counts. AgentPlaza's output usage includes both, as provider billing does.
        usage.output_tokens += tokens["output"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(tokens["reasoning"].as_u64().unwrap_or(0));
        usage.cache_read_tokens += tokens["cache"]["read"].as_u64().unwrap_or(0);
        usage.cache_write_tokens += tokens["cache"]["write"].as_u64().unwrap_or(0);
        let parts = message["content"].as_array().ok_or_else(|| {
            GatewayError::decode("local OpenCode assistant content must be an array")
        })?;
        for part in parts {
            match part["type"].as_str() {
                Some("text") => {
                    text.push_str(part["text"].as_str().ok_or_else(|| {
                        GatewayError::decode("local OpenCode text is not a string")
                    })?)
                }
                Some("reasoning") => {}
                _ => return Err(GatewayError::Unsupported(
                    "local OpenCode attempted a native tool instead of an AgentPlaza tool envelope"
                        .into(),
                )),
            }
        }
    }
    if text.len() > MAX_RESPONSE_BYTES {
        return Err(GatewayError::decode(
            "local OpenCode tool envelope is too large",
        ));
    }
    let envelope: Envelope = serde_json::from_str(&text).map_err(|_| {
        GatewayError::decode("local OpenCode did not return a valid AgentPlaza JSON tool envelope")
    })?;
    if envelope.content.is_empty() || envelope.content.len() > 64 {
        return Err(GatewayError::decode(
            "local OpenCode tool envelope must contain 1 to 64 blocks",
        ));
    }
    let offered: HashSet<_> = req.tools.iter().map(|tool| tool.name.as_str()).collect();
    let mut content = Vec::new();
    for (index, block) in envelope.content.into_iter().enumerate() {
        content.push(match block {
            AnswerBlock::Text { text } => Block::Text { text },
            AnswerBlock::ToolUse { name, input } => {
                if !offered.contains(name.as_str()) || !input.is_object() {
                    return Err(GatewayError::decode(
                        "local OpenCode requested an unoffered tool or non-object tool input",
                    ));
                }
                Block::ToolUse {
                    id: format!("oc-{nonce}-{index}"),
                    name,
                    input,
                    meta: None,
                }
            }
        });
    }
    let stop = if content.iter().any(|b| matches!(b, Block::ToolUse { .. })) {
        StopReason::ToolUse
    } else {
        StopReason::EndTurn
    };
    Ok(Some(ChatResponse {
        // The common gateway adds its instance id exactly once. The adapter returns only the
        // upstream provider/model, just as the remote adapters return their upstream model id.
        model: crate::ModelRef::parse(&req.model)?.model,
        content,
        stop,
        usage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_proto::chat::Message;

    fn request() -> ChatRequest {
        serde_json::from_value(json!({
            "model":"local/deepseek/deepseek-flash",
            "system":"Use the project checkout in the VM.",
            "messages":[{"role":"user","content":[{"type":"text","text":"Read the README"}]}],
            "tools":[{"name":"read","description":"Read a guest file","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}]
        })).unwrap()
    }

    fn finished(text: &str) -> Value {
        json!({"data":[
            {"type":"assistant","content":[{"type":"text","text":text}],"tokens":{"input":11,"output":7,"cache":{"read":3,"write":2}},"finish":"stop"},
            {"type":"idle","outcome":"succeeded"}
        ]})
    }

    #[test]
    fn only_hub_tools_are_returned_with_original_inputs_and_usage() {
        let req = request();
        let response = completed(&req, &finished(r#"{"content":[{"type":"text","text":"Reading it."},{"type":"tool_use","name":"read","input":{"path":"README.md"}}]}"#), "unique").unwrap().unwrap();
        assert_eq!(response.stop, StopReason::ToolUse);
        assert_eq!(response.model, "deepseek/deepseek-flash");
        assert_eq!(
            response.tool_uses().collect::<Vec<_>>(),
            [("oc-unique-1", "read", &json!({"path":"README.md"}))]
        );
        assert_eq!(
            response.usage,
            Usage {
                input_tokens: 11,
                output_tokens: 7,
                cache_read_tokens: 3,
                cache_write_tokens: 2
            }
        );
    }

    #[test]
    fn session_reasoning_tokens_are_included_in_billable_output() {
        let mut messages = finished(r#"{"content":[{"type":"text","text":"ok"}]}"#);
        // The installed v2 runtime reports an upstream completion_tokens=7 with
        // reasoning_tokens=4 as these separate visible/reasoning counts.
        messages["data"][0]["tokens"]["output"] = json!(3);
        messages["data"][0]["tokens"]["reasoning"] = json!(4);
        let response = completed(&request(), &messages, "usage").unwrap().unwrap();
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(response.usage.input_tokens, 11);
        assert_eq!(response.usage.cache_read_tokens, 3);
    }

    #[test]
    fn refusal_invalid_json_unknown_tools_and_native_tools_do_not_fall_back() {
        let req = request();
        for body in [
            "I cannot comply",
            "```json\n{}\n```",
            r#"{"content":[{"type":"tool_use","name":"shell","input":{}}]}"#,
            r#"{"content":[{"type":"tool_use","name":"read","input":[]}]}"#,
        ] {
            assert!(completed(&req, &finished(body), "n").is_err());
        }
        let mut native = finished("{}");
        native["data"][0]["content"] = json!([{"type":"tool","name":"shell"}]);
        assert!(matches!(
            completed(&req, &native, "n"),
            Err(GatewayError::Unsupported(_))
        ));
        let mut failed = finished("{}");
        failed["data"][1]["outcome"] = json!("failed");
        assert!(completed(&req, &failed, "n").is_err());
        let mut capped = finished(r#"{"content":[{"type":"text","text":"partial"}]}"#);
        capped["data"][0]["finish"] = json!("length");
        assert!(completed(&req, &capped, "n").is_err());
    }

    #[test]
    fn replay_keeps_tool_result_identity_and_discards_foreign_reasoning() {
        let mut req = request();
        req.messages.push(Message::assistant(vec![
            Block::ToolUse {
                id: "c1".into(),
                name: "read".into(),
                input: json!({"path":"README.md"}),
                meta: Some(json!({"kind":"other","secret":"opaque"})),
            },
            Block::Thinking {
                text: "private".into(),
            },
        ]));
        req.messages.push(Message {
            role: apz_proto::chat::Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "c1".into(),
                content: "File content".into(),
                is_error: false,
            }],
        });
        let text = transcript(&req).unwrap();
        assert!(text.contains("File content") && text.contains("c1"));
        assert!(!text.contains("opaque") && !text.contains("private"));
    }

    #[test]
    fn native_tools_and_integrations_are_denied_and_model_identity_is_explicit() {
        let config = configuration(Some(&request()));
        assert_eq!(config["permissions"], deny_permissions());
        assert_eq!(
            config["policies"],
            json!([
                {"action":"tool.use","resource":"*","effect":"deny"},
                {"action":"integration.use","resource":"*","effect":"deny"}
            ])
        );
        assert_eq!(config["plugins"], json!([]));
        assert_eq!(config["mcp"], json!({"servers":{}}));
        assert_eq!(
            config["providers"]["deepseek"]["body"]["response_format"],
            json!({"type":"json_object"})
        );
        assert_eq!(config["agents"]["plaza"]["steps"], 2);
        assert_eq!(
            upstream("opencode/glm-5.3").unwrap(),
            ("opencode", "glm-5.3")
        );
        assert!(upstream("deepseek/../../secret").is_err());
        assert_eq!(
            model_selection("deepseek", "deepseek-flash", Some("high")),
            json!({"providerID":"deepseek","id":"deepseek-flash","variant":"high"})
        );
        assert_eq!(base64(b"opencode:test"), "b3BlbmNvZGU6dGVzdA==");
    }

    #[test]
    fn wrong_upstream_and_unconfigured_models_never_receive_the_scoped_key() {
        assert!(
            check_models(
                &["deepseek/deepseek-flash".into()],
                Some("opencode/glm-5.3")
            )
            .is_err()
        );
        assert!(
            check_models(
                &["deepseek/deepseek-flash".into(), "opencode/glm-5.3".into()],
                None
            )
            .is_err()
        );
        assert!(check_models(&[], None).is_err());
        assert!(check_models(&["opencode/glm-5.3".into()], Some("opencode/glm-5.3")).is_ok());
    }

    #[test]
    fn timeout_and_malformed_server_responses_are_errors() {
        let req = request();
        assert!(matches!(
            wait_for_answer(&req, "n", Instant::now(), || panic!(
                "expired call must not fetch"
            )),
            Err(GatewayError::Transport(_))
        ));
        assert!(matches!(
            wait_for_answer(&req, "n", Instant::now() + Duration::from_secs(1), || Ok(
                json!({"html":"wrong API"})
            )),
            Err(GatewayError::Decode(_))
        ));
    }

    /// A real local binary, with an entirely local fake upstream and a dummy credential. This
    /// catches v1/v2 API drift and proves the model receives no native tools. It never calls a
    /// paid provider; run explicitly where OpenCode v2 is installed.
    #[test]
    #[ignore = "requires /usr/bin/opencode v2; upstream is an offline HTTP fixture"]
    fn local_binary_roundtrip_and_explicit_shutdown() {
        use crate::testutil::{FakeServer, Reply};
        use crate::{Caller, Gateway, ProviderConfig};

        let envelope =
            r#"{"content":[{"type":"tool_use","name":"read","input":{"path":"README.md"}}]}"#;
        let chunk = json!({
            "id":"offline", "object":"chat.completion.chunk", "created":1,
            "model":"deepseek-flash",
            "choices":[{"index":0,"delta":{"content":envelope},"finish_reason":null}]
        });
        let end = json!({
            "id":"offline", "object":"chat.completion.chunk", "created":1,
            "model":"deepseek-flash",
            "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18,
                "completion_tokens_details":{"reasoning_tokens":4}}
        });
        let fake = FakeServer::start(vec![Reply::sse(format!(
            "data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n"
        ))]);
        let provider: ProviderConfig = serde_json::from_value(json!({
            "id":"local", "kind":"opencode-local", "auth":{"env":"OFFLINE_KEY"},
            "models":["deepseek/deepseek-flash"], "options":{"executable":"/usr/bin/opencode"}
        }))
        .unwrap();
        let gateway = Gateway::builder()
            .providers(vec![provider.clone()])
            .env(|name| (name == "OFFLINE_KEY").then(|| "offline-fixture-only".into()))
            .build();
        let ctx = Ctx {
            gw: &gateway.inner,
            p: &provider,
            caller: &Caller::Hub,
            listing: false,
        };
        let mut req = request();
        req.max_tokens = 256;
        let mut service = Service::start(&ctx, "deepseek/deepseek-flash", Some(&req)).unwrap();
        let root = service.root.clone();
        let mut config = configuration(Some(&req));
        config["providers"]["deepseek"]["settings"] = json!({
            "baseURL":format!("{}/v1", fake.url()),"apiKey":"offline-fixture-only"
        });
        // No instance has loaded yet: startup only fetched the API schema.
        fs::write(
            root.join("config/opencode/opencode.json"),
            config.to_string(),
        )
        .unwrap();
        let response = run_turn(&service, &req, "deepseek/deepseek-flash", &mut |_| {}).unwrap();
        assert_eq!(response.stop, StopReason::ToolUse);
        assert_eq!(response.usage.input_tokens, 11);
        assert_eq!(response.usage.output_tokens, 7);
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/chat/completions");
        assert!(
            requests[0].json()["tools"]
                .as_array()
                .is_none_or(Vec::is_empty)
        );
        assert_eq!(requests[0].json()["model"], "deepseek-flash");
        assert_eq!(
            requests[0].json()["response_format"],
            json!({"type":"json_object"})
        );
        assert_eq!(requests[0].json()["max_tokens"], 256);
        let sent = requests[0].json()["messages"].to_string();
        assert!(sent.contains("Only the AgentPlaza tools listed below exist"));
        assert!(sent.contains("the outer AgentPlaza JSON envelope is always required"));
        assert!(!sent.contains("MAXIMUM STEPS REACHED"));
        gateway.shutdown_local_providers();
        assert!(
            service
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_some()
        );
        assert!(
            !root.exists(),
            "explicit shutdown must remove the private home before Service drops"
        );
        drop(service);
    }
}
