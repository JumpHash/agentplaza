//! The ACP agent methods the hub answers, over the plaza as [`Hub`] sees it.
//!
//! | ACP | Plaza |
//! |---|---|
//! | `initialize` | protocol 1; `loadSession`; prompts take text and embedded resources; sessions can be listed, resumed and closed |
//! | `session/new` | a new session on the connection's agent (`agentplaza acp --agent`), else `_meta.agentplaza.agent`, else the orchestrator; the editor's `mcpServers` are not connected (a notice says so); modes are the agent's approval modes |
//! | `session/prompt` | a chat work item on the session's thread; its stream as updates until its end; `stopReason` from the end (`end_turn`, `cancelled`); a failed item is a JSON-RPC error; an approval the work waits on is asked as `session/request_permission` and answered for the owner; the turn's end reports `usage_update`, and the first prompt titles the session (`session_info_update`); a prompt that is one of the [`COMMANDS`] runs it on the agent instead of queuing chat |
//! | `session/cancel` | cancels the session's work item: the one in progress, or one still queued |
//! | `session/load` | replays the session's work items, then continues; the commands and the model options come first |
//! | `session/list`, `session/resume`, `session/close` | the sessions; resume without replay; close ends it |
//! | `session/set_mode` | the agent's approval mode: `autonomous`, `ask`, `read_only` (`current_mode_update` follows) |
//! | `session/set_config_option` | the agent's `model`, within its tier's list; the reply carries the options |

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_acp::rpc::Peer;
use apz_acp::types::{self, AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES};
use serde_json::{Value, json};

use super::feed::{self, End, Feed};
use super::store::{Session, Sessions};
use crate::events::Frame;
use crate::hub_mcp::auth::Scope;
use crate::transcript::Entry;

/// What the ACP methods need from the plaza. [`super::app_hub`] gives the hub's; the tests give a
/// fake.
pub trait Hub: Sync {
    /// The agent `name` names (an agent name or a CID), or the orchestrator for `None`: its CID,
    /// or why there is none (no such agent, offline, a model agent with no chat).
    fn agent(&self, name: Option<&str>) -> Result<u64, String>;
    /// Subscribes to the bus. Taken before the work is queued, so no frame is missed.
    fn subscribe(&self) -> Receiver<Frame>;
    /// Queues `text` as the owner's chat on agent `cid`, thread `thread` (origin: the owner, over
    /// ACP); the work item's id.
    fn chat(&self, cid: u64, thread: &str, text: &str) -> Result<u64, String>;
    /// Cancels work item `work` of agent `cid` if it is in progress or queued; false otherwise.
    fn cancel(&self, cid: u64, work: u64) -> bool;
    /// The transcript entries of agent `cid` for the given work items, oldest first.
    fn entries(&self, cid: u64, works: &[u64]) -> Vec<Entry>;
    /// The pending approvals of agent `cid` held on `thread` (each as the approvals API shows it).
    fn pending_approvals(&self, cid: u64, thread: &str) -> Vec<Value>;
    /// Decides approval `id` for the owner ("from the editor"), as `POST /v1/approvals/{id}` does.
    fn decide(&self, id: &str, approve: bool, reason: &str) -> Result<(), String>;
    /// The agent's approval mode: `autonomous`, `ask` or `read_only`.
    fn mode(&self, cid: u64) -> String;
    fn set_mode(&self, cid: u64, mode: &str) -> Result<(), String>;
    /// The token usage work item `work` of agent `cid` made, as `input_tokens` and `output_tokens`;
    /// None when the work made no model call.
    fn usage(&self, cid: u64, work: u64) -> Option<Value>;
    /// Sends a control to agent `cid`'s work `work` (the one in progress for `None`): one of
    /// [`COMMANDS`] with its argument. Ok and Err both carry the text to show the editor.
    fn control(
        &self,
        cid: u64,
        work: Option<u64>,
        command: &str,
        arg: &str,
    ) -> Result<String, String>;
    /// The model agent `cid` runs now, and the models its tier allows.
    fn models(&self, cid: u64) -> (String, Vec<String>);
    /// Sets agent `cid`'s model, when its tier allows it.
    fn set_model(&self, cid: u64, model: &str) -> Result<(), String>;
    fn now_ms(&self) -> u64;
}

/// Who an editor connection acts for: the owner, or a hub MCP token of operate or govern scope and the
/// agents its allowlist names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    Owner,
    Token {
        id: String,
        scope: Scope,
        /// The agents the token's `acp:<name or cid>` entries name, without the prefix. Empty means every agent.
        agents: Vec<String>,
    },
}

impl Caller {
    /// Whether the caller may drive agent `cid`: its sessions, its work, and its settings. An entry is
    /// the agent's CID when it is a number, else its name, which the hub resolves.
    pub fn may_drive(&self, hub: &dyn Hub, cid: u64) -> bool {
        match self {
            Caller::Owner => true,
            Caller::Token { agents, .. } => {
                agents.is_empty()
                    || agents.iter().any(|agent| match agent.parse::<u64>() {
                        Ok(named) => named == cid,
                        Err(_) => hub.agent(Some(agent)) == Ok(cid),
                    })
            }
        }
    }

    /// Whether the caller may change what the agent does (its approval mode and model) and answer its
    /// approvals for the owner. An operate token drives the agent; the owner's decisions stay the owner's.
    pub fn may_govern(&self) -> bool {
        match self {
            Caller::Owner => true,
            Caller::Token { scope, .. } => *scope >= Scope::Govern,
        }
    }
}

/// One editor connection: who it is and the agent it asked for.
pub struct Conn<'h> {
    pub hub: &'h dyn Hub,
    pub sessions: &'h Sessions,
    /// `agentplaza acp --agent <name|cid>`, from the socket's `?agent=` query.
    pub agent: Option<String>,
    /// The editor's `clientInfo`, once `initialize` named it. The connection owns it; each call
    /// borrows it (the socket's handlers build a `Conn` per message over the connection's state).
    pub client: &'h Mutex<String>,
    pub caller: Caller,
}

