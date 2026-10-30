//! Codex is a host-side model transport, never a second executor. Its own shell, integrations,
//! skills and agents are disabled. Dynamic tool requests return to the existing guest harness.
//! The child owns its login (including renewal); the gateway never reads or copies credentials.
//!
//! A continuation is bound to the caller and an exact append-only transcript. A lost child fails
//! closed: replaying a pending tool in a fresh conversation could repeat a side effect.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Auth, Caller, Ctx, GatewayError, ModelInfo, ProviderConfig};

const TAG: &str = "codex-app-server";
const TIMEOUT: Duration = Duration::from_secs(20 * 60);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const MAX_SESSIONS: usize = 64;
const MAX_LINE: usize = 16 * 1024 * 1024;
// Shared with the guest harness contract in agent.rs::compress.
const SUMMARY_HEAD: &str = "Summary of the work so far (earlier turns were compressed):";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn failure(message: &str) -> GatewayError {
    // RPC error bodies and stderr are deliberately not surfaced: a third-party process can echo
    // account identifiers, configuration, or credentials. Call sites supply useful fixed messages.
    GatewayError::Provider {
        message: format!("Codex app-server: {message}"),
        retryable: false,
    }
}

pub(crate) struct Session {
    rpc: Rpc,
    caller: Caller,
    config: ProviderConfig,
    model: String,
    specification: [u8; 32],
    tools: Vec<apz_proto::chat::ToolSpec>,
    prefix: [u8; 32],
    prefix_len: usize,
    first_task: [u8; 32],
    last_response: [u8; 32],
    revision: u64,
    thread: String,
    turn: String,
    pending: Option<(Value, String)>,
    touched: Instant,
    total: Usage,
}

struct Rpc {
    _child: crate::local_process::ManagedChild,
    input: ChildStdin,
    output: Receiver<Result<Value, ()>>,
    next: u64,
    queued: Vec<Value>,
}

impl Rpc {
    fn start(
        config: &ProviderConfig,
        registry: &crate::local_process::Registry,
    ) -> Result<Self, GatewayError> {
        if config.auth != Auth::None
            || config.base_url.is_some()
            || !config.extra_headers.is_empty()
            || !config.extra_body.is_null()
        {
            return Err(failure(
                "local login requires auth=none and no HTTP overrides",
            ));
        }
        let options = config.options.as_ref();
        let executable = options
            .and_then(|o| o.executable.as_deref())
            .unwrap_or("codex");
        let cwd = options.and_then(|o| o.working_directory.as_deref());
        let mut cmd = Command::new(executable);
        cmd.args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear();
        // Preserve login/config locations without passing provider keys, GH credentials or the
        // hub's owner token into this process. Codex alone handles its existing account.
        for name in [
            "HOME",
            "USERPROFILE",
            "PATH",
            "LANG",
            "SYSTEMROOT",
            "CODEX_HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            if let Some(value) = std::env::var_os(name) {
                cmd.env(name, value);
            }
        }
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        } else {
            cmd.current_dir(std::env::temp_dir());
        }
        for name in [
            "shell_tool",
            "apps",
            "plugins",
            "remote_plugin",
            "multi_agent",
            "hooks",
            "memories",
            "goals",
            "computer_use",
            "browser_use",
            "browser_use_external",
            "image_generation",
            "shell_snapshot",
            "skill_search",
            "sleep_tool",
        ] {
            cmd.args(["-c", &format!("features.{name}=false")]);
        }
        for setting in [
            "web_search=\"disabled\"",
            "skills.include_instructions=false",
            "skills.bundled.enabled=false",
            "project_doc_max_bytes=0",
            "approval_policy=\"never\"",
            "sandbox_mode=\"read-only\"",
        ] {
            cmd.args(["-c", setting]);
        }
        let mut child = cmd
            .spawn()
            .map_err(|_| failure("could not start the configured executable"))?;
        let Some(input) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure("child has no input pipe"));
        };
        let Some(output) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure("child has no output pipe"));
        };
        let (sent, received) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                // Bound a single line before allocating unbounded process output.
                let read = (&mut reader)
                    .take(MAX_LINE as u64 + 1)
                    .read_until(b'\n', &mut line);
                if !matches!(read, Ok(n) if n > 0 && n <= MAX_LINE) {
                    let _ = sent.send(Err(()));
                    break;
                }
                let parsed = serde_json::from_slice(&line).map_err(|_| ());
                let invalid = parsed.is_err();
                if sent.send(parsed).is_err() || invalid {
                    break;
                }
            }
        });
        let mut rpc = Self {
            _child: registry.track(child)?,
            input,
            output: received,
            next: 1,
            queued: Vec::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        rpc.call("initialize", json!({"clientInfo":{"name":"agentplaza","title":"AgentPlaza","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}), deadline)?;
        rpc.send(json!({"method":"initialized"}))?;
        let account = rpc.call("account/read", json!({"refreshToken":false}), deadline)?;
        if !matches!(
            account["account"]["type"].as_str(),
            Some("chatgpt" | "chatgptAuthTokens")
        ) {
            return Err(failure(
                "sign in to the configured Codex installation with ChatGPT first",
            ));
        }
        Ok(rpc)
    }

    fn send(&mut self, value: Value) -> Result<(), GatewayError> {
        serde_json::to_writer(&mut self.input, &value)
            .map_err(|_| failure("writing an RPC request failed"))?;
        self.input
            .write_all(b"\n")
            .and_then(|()| self.input.flush())
            .map_err(|_| failure("writing an RPC request failed"))
    }

    fn read(&mut self, deadline: Instant) -> Result<Value, GatewayError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(failure(
                "request deadline exceeded; the conversation was stopped",
            ));
        }
        self.output
            .recv_timeout(remaining)
            .map_err(|_| failure("process exited or request deadline exceeded"))?
            .map_err(|()| failure("process returned an invalid or oversized RPC message"))
    }

    fn call(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, GatewayError> {
        let id = self.next;
        self.next += 1;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        loop {
            let value = self.read(deadline)?;
            if value["id"].as_u64() == Some(id) && value.get("method").is_none() {
                if value.get("error").is_some() {
                    return Err(failure(&format!(
                        "{method} was rejected by the installed Codex; check its version and login"
                    )));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| failure("RPC response has no result"));
            }
            if value.get("id").is_some() && value.get("method").is_some() {
                // A resumed turn may request its next dynamic tool before the
                // start/steer acknowledgement. Keep it for advance(), where the
                // thread, namespace and advertised tool name are checked.
                if !matches!(method, "turn/start" | "turn/steer")
                    || value["method"].as_str() != Some("item/tool/call")
                {
                    return Err(failure(
                        "unexpected server request while awaiting an RPC response",
                    ));
                }
            }
            if self.queued.len() >= 128 {
                return Err(failure("too many events before an RPC response"));
            }
            self.queued.push(value);
        }
    }

    fn event(&mut self, deadline: Instant) -> Result<Value, GatewayError> {
        if !self.queued.is_empty() {
            return Ok(self.queued.remove(0));
        }
        self.read(deadline)
    }
}

pub(crate) fn list_models(ctx: &Ctx<'_>) -> Result<Vec<ModelInfo>, GatewayError> {
    let mut rpc = Rpc::start(ctx.p, &ctx.gw.local_processes)?;
    let answer = rpc.call(
        "model/list",
        json!({"limit":100}),
        Instant::now() + Duration::from_secs(30),
    )?;
    Ok(answer["data"]
        .as_array()
        .ok_or_else(|| failure("model/list returned no catalog"))?
        .iter()
        .filter_map(|m| {
            Some(ModelInfo {
                id: m["model"]
                    .as_str()
                    .or_else(|| m["id"].as_str())?
                    .to_string(),
                name: m["displayName"].as_str().map(str::to_string),
                context: None,
            })
        })
        .collect())
}

fn digest(value: &(impl serde::Serialize + ?Sized)) -> Result<[u8; 32], GatewayError> {
    Ok(Sha256::digest(serde_json::to_vec(value).map_err(GatewayError::decode)?).into())
}

fn marker(messages: &[Message]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .flat_map(|m| m.content.iter().rev())
        .find_map(|b| match b {
            Block::Opaque { kind, raw } if kind == TAG => raw["session"].as_str(),
            _ => None,
        })
}

// Deferred tool search only reveals a schema: it cannot add names, change an existing
// schema, or replace descriptions. Initial empty schemas admit arguments; the guest validates
// the revealed schema and refuses calls before a tool is fetched.
fn compatible_tools(
    before: &[apz_proto::chat::ToolSpec],
    after: &[apz_proto::chat::ToolSpec],
) -> bool {
    before.len() == after.len()
        && before.iter().zip(after).all(|(a, b)| {
            a.name == b.name && a.description == b.description && (a.deferred || a == b)
        })
}

/// A harness compaction changes the first message by adding/replacing exactly its summary,
/// drains older turns, and retains the most recent assistant response verbatim. Codex still owns
/// the original context and can compact that context natively; forward only messages following
/// that exact response, never the summary, old tool calls, or old results a second time.
fn compacted_prefix_len(
    session: &Session,
    messages: &[Message],
    id: &str,
) -> Result<Option<usize>, GatewayError> {
    let Some(first) = messages.first() else {
        return Ok(None);
    };
    let summaries = first
        .content
        .iter()
        .filter(|b| matches!(b,Block::Text{text} if text.starts_with(SUMMARY_HEAD)))
        .count();
    if first.role != Role::User || summaries != 1 || task_digest(first)? != session.first_task {
        return Ok(None);
    }
    let Some(last) = messages.iter().rposition(|m| m.content.iter().any(|b|matches!(b,Block::Opaque{kind,raw} if kind==TAG && raw["session"].as_str()==Some(id)))) else {return Ok(None);};
    if last + 1 >= session.prefix_len || digest(&messages[last])? != session.last_response {
        return Ok(None);
    }
    Ok(Some(last + 1))
}

fn task_digest(message: &Message) -> Result<[u8; 32], GatewayError> {
    let mut task = message.clone();
    task.content
        .retain(|b| !matches!(b,Block::Text{text} if text.starts_with(SUMMARY_HEAD)));
    digest(&task)
}

struct Reservation<'a> {
    sessions: &'a Mutex<HashMap<String, Option<Session>>>,
    id: String,
    retained: bool,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.retained {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.id);
        }
    }
}