/// How often the stream checks the editor and the approvals when no transcript frame came.
const POLL: Duration = Duration::from_millis(200);
/// How often the approvals are checked on their own, between transcript frames.
const APPROVALS_EVERY: Duration = Duration::from_secs(2);
/// How long the editor has to answer an approval before its request is left pending.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The most attached file text one prompt carries; the rest of the files are left out.
const PROMPT_BYTES: usize = 256 * 1024;
/// Said before the reply to `session/new` when the editor offered MCP servers: they are not run.
const MCP_NOTICE: &str = "AgentPlaza agents work in their own VM: the editor's MCP servers are not connected. Add remote MCP servers in the hub's MCP screen.";
/// The plaza's commands: name, what the editor shows, and the argument's hint when it takes one.
/// A prompt that is one of them (`/steer <text>`) is a control of the agent's work, not chat.
const COMMANDS: [(&str, &str, Option<&str>); 4] = [
    ("compress", "Compress the agent's conversation", None),
    (
        "steer",
        "Steer the work in progress",
        Some("what to change"),
    ),
    (
        "extend-turns",
        "Allow more model calls for this work",
        Some("how many"),
    ),
    ("stop", "Stop the work in progress", None),
];

/// Answers one request from the editor.
pub fn request(
    conn: &Conn<'_>,
    peer: &Peer,
    method: &str,
    params: Value,
) -> Result<Value, types::Error> {
    let names = &AGENT_METHOD_NAMES;
    if method == names.initialize {
        initialize(conn, &params)
    } else if method == names.session_new {
        new_session(conn, peer, &params)
    } else if method == names.session_prompt {
        prompt(conn, peer, &params)
    } else if method == names.session_load {
        load(conn, peer, &params)
    } else if method == names.session_resume {
        resume(conn, &params)
    } else if method == names.session_list {
        list(conn, &params)
    } else if method == names.session_close {
        close(conn, &params)
    } else if method == names.session_set_mode {
        set_mode(conn, peer, &params)
    } else if method == names.session_set_config_option {
        set_config_option(conn, &params)
    } else {
        Err(types::Error::method_not_found())
    }
}

/// Takes one notification from the editor (`session/cancel`).
pub fn notification(conn: &Conn<'_>, method: &str, params: Value) {
    if method != AGENT_METHOD_NAMES.session_cancel {
        return;
    }
    let Ok(session) = session_of(conn, &params) else {
        return;
    };
    // The last work is the one the editor's prompt waits on, in progress or queued.
    if let Some(&work) = session.works.last() {
        conn.hub.cancel(session.cid, work);
    }
}

fn initialize(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    // We answer protocol 1 whatever the editor asked for: it is the only one we speak.
    let name = params["clientInfo"]["name"].as_str().unwrap_or_default();
    let version = params["clientInfo"]["version"].as_str().unwrap_or_default();
    *lock(conn.client) = format!("{name} {version}").trim().to_string();
    Ok(json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
            "sessionCapabilities": {"list": {}, "resume": {}, "close": {}},
        },
        "agentInfo": {"name": "agentplaza", "version": env!("CARGO_PKG_VERSION")},
        "authMethods": [],
    }))
}

fn new_session(conn: &Conn<'_>, peer: &Peer, params: &Value) -> Result<Value, types::Error> {
    let name = conn.agent.clone().or_else(|| {
        params["_meta"]["agentplaza"]["agent"]
            .as_str()
            .map(str::to_string)
    });
    let cid = conn.hub.agent(name.as_deref()).map_err(invalid)?;
    if !conn.caller.may_drive(conn.hub, cid) {
        return Err(invalid("this token may not drive that agent"));
    }
    let cwd = params["cwd"].as_str().unwrap_or_default();
    let client = lock(conn.client).clone();
    let session = conn.sessions.open(cid, cwd, &client, conn.hub.now_ms());
    // The notice goes before the reply: the editor reads it as the session's first message.
    if params["mcpServers"]
        .as_array()
        .is_some_and(|servers| !servers.is_empty())
    {
        notify_update(
            peer,
            &session.id,
            json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": MCP_NOTICE},
            }),
        );
    }
    // Before the reply: the editor takes the commands as the session's, and a notification may come
    // ahead of the reply to `session/new`.
    notify_update(peer, &session.id, commands_update());
    Ok(json!({
        "sessionId": session.id,
        "modes": modes(conn.hub, cid),
        "configOptions": config_options(conn.hub, cid),
    }))
}

fn prompt(conn: &Conn<'_>, peer: &Peer, params: &Value) -> Result<Value, types::Error> {
    let session = open_session_of(conn, params)?;
    let text = prompt_text(&params["prompt"]);
    if let Some((command, arg)) = slash_command(&text) {
        return command_turn(conn, peer, &session, command, arg);
    }
    // Subscribed before the work is queued: the frames wait in the channel until the stream reads
    // them, so none is lost while `chat` returns.
    let frames = conn.hub.subscribe();
    let work = conn
        .hub
        .chat(session.cid, &session.id, &text)
        .map_err(internal)?;
    let had_title = !session.title.is_empty();
    conn.sessions
        .add_work(&session.id, work, &text, conn.hub.now_ms());
    // The first prompt sets the title; the editor learns it once.
    if !had_title {
        if let Some(titled) = conn
            .sessions
            .get(&session.id)
            .filter(|s| !s.title.is_empty())
        {
            notify_update(
                peer,
                &session.id,
                json!({
                    "sessionUpdate": "session_info_update",
                    "title": titled.title,
                    "updatedAt": rfc3339(titled.updated_ms),
                }),
            );
        }
    }
    let result = stream(
        conn.hub,
        peer,
        &session,
        work,
        &frames,
        conn.caller.may_govern(),
    );
    // The turn's tokens are reported whatever its end: a failed work spent them too.
    report_usage(conn.hub, peer, &session.id, session.cid, work);
    result
}