pub(crate) fn complete(
    ctx: &Ctx<'_>,
    req: &ChatRequest,
    model: &str,
    sink: &mut dyn FnMut(Delta),
) -> Result<ChatResponse, GatewayError> {
    let deadline = Instant::now() + TIMEOUT;
    let specification = digest(&(req.system_text(), &req.effort))?;
    let continuation = marker(&req.messages).map(str::to_string);
    let id = continuation.clone().unwrap_or_else(|| {
        format!(
            "{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        )
    });
    let mut session = {
        let mut sessions = ctx.gw.codex.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| {
            s.as_ref()
                .is_none_or(|s| s.touched.elapsed() < IDLE_TIMEOUT)
        });
        if continuation.is_some() {
            let slot = sessions.get_mut(&id).ok_or_else(|| failure("conversation is no longer available; start a new chat without replaying completed actions"))?;
            let old = slot
                .as_mut()
                .ok_or_else(|| failure("another call is using this conversation"))?;
            if old.caller != *ctx.caller
                || old.config != *ctx.p
                || old.model != model
                || old.specification != specification
                || !compatible_tools(&old.tools, &req.tools)
            {
                return Err(failure(
                    "continuation does not belong to this caller, provider, model and tool specification",
                ));
            }
            if req.messages.len() < old.prefix_len
                || digest(&req.messages[..old.prefix_len])? != old.prefix
            {
                old.prefix_len =
                    compacted_prefix_len(old, &req.messages, &id)?.ok_or_else(|| {
                        failure("continuation history changed; refusing to replay tools")
                    })?;
            }
            slot.take()
        } else {
            if sessions.len() >= MAX_SESSIONS {
                return Err(failure("local conversation capacity reached"));
            }
            sessions.insert(id.clone(), None);
            None
        }
    };
    let mut reservation = Reservation {
        sessions: &ctx.gw.codex,
        id: id.clone(),
        retained: false,
    };
    if session.is_none() {
        session = Some(Session::new(ctx, req, model, specification, deadline)?);
    }
    let mut session = session.ok_or_else(|| failure("conversation initialization failed"))?;
    let mut response = session.advance(req, sink, deadline)?;
    session.revision += 1;
    response.content.push(Block::Opaque {
        kind: TAG.into(),
        raw: json!({"session":id,"revision":session.revision}),
    });
    session.last_response = digest(&Message::assistant(response.content.clone()))?;
    let mut prefix = req.messages.clone();
    prefix.push(Message::assistant(response.content.clone()));
    session.prefix = digest(&prefix)?;
    session.prefix_len = prefix.len();
    session.touched = Instant::now();
    ctx.gw
        .codex
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, Some(session));
    reservation.retained = true;
    Ok(response)
}

impl Session {
    fn new(
        ctx: &Ctx<'_>,
        req: &ChatRequest,
        model: &str,
        specification: [u8; 32],
        deadline: Instant,
    ) -> Result<Self, GatewayError> {
        if req.messages.iter().any(|m| m.role != Role::User) {
            return Err(failure(
                "a new local conversation must start with user messages",
            ));
        }
        let mut rpc = Rpc::start(ctx.p, &ctx.gw.local_processes)?;
        let catalog = rpc.call("model/list", json!({"limit":100}), deadline)?;
        let offered = catalog["data"]
            .as_array()
            .and_then(|ms| {
                ms.iter()
                    .find(|m| m["model"].as_str().or_else(|| m["id"].as_str()) == Some(model))
            })
            .ok_or_else(|| {
                failure("the requested model is absent from the installed Codex catalog")
            })?;
        if let Some(effort) = &req.effort {
            let supported = offered["supportedReasoningEfforts"]
                .as_array()
                .is_some_and(|es| {
                    es.iter()
                        .any(|e| e["reasoningEffort"].as_str() == Some(effort))
                });
            if !supported {
                return Err(failure(
                    "the installed Codex catalog does not support the requested reasoning effort",
                ));
            }
        }
        // Disable every configured MCP server as well as plugin/app discovery. These are per-thread
        // overrides; the owner's Codex configuration and other Codex sessions stay untouched.
        let config = rpc.call("config/read", json!({"includeLayers":false}), deadline)?;
        let mut overrides = serde_json::Map::new();
        if let Some(servers) = config["config"]["mcp_servers"].as_object() {
            for name in servers.keys() {
                overrides.insert(format!("mcp_servers.{name}.enabled"), json!(false));
            }
        }
        overrides.insert("model_reasoning_effort".into(), json!(req.effort));
        let tools: Vec<_> = req.tools.iter().map(|t| json!({"type":"function","name":t.name,"description":t.description,"inputSchema":t.input_schema,"deferLoading":false})).collect();
        let cwd = ctx
            .p
            .options
            .as_ref()
            .and_then(|o| o.working_directory.clone())
            .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
        let started = rpc.call("thread/start", json!({"model":model,"modelProvider":"openai","allowProviderModelFallback":false,"baseInstructions":req.system_text().unwrap_or_default(),"developerInstructions":"All work is performed through the supplied AgentPlaza tools. Do not execute host commands or use other integrations. Delegate using AgentPlaza's tools, not Codex subagents.","dynamicTools":if tools.is_empty() {json!([])} else {json!([{ "type":"namespace", "name":"agentplaza", "description":"Tools executed by the AgentPlaza guest harness", "tools":tools }])},"config":overrides,"environments":[],"cwd":cwd,"sandbox":"read-only","approvalPolicy":"never","ephemeral":true}), deadline)?;
        if started["model"].as_str() != Some(model) {
            return Err(failure("Codex changed the requested model"));
        }
        if let Some(effort) = &req.effort
            && started["reasoningEffort"].as_str() != Some(effort)
        {
            return Err(failure("Codex changed the requested reasoning effort"));
        }
        let thread = started["thread"]["id"]
            .as_str()
            .ok_or_else(|| failure("thread/start returned no thread ID"))?
            .to_string();
        Ok(Self {
            rpc,
            caller: ctx.caller.clone(),
            config: ctx.p.clone(),
            model: model.into(),
            specification,
            tools: req.tools.clone(),
            prefix: [0; 32],
            prefix_len: 0,
            first_task: req
                .messages
                .first()
                .map(task_digest)
                .transpose()?
                .unwrap_or_default(),
            last_response: [0; 32],
            revision: 0,
            thread,
            turn: String::new(),
            pending: None,
            touched: Instant::now(),
            total: Usage::default(),
        })
    }