/// Runs a plaza command on the session's agent and answers with what the hub said, as the turn's
/// message. Nothing is queued as chat; a command the hub refused says why, and still ends the turn.
fn command_turn(
    conn: &Conn<'_>,
    peer: &Peer,
    session: &Session,
    command: &str,
    arg: &str,
) -> Result<Value, types::Error> {
    let said = match conn.hub.control(session.cid, None, command, arg) {
        Ok(said) | Err(said) => said,
    };
    notify_update(
        peer,
        &session.id,
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": said},
        }),
    );
    Ok(json!({"stopReason": "end_turn"}))
}

/// The command a prompt names: `/<name>` or `/<name> <argument>` for one of [`COMMANDS`], as
/// the command's name and its trimmed argument (empty when none).
fn slash_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    COMMANDS
        .iter()
        .any(|(known, _, _)| *known == name)
        .then_some((name, arg.trim()))
}

/// Tells the editor the work's tokens, when it made any model call. `size` is 0: the plaza does
/// not know the model's context window here.
fn report_usage(hub: &dyn Hub, peer: &Peer, session_id: &str, cid: u64, work: u64) {
    let Some(usage) = hub.usage(cid, work) else {
        return;
    };
    let input = usage["input_tokens"].as_u64().unwrap_or(0);
    let output = usage["output_tokens"].as_u64().unwrap_or(0);
    // `used` counts both parts; the engine part (the gateway's calls) is reported apart as well.
    notify_update(
        peer,
        session_id,
        json!({
            "sessionUpdate": "usage_update",
            "used": input + output,
            "size": 0,
            "_meta": {"agentplaza": {
                "input_tokens": input,
                "output_tokens": output,
                "engine_input_tokens": usage["engine_input_tokens"].as_u64().unwrap_or(0),
                "engine_output_tokens": usage["engine_output_tokens"].as_u64().unwrap_or(0),
            }},
        }),
    );
}

/// Sends the work item's updates as they come, asks the approvals it waits on, and returns the
/// turn's outcome when the item ends. The approvals are asked only when `govern` is set: otherwise
/// they stay pending for the owner to decide in the console.
fn stream(
    hub: &dyn Hub,
    peer: &Peer,
    session: &Session,
    work: u64,
    frames: &Receiver<Frame>,
    govern: bool,
) -> Result<Value, types::Error> {
    let mut feed = Feed::new(session.cid, work);
    let mut asked = HashSet::new();
    let mut checked = Instant::now();
    thread::scope(|scope| -> Result<Value, types::Error> {
        loop {
            match frames.recv_timeout(POLL) {
                Ok(frame) => {
                    let data: Value = serde_json::from_str(&frame.data).unwrap_or(Value::Null);
                    for update in feed.frame(&frame.kind, &data) {
                        notify_update(peer, &session.id, update);
                    }
                    if frame.kind == "transcript" {
                        if govern {
                            ask_approvals(scope, peer, hub, session.cid, &session.id, &mut asked);
                        }
                        checked = Instant::now();
                    }
                    if let Some(end) = feed.end(&frame.kind, &data) {
                        return finish(end);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(internal("the plaza's event bus closed"));
                }
            }
            // The work goes on without the editor; its updates have nowhere to go, so the turn ends.
            if peer.is_closed() {
                return Err(internal("the editor disconnected"));
            }
            if govern && checked.elapsed() >= APPROVALS_EVERY {
                ask_approvals(scope, peer, hub, session.cid, &session.id, &mut asked);
                checked = Instant::now();
            }
        }
    })
}

/// Asks the editor for each approval the session's thread holds that has not been asked yet. Each
/// ask runs on its own thread, so the stream keeps flowing while the editor decides.
fn ask_approvals<'scope, 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    peer: &'env Peer,
    hub: &'env dyn Hub,
    cid: u64,
    session_id: &str,
    asked: &mut HashSet<String>,
) where
    'env: 'scope,
{
    for approval in hub.pending_approvals(cid, session_id) {
        let Some(id) = approval["id"].as_str() else {
            continue;
        };
        if !asked.insert(id.to_string()) {
            continue;
        }
        let approval_id = id.to_string();
        let title = approval["subject"]
            .as_str()
            .or_else(|| approval["tool"].as_str())
            .unwrap_or("Approval");
        let params = json!({
            "sessionId": session_id,
            "toolCall": {
                "toolCallId": format!("approval-{id}"),
                "title": title,
                "kind": "other",
                "status": "pending",
                "rawInput": approval["detail"],
            },
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ],
        });
        scope.spawn(move || answer_approval(peer, hub, &approval_id, params));
    }
}

/// Asks the editor one approval and decides it for the owner. A cancelled or failed ask leaves the
/// approval pending, for the owner to decide in the console.
fn answer_approval(peer: &Peer, hub: &dyn Hub, approval_id: &str, params: Value) {
    let Ok(reply) = peer.call(
        CLIENT_METHOD_NAMES.session_request_permission,
        params,
        PERMISSION_TIMEOUT,
    ) else {
        return;
    };
    let outcome = &reply["outcome"];
    if outcome["outcome"].as_str() != Some("selected") {
        return;
    }
    let approve = match outcome["optionId"].as_str() {
        Some("allow") => true,
        Some("reject") => false,
        _ => return,
    };
    let _ = hub.decide(approval_id, approve, "from the editor");
}

fn finish(end: End) -> Result<Value, types::Error> {
    match end.status.as_str() {
        "done" => Ok(json!({"stopReason": "end_turn"})),
        "stopped" => Ok(json!({"stopReason": "cancelled"})),
        _ => Err(internal(end.summary)),
    }
}