    fn advance(
        &mut self,
        req: &ChatRequest,
        sink: &mut dyn FnMut(Delta),
        deadline: Instant,
    ) -> Result<ChatResponse, GatewayError> {
        let messages = &req.messages[self.prefix_len..];
        let mut input = Vec::new();
        let mut result = None;
        for message in messages {
            if message.role != Role::User {
                return Err(failure(
                    "unexpected assistant message after the continuation",
                ));
            }
            for block in &message.content {
                match block {
                    Block::Text { text } => input.push(json!({"type":"text","text":text})),
                    Block::Image { media_type, data } => input.push(
                        json!({"type":"image","url":format!("data:{media_type};base64,{data}")}),
                    ),
                    Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let Some((_, expected)) = &self.pending else {
                            return Err(failure("tool result has no pending call"));
                        };
                        if expected != tool_use_id || result.is_some() {
                            return Err(failure("tool result does not match the pending call"));
                        }
                        result = Some(
                            json!({"success":!is_error,"contentItems":[{"type":"inputText","text":content}]}),
                        );
                    }
                    _ => return Err(failure("unsupported content in a local conversation input")),
                }
            }
        }
        if let Some((id, _)) = self.pending.take() {
            let result = result.ok_or_else(|| failure("the pending tool call has no result"))?;
            self.rpc.send(json!({"id":id,"result":result}))?;
            if !input.is_empty() {
                self.rpc.call(
                    "turn/steer",
                    json!({"threadId":self.thread,"expectedTurnId":self.turn,"input":input}),
                    deadline,
                )?;
            }
        } else {
            if input.is_empty() {
                return Err(failure("a new turn has no input"));
            }
            let turn = self.rpc.call("turn/start", json!({"threadId":self.thread,"input":input,"model":self.model,"effort":req.effort,"environments":[]}), deadline)?;
            self.turn = turn["turn"]["id"]
                .as_str()
                .ok_or_else(|| failure("turn/start returned no turn ID"))?
                .to_string();
        }
        let mut content = Vec::new();
        let mut texts: Vec<(String, String)> = Vec::new();
        let before = self.total.clone();
        let stop = loop {
            let event = self.rpc.event(deadline)?;
            let method = event["method"].as_str().unwrap_or("");
            let params = &event["params"];
            if let Some(thread) = params["threadId"].as_str()
                && thread != self.thread
            {
                return Err(failure("event belongs to another thread"));
            }
            match method {
                "item/agentMessage/delta" => {
                    if let Some(text) = params["delta"].as_str() {
                        let id = params["itemId"].as_str().unwrap_or("").to_string();
                        if let Some((_, accumulated)) = texts.iter_mut().find(|(key, _)| *key == id)
                        {
                            accumulated.push_str(text);
                        } else {
                            texts.push((id, text.into()));
                        }
                        sink(Delta::Text { text: text.into() });
                    }
                }
                "item/reasoning/summaryTextDelta" => {
                    if let Some(text) = params["delta"].as_str() {
                        sink(Delta::Thinking { text: text.into() });
                    }
                }
                "thread/tokenUsage/updated" => {
                    let total = &params["tokenUsage"]["total"];
                    self.total = Usage {
                        input_tokens: total["inputTokens"]
                            .as_u64()
                            .unwrap_or(0)
                            .saturating_sub(total["cachedInputTokens"].as_u64().unwrap_or(0)),
                        output_tokens: total["outputTokens"].as_u64().unwrap_or(0),
                        cache_read_tokens: total["cachedInputTokens"].as_u64().unwrap_or(0),
                        cache_write_tokens: total["cacheWriteInputTokens"].as_u64().unwrap_or(0),
                    };
                }
                "item/tool/call" => {
                    let name = params["tool"]
                        .as_str()
                        .ok_or_else(|| failure("tool call has no name"))?;
                    if params["namespace"].as_str() != Some("agentplaza")
                        || !req.tools.iter().any(|t| t.name == name)
                    {
                        return Err(failure(
                            "Codex requested a tool outside this AgentPlaza request",
                        ));
                    }
                    let id = params["callId"]
                        .as_str()
                        .ok_or_else(|| failure("tool call has no call ID"))?
                        .to_string();
                    let rpc_id = event
                        .get("id")
                        .cloned()
                        .ok_or_else(|| failure("tool call has no RPC ID"))?;
                    let arguments = params
                        .get("arguments")
                        .cloned()
                        .ok_or_else(|| failure("tool call has no arguments"))?;
                    for (_, text) in std::mem::take(&mut texts) {
                        content.push(Block::Text { text });
                    }
                    sink(Delta::ToolUse {
                        name: name.into(),
                        index: content.len(),
                        id: id.clone(),
                    });
                    sink(Delta::ToolInput {
                        index: content.len(),
                        json: arguments.to_string(),
                    });
                    sink(Delta::BlockEnd {
                        index: content.len(),
                    });
                    content.push(Block::ToolUse {
                        id: id.clone(),
                        name: name.into(),
                        input: arguments,
                        meta: None,
                    });
                    self.pending = Some((rpc_id, id));
                    break StopReason::ToolUse;
                }
                "item/started" | "item/completed" => {
                    let item = &params["item"];
                    match item["type"].as_str() {
                        Some(
                            "agentMessage" | "reasoning" | "dynamicToolCall" | "userMessage"
                            | "contextCompaction" | "plan",
                        ) => {}
                        _ => {
                            return Err(failure(
                                "Codex attempted a built-in tool; local execution is not permitted",
                            ));
                        }
                    }
                    if method == "item/completed"
                        && item["type"] == "agentMessage"
                        && let Some(text) = item["text"].as_str()
                    {
                        let id = item["id"].as_str().unwrap_or("").to_string();
                        if let Some((_, accumulated)) = texts.iter_mut().find(|(key, _)| *key == id)
                        {
                            *accumulated = text.into();
                        } else {
                            sink(Delta::Text { text: text.into() });
                            texts.push((id, text.into()));
                        }
                    }
                }
                "turn/completed" => {
                    if params["turn"]["id"].as_str() != Some(&self.turn)
                        || params["turn"]["status"] != "completed"
                    {
                        return Err(failure("turn failed or was interrupted"));
                    }
                    for (_, text) in std::mem::take(&mut texts) {
                        content.push(Block::Text { text });
                    }
                    break StopReason::EndTurn;
                }
                "error" => {
                    return Err(failure(
                        "Codex reported a turn error; check the account and selected model",
                    ));
                }
                _ if event.get("id").is_some() && event.get("method").is_some() => {
                    self.rpc.send(json!({"id":event["id"],"error":{"code":-32601,"message":"Only AgentPlaza dynamic tools are permitted"}}))?;
                    return Err(failure(
                        "unexpected approval or built-in tool request was refused",
                    ));
                }
                _ => {}
            }
        };
        Ok(ChatResponse {
            model: self.model.clone(),
            content,
            stop,
            usage: Usage {
                input_tokens: self.total.input_tokens.saturating_sub(before.input_tokens),
                output_tokens: self
                    .total
                    .output_tokens
                    .saturating_sub(before.output_tokens),
                cache_read_tokens: self
                    .total
                    .cache_read_tokens
                    .saturating_sub(before.cache_read_tokens),
                cache_write_tokens: self
                    .total
                    .cache_write_tokens
                    .saturating_sub(before.cache_write_tokens),
            },
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{Gateway, ProviderKind};
    use apz_proto::chat::ToolSpec;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "apz-codex-protocol-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("codex");
            std::fs::write(&path, r##"#!/usr/bin/env python3
import json,sys
def send(x):
 print(json.dumps(x),flush=True)
def event(method,params):
 send({'method':method,'params':params})
def tool(number=1,name='lookup'):
 send({'id':'tool-rpc-'+str(number),'method':'item/tool/call','params':{'threadId':'thread-1','turnId':turn,'namespace':'agentplaza','tool':name,'callId':'call-'+str(number),'arguments':{'query':'chair' if number==1 else 'table'}}})
def finish():
 event('item/agentMessage/delta',{'threadId':'thread-1','itemId':'z','delta':'Done'})
 event('item/completed',{'threadId':'thread-1','item':{'type':'agentMessage','id':'z','text':'Done'}})
 event('thread/tokenUsage/updated',{'threadId':'thread-1','tokenUsage':{'total':{'inputTokens':30,'cachedInputTokens':8,'outputTokens':9}}})
 event('turn/completed',{'threadId':'thread-1','turn':{'id':turn,'status':'completed'}})
model='fixture'
turn='turn-1'
for line in sys.stdin:
 m=json.loads(line)
 method=m.get('method')
 p=m.get('params',{})
 ident=m.get('id')
 if method=='initialize':
  assert p['capabilities']['experimentalApi']
  send({'id':ident,'result':{}})
 elif method=='initialized': pass
 elif method=='account/read': send({'id':ident,'result':{'account':{'type':'chatgpt'}}})
 elif method=='model/list':
  send({'id':ident,'result':{'data':[{'id':n,'model':n,'displayName':n,'supportedReasoningEfforts':[{'reasoningEffort':'ultra'}]} for n in ['fixture','unknown-tool','builtin','stall','steer','interleaved-start','interleaved-steer','interleaved-unknown','interleaved-approval']]}})
 elif method=='config/read': send({'id':ident,'result':{'config':{'mcp_servers':{'private':{}}}}})
 elif method=='thread/start':
  assert p['environments']==[] and p['sandbox']=='read-only' and p['approvalPolicy']=='never'
  assert p['config']['mcp_servers.private.enabled'] is False
  assert p['allowProviderModelFallback'] is False
  assert p['dynamicTools'][0]['name']=='agentplaza'
  assert p['baseInstructions']=='Use the lookup tool.'
  assert 'features.shell_tool=false' in sys.argv and 'features.plugins=false' in sys.argv
  model=p['model']
  send({'id':ident,'result':{'model':model,'reasoningEffort':'ultra','thread':{'id':'thread-1'}}})
 elif method=='turn/start':
  assert p['effort']=='ultra' and p['environments']==[]
  if model in ['interleaved-start','interleaved-unknown']:
   tool(name='unknown' if model=='interleaved-unknown' else 'lookup')
   send({'id':ident,'result':{'turn':{'id':turn}}})
   continue
  if model=='interleaved-approval':
   send({'id':'approval-rpc','method':'item/commandExecution/requestApproval','params':{'threadId':'thread-1','turnId':turn}})
   send({'id':ident,'result':{'turn':{'id':turn}}})
   continue
  send({'id':ident,'result':{'turn':{'id':turn}}})
  if model=='stall': continue
  if model=='builtin':
   event('item/started',{'threadId':'thread-1','item':{'type':'commandExecution'}})
  else:
   event('thread/tokenUsage/updated',{'threadId':'thread-1','tokenUsage':{'total':{'inputTokens':10,'cachedInputTokens':3,'outputTokens':5}}})
   tool(name='unknown' if model=='unknown-tool' else 'lookup')
 elif method=='turn/steer':
  assert p['expectedTurnId']==turn and p['input']==[{'type':'text','text':'Prefer oak'}]
  if model=='interleaved-steer': tool(2)
  send({'id':ident,'result':{'turnId':turn}})
  if model=='steer': finish()
 elif ident=='tool-rpc-1':
  assert m['result']=={'success':True,'contentItems':[{'type':'inputText','text':'oak chair'}]}
  if model not in ['steer','interleaved-steer']: finish()
 elif ident=='tool-rpc-2':
  assert m['result']=={'success':True,'contentItems':[{'type':'inputText','text':'walnut table'}]}
  finish()
 else: send({'id':ident,'error':{'code':-32601,'message':'unsupported'}})
"##).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(dir)
        }

        fn gateway(&self) -> Gateway {
            let mut p = ProviderConfig::new("local", ProviderKind::CodexAppServer);
            p.options = Some(crate::config::ProviderOptions {
                executable: Some(self.0.join("codex").to_string_lossy().into_owned()),
                working_directory: Some(self.0.to_string_lossy().into_owned()),
                ..Default::default()
            });
            Gateway::new(vec![p], HashMap::<String, String>::new())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn request(model: &str) -> ChatRequest {
        ChatRequest {
            model: format!("local/{model}@ultra"),
            system: Some("Use the lookup tool.".into()),
            system_parts: Vec::new(),
            messages: vec![Message::user_text("Find a chair")],
            tools: vec![ToolSpec {
                name: "lookup".into(),
                description: "Look up furniture".into(),
                input_schema: json!({"type":"object"}),
                deferred: true,
            }],
            cache_last_message: false,
            max_tokens: 0,
            temperature: None,
            effort: None,
        }
    }

    fn continue_with(req: &mut ChatRequest, response: ChatResponse) {
        req.messages.push(Message::assistant(response.content));
        req.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "call-1".into(),
                content: "oak chair".into(),
                is_error: false,
            }],
        });
    }

    #[test]
    fn native_dynamic_tool_round_trip_retains_usage_and_allows_deferred_schema_reveal() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        assert_eq!(gateway.list_models("local").unwrap().len(), 9);
        let mut req = request("fixture");
        req.system = None;
        req.system_parts = vec![apz_proto::chat::SystemPart {
            text: "Use the lookup tool.".into(),
            cache: true,
        }];
        req.cache_last_message = true;
        let first = gateway.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(first.stop, StopReason::ToolUse);
        assert_eq!(first.usage.input_tokens, 7);
        assert_eq!(first.usage.cache_read_tokens, 3);
        assert_eq!(first.tool_uses().next().unwrap().1, "lookup");
        continue_with(&mut req, first);
        req.tools[0].deferred = false;
        req.tools[0].input_schema =
            json!({"type":"object","properties":{"query":{"type":"string"}}});
        let mut deltas = Vec::new();
        let second = gateway.complete(&req, &mut |d| deltas.push(d)).unwrap();
        assert_eq!(second.stop, StopReason::EndTurn);
        assert_eq!(second.text(), "Done");
        assert_eq!(second.usage.input_tokens, 15);
        assert_eq!(second.usage.cache_read_tokens, 5);
        assert_eq!(second.usage.output_tokens, 4);
        assert_eq!(
            deltas
                .iter()
                .filter(|d| matches!(d, Delta::Text { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_continuation_cannot_cross_callers_or_change_its_prefix() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let mut req = request("fixture");
        let first = gateway.complete(&req, &mut |_| {}).unwrap();
        continue_with(&mut req, first);
        assert!(
            gateway
                .complete_for(&req, &mut |_| {}, &Caller::Owner { node: None })
                .unwrap_err()
                .to_string()
                .contains("caller")
        );
        let mut changed = req.clone();
        changed.messages[0] = Message::user_text("different history");
        assert!(
            gateway
                .complete(&changed, &mut |_| {})
                .unwrap_err()
                .to_string()
                .contains("history changed")
        );
        // Invalid attempts did not consume the legitimate pending call.
        assert_eq!(gateway.complete(&req, &mut |_| {}).unwrap().text(), "Done");
    }

    #[test]
    fn steering_appends_input_to_the_same_turn() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let mut req = request("steer");
        let first = gateway.complete(&req, &mut |_| {}).unwrap();
        continue_with(&mut req, first);
        req.messages.last_mut().unwrap().content.push(Block::Text {
            text: "Prefer oak".into(),
        });
        let answer = gateway.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(answer.stop, StopReason::EndTurn);
        assert_eq!(answer.text(), "Done");
    }

    #[test]
    fn a_dynamic_tool_request_can_precede_the_turn_start_response() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let mut req = request("interleaved-start");
        let first = gateway.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(first.stop, StopReason::ToolUse);
        continue_with(&mut req, first);
        assert_eq!(gateway.complete(&req, &mut |_| {}).unwrap().text(), "Done");
    }

    #[test]
    fn a_second_dynamic_tool_request_can_precede_the_steer_response() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let mut req = request("interleaved-steer");
        let first = gateway.complete(&req, &mut |_| {}).unwrap();
        continue_with(&mut req, first);
        req.messages.last_mut().unwrap().content.push(Block::Text {
            text: "Prefer oak".into(),
        });
        let second = gateway.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(second.stop, StopReason::ToolUse);
        let (id, name, input) = second.tool_uses().next().unwrap();
        assert_eq!(id, "call-2");
        assert_eq!(name, "lookup");
        assert_eq!(input, &json!({"query":"table"}));
        req.messages.push(Message::assistant(second.content));
        req.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "call-2".into(),
                content: "walnut table".into(),
                is_error: false,
            }],
        });
        assert_eq!(gateway.complete(&req, &mut |_| {}).unwrap().text(), "Done");
    }

    #[test]
    fn interleaved_requests_do_not_bypass_tool_name_or_approval_gates() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let unknown = gateway
            .complete(&request("interleaved-unknown"), &mut |_| {})
            .unwrap_err();
        assert!(
            unknown
                .to_string()
                .contains("outside this AgentPlaza request")
        );
        assert!(
            gateway
                .complete(&request("interleaved-approval"), &mut |_| {})
                .is_err()
        );
        assert!(gateway.inner.codex.lock().unwrap().is_empty());
    }

    #[test]
    fn chat_work_resets_deferred_tools_and_compaction_forwards_only_the_latest_result() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let mut req = request("fixture");
        let registered = req.tools.clone();
        let first = gateway.complete(&req, &mut |_| {}).unwrap();
        continue_with(&mut req, first);
        req.tools[0].deferred = false;
        req.tools[0].input_schema =
            json!({"type":"object","properties":{"query":{"type":"string"}}});
        let answer = gateway.complete(&req, &mut |_| {}).unwrap();
        req.messages.push(Message::assistant(answer.content));
        req.messages.push(Message::user_text("Look it up again"));
        // run_job retains thread.messages but creates Deferred::new for each chat work item.
        req.tools = registered;
        let next = gateway.complete(&req, &mut |_| {}).unwrap();
        continue_with(&mut req, next);
        let latest = req.messages.len() - 2;
        // The exact transform used by agent.rs::compress: add the summary to the first
        // user message, remove older turns, retain the latest assistant and its result.
        req.messages[0].content.push(Block::Text {
            text: format!(
                "{SUMMARY_HEAD}\nThe first lookup completed. The second returned oak chair."
            ),
        });
        req.messages.drain(1..latest);
        let mut changed_task = req.clone();
        changed_task.messages[0].content[0] = Block::Text {
            text: "A different task".into(),
        };
        assert!(gateway.complete(&changed_task, &mut |_| {}).is_err());
        let mut changed_response = req.clone();
        changed_response.messages[1].content.push(Block::Text {
            text: "fabricated response".into(),
        });
        assert!(gateway.complete(&changed_response, &mut |_| {}).is_err());
        let response = gateway.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(response.text(), "Done");
        assert_eq!(response.stop, StopReason::EndTurn);
    }

    #[test]
    fn lost_process_context_does_not_replay_completed_tools() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let mut req = request("fixture");
        let first = gateway.complete(&req, &mut |_| {}).unwrap();
        continue_with(&mut req, first);
        drop(gateway);
        assert!(
            fixture
                .gateway()
                .complete(&req, &mut |_| {})
                .unwrap_err()
                .to_string()
                .contains("no longer available")
        );
    }

    #[test]
    fn unknown_and_builtin_tools_fail_closed() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        for model in ["unknown-tool", "builtin"] {
            assert!(gateway.complete(&request(model), &mut |_| {}).is_err());
            assert!(gateway.inner.codex.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn a_silent_child_has_a_bounded_read_deadline() {
        let fixture = Fixture::new();
        let gateway = fixture.gateway();
        let config = gateway.provider("local").unwrap();
        let mut rpc = Rpc::start(&config, &gateway.inner.local_processes).unwrap();
        let before = Instant::now();
        assert!(rpc.read(before + Duration::from_millis(50)).is_err());
        assert!(before.elapsed() < Duration::from_secs(2));
    }

    /// Explicit opt-in only: uses the installed Codex login and consumes one real model turn.
    #[test]
    #[ignore = "requires APZ_CODEX_EXECUTABLE and a Codex ChatGPT login"]
    fn live_codex_astra_ultra_two_sequential_dynamic_tool_round_trips() {
        let executable = std::env::var("APZ_CODEX_EXECUTABLE").expect("explicit Codex executable");
        let fixture = Fixture::new();
        let mut config = ProviderConfig::new("local", ProviderKind::CodexAppServer);
        config.options = Some(crate::config::ProviderOptions {
            executable: Some(executable),
            working_directory: Some(fixture.0.to_string_lossy().into_owned()),
            ..Default::default()
        });
        let gateway = Gateway::new(vec![config], HashMap::<String, String>::new());
        let mut req = request("gpt-6-astra");
        req.system = Some("This is a transport verification. Call the supplied lookup tool with query chair. After receiving that result, call lookup again with query table. These must be two sequential calls, not parallel calls. After the second result, reply with both exact results separated by a semicolon and a space. Do not call any other tools.".into());
        req.messages = vec![Message::user_text("Look up a chair and then a table.")];
        req.tools[0].deferred = false;
        req.tools[0].input_schema = json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false});
        for (query, result) in [("chair", "oak chair"), ("table", "walnut table")] {
            let response = gateway.complete(&req, &mut |_| {}).unwrap();
            assert_eq!(response.model, "local/gpt-6-astra");
            assert_eq!(response.stop, StopReason::ToolUse);
            assert_eq!(response.tool_uses().count(), 1);
            let (id, name, arguments) = response.tool_uses().next().unwrap();
            assert_eq!(name, "lookup");
            assert_eq!(arguments, &json!({"query":query}));
            let id = id.to_string();
            req.messages.push(Message::assistant(response.content));
            req.messages.push(Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: id,
                    content: result.into(),
                    is_error: false,
                }],
            });
        }
        let answer = gateway.complete(&req, &mut |_| {}).unwrap();
        assert_eq!(answer.stop, StopReason::EndTurn);
        assert_eq!(answer.text().trim(), "oak chair; walnut table");
    }
}