fn load(conn: &Conn<'_>, peer: &Peer, params: &Value) -> Result<Value, types::Error> {
    let session = session_of(conn, params)?;
    let entries = conn.hub.entries(session.cid, &session.works);
    for &work in &session.works {
        for update in feed::replay(work, &entries) {
            notify_update(peer, &session.id, update);
        }
    }
    notify_update(peer, &session.id, commands_update());
    Ok(json!({
        "modes": modes(conn.hub, session.cid),
        "configOptions": config_options(conn.hub, session.cid),
    }))
}

fn resume(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    let session = session_of(conn, params)?;
    Ok(json!({"modes": modes(conn.hub, session.cid)}))
}

fn list(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    let sessions: Vec<Value> = conn
        .sessions
        .list(params["cwd"].as_str())
        .iter()
        .filter(|s| conn.caller.may_drive(conn.hub, s.cid))
        .map(|s| {
            json!({
                "sessionId": s.id,
                "cwd": s.cwd,
                "title": s.title,
                "updatedAt": rfc3339(s.updated_ms),
            })
        })
        .collect();
    Ok(json!({"sessions": sessions}))
}

fn close(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    let id = params["sessionId"].as_str().unwrap_or_default();
    session_of(conn, params)?;
    if conn.sessions.close(id, conn.hub.now_ms()) {
        Ok(json!({}))
    } else {
        Err(invalid(format!("no session {id}")))
    }
}

fn set_mode(conn: &Conn<'_>, peer: &Peer, params: &Value) -> Result<Value, types::Error> {
    let session = open_session_of(conn, params)?;
    require_govern(conn, "modes")?;
    let mode = params["modeId"].as_str().unwrap_or_default();
    conn.hub.set_mode(session.cid, mode).map_err(invalid)?;
    notify_update(
        peer,
        &session.id,
        json!({
            "sessionUpdate": "current_mode_update",
            "currentModeId": conn.hub.mode(session.cid),
        }),
    );
    Ok(json!({}))
}

/// `session/set_config_option`: the agent's model. The reply carries every config option again.
fn set_config_option(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    let session = open_session_of(conn, params)?;
    require_govern(conn, "the model")?;
    let config = params["configId"].as_str().unwrap_or_default();
    if config != "model" {
        return Err(invalid(format!("no config option {config}")));
    }
    let model = params["value"]
        .as_str()
        .ok_or_else(|| invalid("the model is a model id, as a string"))?;
    conn.hub.set_model(session.cid, model).map_err(invalid)?;
    Ok(json!({"configOptions": config_options(conn.hub, session.cid)}))
}

/// The session's config options: the agent's `model`, and the models its tier allows.
fn config_options(hub: &dyn Hub, cid: u64) -> Value {
    let (current, models) = hub.models(cid);
    json!([{
        "id": "model",
        "name": "Model",
        "type": "select",
        "currentValue": current,
        "options": models
            .iter()
            .map(|m| json!({"value": m, "name": m}))
            .collect::<Vec<_>>(),
    }])
}

/// The plaza's commands, for the editor's command list.
fn commands_update() -> Value {
    let commands: Vec<Value> = COMMANDS
        .iter()
        .map(|(name, description, hint)| {
            let mut command = json!({"name": name, "description": description});
            if let Some(hint) = hint {
                command["input"] = json!({"hint": hint});
            }
            command
        })
        .collect();
    json!({"sessionUpdate": "available_commands_update", "availableCommands": commands})
}

/// The session `params` name, open or closed, when the caller may drive its agent.
fn session_of(conn: &Conn<'_>, params: &Value) -> Result<Session, types::Error> {
    let id = params["sessionId"].as_str().unwrap_or_default();
    let session = conn
        .sessions
        .get(id)
        .ok_or_else(|| invalid(format!("no session {id}")))?;
    if conn.caller.may_drive(conn.hub, session.cid) {
        Ok(session)
    } else {
        Err(invalid("this token may not drive that session's agent"))
    }
}

/// Changing what the agent does (its approval mode, its model) needs govern. An operate token drives
/// the agent; it does not change the owner's settings.
fn require_govern(conn: &Conn<'_>, what: &str) -> Result<(), types::Error> {
    if conn.caller.may_govern() {
        Ok(())
    } else {
        Err(invalid(format!(
            "this token's scope is operate; changing {what} needs govern"
        )))
    }
}

/// The session `params` name, when it is still open.
fn open_session_of(conn: &Conn<'_>, params: &Value) -> Result<Session, types::Error> {
    let session = session_of(conn, params)?;
    if session.closed {
        Err(invalid(format!("session {} is closed", session.id)))
    } else {
        Ok(session)
    }
}

fn modes(hub: &dyn Hub, cid: u64) -> Value {
    json!({
        "currentModeId": hub.mode(cid),
        "availableModes": [
            {"id": "autonomous", "name": "Autonomous"},
            {"id": "ask", "name": "Ask before writes"},
            {"id": "read_only", "name": "Read only"},
        ],
    })
}

fn notify_update(peer: &Peer, session_id: &str, update: Value) {
    let _ = peer.notify(
        CLIENT_METHOD_NAMES.session_update,
        json!({"sessionId": session_id, "update": update}),
    );
}

/// The prompt as the agent reads it: the text blocks, then the editor's attached files and links,
/// in the order given. Attached files stop at [`PROMPT_BYTES`] of text; images are not sent.
fn prompt_text(prompt: &Value) -> String {
    let mut out = String::new();
    let mut files_left_out = false;
    for block in prompt.as_array().into_iter().flatten() {
        let kind = block["type"].as_str();
        let piece = match kind {
            Some("text") => block["text"].as_str().map(str::to_string),
            Some("resource") => match (
                block["resource"]["uri"].as_str(),
                block["resource"]["text"].as_str(),
            ) {
                (Some(uri), Some(text)) => Some(format!("File {uri}:\n```\n{text}\n```")),
                _ => None,
            },
            Some("resource_link") => block["uri"].as_str().map(|uri| format!("Attached: {uri}")),
            _ => None,
        };
        let Some(piece) = piece else {
            continue;
        };
        let sep = if out.is_empty() { 0 } else { 2 };
        if kind == Some("resource") && out.len() + sep + piece.len() > PROMPT_BYTES {
            files_left_out = true;
            continue;
        }
        if sep > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&piece);
    }
    if files_left_out {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("(… more attached files left out)");
    }
    out
}

/// `ms` since the Unix epoch as an RFC 3339 UTC time with milliseconds, e.g.
/// `2023-11-14T22:13:20.123Z`. The day is converted with Howard Hinnant's `civil_from_days`, so no
/// date crate is needed for one timestamp.
fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000,
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An ACP error for a request the editor got wrong, with why.
fn invalid(msg: impl ToString) -> types::Error {
    types::Error::invalid_params().data(json!(msg.to_string()))
}

/// An ACP error for a failure on the plaza's side, with why.
fn internal(msg: impl ToString) -> types::Error {
    types::Error::internal_error().data(json!(msg.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::{self, Sender};

    use apz_acp::rpc;
    use apz_acp::testing::{ClientConfig, FakeClient};

    use super::*;

    /// The plaza as the tests see it: a fake hub that records what the methods ask of it, and the
    /// real session store.
    struct Plaza {
        hub: FakeHub,
        sessions: Sessions,
        client: Mutex<String>,
        agent: Option<String>,
        caller: Caller,
    }

    struct FakeHub {
        calls: Mutex<Vec<String>>,
        /// The bus receiver, handed out once by `subscribe`; the test keeps the sender.
        frames: Mutex<Option<Receiver<Frame>>>,
        mode: Mutex<String>,
        /// What `usage` reports for any work (None: no model call).
        usage: Mutex<Option<Value>>,
        /// What `models` reports: the current model and the tier's list.
        models: Mutex<(String, Vec<String>)>,
        /// The approvals the session's thread holds (each as the approvals API shows it).
        approvals: Mutex<Vec<Value>>,
    }

    impl FakeHub {
        fn record(&self, call: String) {
            lock(&self.calls).push(call);
        }

        fn calls(&self) -> Vec<String> {
            lock(&self.calls).clone()
        }
    }

    impl Hub for FakeHub {
        fn agent(&self, name: Option<&str>) -> Result<u64, String> {
            self.record(format!("agent {name:?}"));
            match name {
                Some("missing") => Err("no agent named missing".to_string()),
                Some("other") => Ok(8),
                _ => Ok(7),
            }
        }

        fn subscribe(&self) -> Receiver<Frame> {
            lock(&self.frames)
                .take()
                .expect("the fake hub hands out one receiver")
        }

        fn chat(&self, cid: u64, thread: &str, text: &str) -> Result<u64, String> {
            self.record(format!("chat {cid} {thread} {text}"));
            Ok(42)
        }

        fn cancel(&self, cid: u64, work: u64) -> bool {
            self.record(format!("cancel {cid} {work}"));
            true
        }

        fn entries(&self, _cid: u64, _works: &[u64]) -> Vec<Entry> {
            Vec::new()
        }

        fn pending_approvals(&self, _cid: u64, _thread: &str) -> Vec<Value> {
            lock(&self.approvals).clone()
        }

        fn decide(&self, id: &str, approve: bool, reason: &str) -> Result<(), String> {
            self.record(format!("decide {id} {approve} {reason}"));
            Ok(())
        }

        fn mode(&self, _cid: u64) -> String {
            lock(&self.mode).clone()
        }

        fn set_mode(&self, cid: u64, mode: &str) -> Result<(), String> {
            self.record(format!("set_mode {cid} {mode}"));
            if !["autonomous", "ask", "read_only"].contains(&mode) {
                return Err(format!("no mode {mode}"));
            }
            *lock(&self.mode) = mode.to_string();
            Ok(())
        }

        fn usage(&self, cid: u64, work: u64) -> Option<Value> {
            self.record(format!("usage {cid} {work}"));
            lock(&self.usage).clone()
        }

        fn control(
            &self,
            cid: u64,
            work: Option<u64>,
            command: &str,
            arg: &str,
        ) -> Result<String, String> {
            self.record(format!("control {cid} {work:?} {command}({arg})"));
            if arg == "refused" {
                return Err("agent 7 has no running work".to_string());
            }
            Ok(format!("{command} sent"))
        }

        fn models(&self, _cid: u64) -> (String, Vec<String>) {
            lock(&self.models).clone()
        }

        fn set_model(&self, cid: u64, model: &str) -> Result<(), String> {
            self.record(format!("set_model {cid} {model}"));
            if model.starts_with("forbidden/") {
                return Err(format!("{model} is not allowed for this tier"));
            }
            lock(&self.models).0 = model.to_string();
            Ok(())
        }

        fn now_ms(&self) -> u64 {
            1_700_000_000_123
        }
    }

    /// An editor on one end of two socket pairs and the agent's peer on the other.
    struct Harness {
        client: FakeClient,
        plaza: Arc<Plaza>,
        frames: Sender<Frame>,
        _server: Peer,
    }

    fn harness(agent: Option<&str>) -> Harness {
        harness_with(agent, Caller::Owner, ClientConfig::default())
    }

    /// An editor connection as `caller`, whose client answers as `config` says.
    fn harness_with(agent: Option<&str>, caller: Caller, config: ClientConfig) -> Harness {
        let (frames, rx) = mpsc::channel();
        let plaza = Arc::new(Plaza {
            hub: FakeHub {
                calls: Mutex::default(),
                frames: Mutex::new(Some(rx)),
                mode: Mutex::new("autonomous".to_string()),
                usage: Mutex::default(),
                models: Mutex::new((
                    "anthropic/claude-haiku-5-5".to_string(),
                    vec![
                        "anthropic/claude-haiku-5-5".to_string(),
                        "anthropic/claude-opus-5-5".to_string(),
                    ],
                )),
                approvals: Mutex::default(),
            },
            sessions: Sessions::default(),
            client: Mutex::default(),
            agent: agent.map(str::to_string),
            caller,
        });
        // Pipes, not a socket pair: the hub builds and tests on Windows too.
        let (server_in, client_out) = std::io::pipe().expect("a pipe");
        let (client_in, server_out) = std::io::pipe().expect("a pipe");
        let for_requests = Arc::clone(&plaza);
        let for_notes = Arc::clone(&plaza);
        let server = Peer::new(
            server_in,
            server_out,
            move |peer, method, params| request(&conn_of(&for_requests), peer, method, params),
            move |_peer, method, params| notification(&conn_of(&for_notes), method, params),
        );
        let client = FakeClient::spawn(client_in, client_out, config);
        Harness {
            client,
            plaza,
            frames,
            _server: server,
        }
    }

    fn conn_of(plaza: &Plaza) -> Conn<'_> {
        Conn {
            hub: &plaza.hub,
            sessions: &plaza.sessions,
            agent: plaza.agent.clone(),
            client: &plaza.client,
            caller: plaza.caller.clone(),
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    /// An operate token with no allowlist: it may drive every agent.
    fn operate() -> Caller {
        Caller::Token {
            id: "mcp_test".into(),
            scope: Scope::Operate,
            agents: Vec::new(),
        }
    }

    /// The work's end on the bus, as the agent's transcript sends it.
    fn work_end() -> Frame {
        Frame {
            kind: "transcript".to_string(),
            data: json!({
                "cid": 7,
                "work": 42,
                "kind": "work_end",
                "data": {"status": "done", "summary": ""},
            })
            .to_string(),
        }
    }

    #[test]
    fn an_operate_token_cannot_change_the_mode_or_the_model_and_is_not_asked_approvals() {
        let h = harness_with(None, operate(), ClientConfig::default());
        *lock(&h.plaza.hub.approvals) = vec![json!({"id": "ap1", "subject": "Write a file"})];
        let session = h.client.new_session("/work/app").expect("session/new");
        let mode = h
            .client
            .peer()
            .call(
                "session/set_mode",
                json!({"sessionId": session, "modeId": "ask"}),
                WAIT,
            )
            .expect_err("an operate token does not set the mode");
        assert!(matches!(mode, rpc::Error::Remote(_)), "{mode:?}");
        assert!(format!("{mode:?}").contains("changing modes needs govern"));
        let model = h
            .client
            .peer()
            .call(
                "session/set_config_option",
                json!({"sessionId": session, "configId": "model", "value": "anthropic/claude-opus-5-5"}),
                WAIT,
            )
            .expect_err("an operate token does not set the model");
        assert!(format!("{model:?}").contains("changing the model needs govern"));
        // The work ends with an approval pending: the owner decides it in the console, not the editor.
        h.frames.send(work_end()).expect("the bus is open");
        h.client
            .prompt(&session, "write it")
            .expect("session/prompt");
        let calls = h.plaza.hub.calls();
        assert!(
            !calls.iter().any(|c| c.starts_with("set_mode")),
            "{calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("set_model")),
            "{calls:?}"
        );
        assert!(!calls.iter().any(|c| c.starts_with("decide")), "{calls:?}");
    }

    #[test]
    fn the_owner_is_asked_the_approval_and_the_answer_decides_it() {
        // The control for the test above: the same approval, asked when the caller may govern.
        let h = harness_with(None, Caller::Owner, ClientConfig::default());
        *lock(&h.plaza.hub.approvals) = vec![json!({"id": "ap1", "subject": "Write a file"})];
        let session = h.client.new_session("/work/app").expect("session/new");
        h.frames.send(work_end()).expect("the bus is open");
        h.client
            .prompt(&session, "write it")
            .expect("session/prompt");
        assert!(
            h.plaza
                .hub
                .calls()
                .contains(&"decide ap1 true from the editor".to_string())
        );
    }

    #[test]
    fn a_govern_token_is_asked_the_approval_and_changes_the_mode() {
        let govern = Caller::Token {
            id: "mcp_test".into(),
            scope: Scope::Govern,
            agents: Vec::new(),
        };
        let h = harness_with(None, govern, ClientConfig::default());
        *lock(&h.plaza.hub.approvals) = vec![json!({"id": "ap1", "subject": "Write a file"})];
        let session = h.client.new_session("/work/app").expect("session/new");
        h.client
            .peer()
            .call(
                "session/set_mode",
                json!({"sessionId": session, "modeId": "ask"}),
                WAIT,
            )
            .expect("a govern token sets the mode");
        assert!(h.plaza.hub.calls().contains(&"set_mode 7 ask".to_string()));
        h.frames.send(work_end()).expect("the bus is open");
        h.client
            .prompt(&session, "write it")
            .expect("session/prompt");
        assert!(
            h.plaza
                .hub
                .calls()
                .contains(&"decide ap1 true from the editor".to_string())
        );
    }

    #[test]
    fn an_allowlist_names_the_agents_a_token_may_drive_by_name_or_cid() {
        let h = harness(None);
        let named = Caller::Token {
            id: "mcp_test".into(),
            scope: Scope::Operate,
            agents: vec!["research".into()],
        };
        let by_cid = Caller::Token {
            id: "mcp_test".into(),
            scope: Scope::Operate,
            agents: vec!["8".into()],
        };
        assert!(named.may_drive(&h.plaza.hub, 7));
        assert!(!named.may_drive(&h.plaza.hub, 8));
        assert!(by_cid.may_drive(&h.plaza.hub, 8));
        assert!(!by_cid.may_drive(&h.plaza.hub, 7));
        assert!(operate().may_drive(&h.plaza.hub, 8));
        assert!(Caller::Owner.may_drive(&h.plaza.hub, 8));
    }

    #[test]
    fn a_token_is_refused_a_session_on_an_agent_its_allowlist_does_not_name() {
        let allowed = Caller::Token {
            id: "mcp_test".into(),
            scope: Scope::Operate,
            agents: vec!["research".into()],
        };
        // The editor asks for agent "other" (CID 8); the token names "research" (CID 7).
        let h = harness_with(Some("other"), allowed, ClientConfig::default());
        let refused = h
            .client
            .peer()
            .call("session/new", json!({"cwd": "/work/app"}), WAIT)
            .expect_err("agent other is not on the allowlist");
        assert!(matches!(refused, rpc::Error::Remote(_)), "{refused:?}");
        assert!(format!("{refused:?}").contains("may not drive that agent"));
    }

    #[test]
    fn initialize_answers_protocol_one_and_keeps_the_editor() {
        let h = harness(None);
        let reply = h
            .client
            .peer()
            .call(
                "initialize",
                json!({
                    "protocolVersion": 9,
                    "clientInfo": {"name": "zed", "version": "0.200"},
                }),
                WAIT,
            )
            .expect("initialize");
        assert_eq!(reply["protocolVersion"], 1);
        assert_eq!(reply["agentInfo"]["name"], "agentplaza");
        assert_eq!(reply["agentCapabilities"]["loadSession"], true);
        assert_eq!(*lock(&h.plaza.client), "zed 0.200");
    }

    #[test]
    fn unknown_methods_are_method_not_found() {
        let h = harness(None);
        let err = h
            .client
            .peer()
            .call("session/bogus", json!({}), WAIT)
            .expect_err("no such method");
        assert!(matches!(err, rpc::Error::Remote(_)), "{err:?}");
    }

    #[test]
    fn new_session_asks_the_connection_agent() {
        let h = harness(Some("research"));
        h.client.new_session("/work/app").expect("session/new");
        assert_eq!(
            h.plaza.hub.calls(),
            vec!["agent Some(\"research\")".to_string()]
        );
    }

    #[test]
    fn new_session_says_the_editor_mcp_servers_are_not_connected() {
        let h = harness(None);
        h.client
            .peer()
            .call(
                "session/new",
                json!({
                    "cwd": "/work/app",
                    "mcpServers": [{"name": "fs", "command": "mcp-fs", "args": [], "env": []}],
                }),
                WAIT,
            )
            .expect("session/new");
        let updates = h.client.updates();
        // The notice, then the command list.
        assert_eq!(updates.len(), 2);
        let text = serde_json::to_string(&updates[0]).expect("serialize");
        assert!(
            text.contains("the editor's MCP servers are not connected"),
            "{text}"
        );
    }

    #[test]
    fn new_session_offers_the_commands_and_the_model_options() {
        let h = harness(None);
        let reply = h
            .client
            .peer()
            .call("session/new", json!({"cwd": "/work/app"}), WAIT)
            .expect("session/new");
        let model = &reply["configOptions"][0];
        assert_eq!(model["id"], "model");
        assert_eq!(model["currentValue"], "anthropic/claude-haiku-5-5");
        assert_eq!(model["options"].as_array().map(Vec::len), Some(2));
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(said.contains("available_commands_update"), "{said}");
        assert!(said.contains("\"name\":\"extend-turns\""), "{said}");
        assert!(said.contains("\"hint\":\"how many\""), "{said}");
    }

    #[test]
    fn session_cancel_asks_the_hub_to_cancel_the_last_work_queued_or_not() {
        let h = harness(None);
        let session = h.plaza.sessions.open(7, "/work/app", "", 0);
        h.plaza.sessions.add_work(&session.id, 42, "fix it", 0);
        notification(
            &conn_of(&h.plaza),
            "session/cancel",
            json!({"sessionId": session.id}),
        );
        assert!(h.plaza.hub.calls().contains(&"cancel 7 42".to_string()));
    }

    #[test]
    fn a_slash_command_runs_on_the_agent_and_is_not_queued_as_chat() {
        let h = harness(None);
        let session = h.client.new_session("/work/app").expect("session/new");
        let stop = h.client.prompt(&session, "/stop").expect("session/prompt");
        assert_eq!(
            serde_json::to_value(stop).expect("serialize"),
            json!("end_turn")
        );
        let calls = h.plaza.hub.calls();
        assert!(
            calls.contains(&"control 7 None stop()".to_string()),
            "{calls:?}"
        );
        assert!(!calls.iter().any(|c| c.starts_with("chat")), "{calls:?}");
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(said.contains("stop sent"), "{said}");
    }

    #[test]
    fn a_refused_command_says_why_and_still_ends_the_turn() {
        let h = harness(None);
        let session = h.client.new_session("/work/app").expect("session/new");
        let stop = h
            .client
            .prompt(&session, "/steer refused")
            .expect("session/prompt");
        assert_eq!(
            serde_json::to_value(stop).expect("serialize"),
            json!("end_turn")
        );
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(said.contains("no running work"), "{said}");
    }

    #[test]
    fn slash_commands_are_the_listed_names_with_their_argument() {
        assert_eq!(
            slash_command("/steer  add tests "),
            Some(("steer", "add tests"))
        );
        assert_eq!(
            slash_command(" /extend-turns 5"),
            Some(("extend-turns", "5"))
        );
        assert_eq!(slash_command("/compress"), Some(("compress", "")));
        assert_eq!(slash_command("/compressor"), None);
        assert_eq!(slash_command("please /stop"), None);
        assert_eq!(slash_command("/unknown x"), None);
    }

    #[test]
    fn set_config_option_picks_the_model_and_the_tier_refuses_the_rest() {
        let h = harness(None);
        let session = h.client.new_session("/work/app").expect("session/new");
        let reply = h
            .client
            .peer()
            .call(
                "session/set_config_option",
                json!({"sessionId": session, "configId": "model", "value": "anthropic/claude-opus-5-5"}),
                WAIT,
            )
            .expect("session/set_config_option");
        assert!(
            h.plaza
                .hub
                .calls()
                .contains(&"set_model 7 anthropic/claude-opus-5-5".to_string())
        );
        assert_eq!(reply["configOptions"][0]["id"], "model");
        let refused = h
            .client
            .peer()
            .call(
                "session/set_config_option",
                json!({"sessionId": session, "configId": "model", "value": "forbidden/model"}),
                WAIT,
            )
            .expect_err("the tier refuses it");
        assert!(matches!(refused, rpc::Error::Remote(_)), "{refused:?}");
        let other = h
            .client
            .peer()
            .call(
                "session/set_config_option",
                json!({"sessionId": session, "configId": "effort", "value": "low"}),
                WAIT,
            )
            .expect_err("no such option");
        assert!(matches!(other, rpc::Error::Remote(_)), "{other:?}");
    }

    #[test]
    fn a_turn_reports_its_usage_and_the_first_prompt_titles_the_session() {
        let h = harness(None);
        *lock(&h.plaza.hub.usage) = Some(json!({"input_tokens": 120, "output_tokens": 30}));
        let session = h.client.new_session("/work/app").expect("session/new");
        // The work's end is already on the bus; the turn reads it.
        h.frames
            .send(Frame {
                kind: "transcript".to_string(),
                data: json!({
                    "cid": 7,
                    "work": 42,
                    "kind": "work_end",
                    "data": {"status": "done", "summary": ""},
                })
                .to_string(),
            })
            .expect("the bus is open");
        h.client
            .prompt(&session, "Fix the parser\nthen test it")
            .expect("session/prompt");
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(
            said.contains("\"sessionUpdate\":\"session_info_update\""),
            "{said}"
        );
        assert!(said.contains("\"title\":\"Fix the parser\""), "{said}");
        assert!(
            said.contains("\"updatedAt\":\"2023-11-14T22:13:20.123Z\""),
            "{said}"
        );
        assert!(
            said.contains("\"sessionUpdate\":\"usage_update\""),
            "{said}"
        );
        assert!(said.contains("\"used\":150"), "{said}");
        assert!(said.contains("\"input_tokens\":120"), "{said}");
        assert!(h.plaza.hub.calls().contains(&"usage 7 42".to_string()));
    }

    #[test]
    fn a_later_prompt_does_not_retitle_the_session() {
        let h = harness(None);
        let session = h.client.new_session("/work/app").expect("session/new");
        // A session with a title already: the prompt sends no `session_info_update`.
        let id = h.plaza.sessions.list(None)[0].id.clone();
        h.plaza.sessions.add_work(&id, 1, "earlier", 0);
        h.frames
            .send(Frame {
                kind: "transcript".to_string(),
                data: json!({
                    "cid": 7,
                    "work": 42,
                    "kind": "work_end",
                    "data": {"status": "done", "summary": ""},
                })
                .to_string(),
            })
            .expect("the bus is open");
        h.client.prompt(&session, "again").expect("session/prompt");
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(!said.contains("session_info_update"), "{said}");
    }

    #[test]
    fn a_missing_agent_is_an_invalid_request() {
        let h = harness(Some("missing"));
        let err = h
            .client
            .peer()
            .call(
                "session/new",
                json!({"cwd": "/work/app", "mcpServers": []}),
                WAIT,
            )
            .expect_err("no such agent");
        assert!(matches!(err, rpc::Error::Remote(_)), "{err:?}");
    }

    #[test]
    fn set_mode_calls_the_hub_and_says_so() {
        let h = harness(None);
        let session = h.client.new_session("/work/app").expect("session/new");
        h.client
            .peer()
            .call(
                "session/set_mode",
                json!({"sessionId": session, "modeId": "ask"}),
                WAIT,
            )
            .expect("session/set_mode");
        assert!(h.plaza.hub.calls().contains(&"set_mode 7 ask".to_string()));
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(said.contains("current_mode_update"), "{said}");
    }

    #[test]
    fn prompt_streams_the_work_until_it_ends() {
        let h = harness(None);
        let session = h.client.new_session("/work/app").expect("session/new");
        // The work's end is already on the bus when the prompt subscribes; the channel keeps it.
        // The frame's shape is the transcript entry's (`cid`, `work`, `kind`, `data`).
        h.frames
            .send(Frame {
                kind: "transcript".to_string(),
                data: json!({
                    "cid": 7,
                    "work": 42,
                    "kind": "work_end",
                    "data": {"status": "done", "summary": ""},
                })
                .to_string(),
            })
            .expect("the bus is open");
        let stop = h.client.prompt(&session, "hello").expect("session/prompt");
        assert_eq!(
            serde_json::to_value(stop).expect("serialize"),
            json!("end_turn")
        );
        assert!(
            h.plaza
                .hub
                .calls()
                .iter()
                .any(|c| c.starts_with("chat 7 acp-") && c.ends_with(" hello"))
        );
    }

    #[test]
    fn prompt_text_joins_text_files_and_links() {
        let prompt = json!([
            {"type": "text", "text": "Fix this"},
            {"type": "image", "data": "AAAA", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///w/a.rs", "text": "fn main() {}"}},
            {"type": "resource_link", "uri": "file:///w/b.rs", "name": "b"},
            {"type": "text", "text": "Thanks"},
        ]);
        assert_eq!(
            prompt_text(&prompt),
            "Fix this\n\nFile file:///w/a.rs:\n```\nfn main() {}\n```\n\nAttached: file:///w/b.rs\n\nThanks"
        );
    }

    #[test]
    fn prompt_text_leaves_out_files_past_the_cap() {
        let big = "x".repeat(PROMPT_BYTES / 2 + 1);
        let prompt = json!([
            {"type": "resource", "resource": {"uri": "a", "text": big}},
            {"type": "resource", "resource": {"uri": "b", "text": big}},
        ]);
        let text = prompt_text(&prompt);
        assert!(text.starts_with("File a:\n```\n"));
        assert!(!text.contains("File b:"));
        assert!(text.ends_with("(… more attached files left out)"));
    }

    #[test]
    fn rfc3339_formats_utc_with_milliseconds() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
