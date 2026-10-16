//! The ACP agent methods the hub answers, over the plaza as [`Hub`] sees it.
//!
//! | ACP | Plaza |
//! |---|---|
//! | `initialize` | protocol 1; `loadSession`; prompts take text and embedded resources; sessions can be listed, resumed and closed |
//! | `session/new` | a new session on the connection's agent (`agentplaza acp --agent`), else `_meta.agentplaza.agent`, else the orchestrator; the editor's `mcpServers` are not connected (a notice says so); modes are the agent's approval modes |
//! | `session/prompt` | a chat work item on the session's thread; its stream as updates until its end; `stopReason` from the end (`end_turn`, `cancelled`); a failed item is a JSON-RPC error; an approval the work waits on is asked as `session/request_permission` and answered for the owner; the turn's end reports `usage_update`, and the first prompt titles the session (`session_info_update`); a prompt that is one of the [`COMMANDS`] runs it on the agent instead of queuing chat; a session linked to a plaza repository (`_meta.agentplaza.repo`, on new/load/resume/prompt) gets a note on that repository's branch, and the pull request its work opened comes back as a `tool_call` of diffs |
//! | `session/cancel` | cancels the session's work item: the one in progress, or one still queued |
//! | `session/load` | replays the session's work items, then continues; the commands and the model options come first |
//! | `session/list`, `session/resume`, `session/close` | the sessions; resume without replay; close ends it |
//! | `session/set_mode` | the agent's approval mode: `autonomous`, `ask`, `read_only` (`current_mode_update` follows) |
//! | `session/set_config_option` | the agent's `model`, within its tier's list; the reply carries the options |
//! | write-back | with the owner's `acp_write_back` on, the editor's `fs.writeTextFile`, and a caller that may govern: after the pull request's `tool_call`, `session/request_permission` asks to apply its files to the project (`apply-pr-<n>`, 10 minutes); on `apply` the editor gets one `fs/write_text_file` per changed file (30 seconds each), then a `tool_call_update`; deleted files are left for the owner. The agent never writes the project itself. Never on a v2 connection, which has no `fs/write_text_file` |
//! | v2 draft | with the owner's `acp_v2` on, an editor that asks for protocol 2 gets it (see [`apz_acp::compat`]): the updates go out as v2 updates, a chunk takes the message id of its turn; `session/prompt` answers `{messageId}` at once and its turn follows as `state_update`s (running, then idle with the stop reason and the usage); `session/new` replies with the commands; `session/resume` replays nothing; `session/load` and `session/set_mode` are method not found |

use std::collections::HashSet;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_acp::compat::{
    idle_update, initialize_request_to_v1, initialize_response_to_v2, negotiate,
    new_session_response_to_v2, permission_request_to_v2, permission_response_to_v1,
    running_update, update_to_v2,
};
use apz_acp::rpc::Peer;
use apz_acp::types::{self, AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES};
use serde_json::{Value, json};

use super::feed::{self, End, Feed};
use super::store::{RepoLink, Session, Sessions};
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
    /// [`COMMANDS`] with its argument. `from` says who sent it (see [`Caller::label`]), as a steer's note
    /// shows it. Ok and Err both carry the text to show the editor.
    fn control(
        &self,
        cid: u64,
        work: Option<u64>,
        command: &str,
        arg: &str,
        from: &str,
    ) -> Result<String, String>;
    /// The model agent `cid` runs now, and the models its tier allows.
    fn models(&self, cid: u64) -> (String, Vec<String>);
    /// The context window of agent `cid`'s model, in tokens; None when it is not known.
    fn context_window(&self, _cid: u64) -> Option<u64> {
        None
    }
    /// Sets agent `cid`'s model, when its tier allows it.
    fn set_model(&self, cid: u64, model: &str) -> Result<(), String>;
    /// The open pull request of repository `name` whose head branch is `head`, with its changed text
    /// files. None when there is none, or when the forge does not answer: the turn then reports no
    /// pull request rather than failing.
    fn repo_pull(&self, name: &str, head: &str) -> Option<RepoPull>;
    /// Whether the owner has turned on write-back: an editor that can write files may apply a pull
    /// request's files to its project, on the owner's yes in the editor.
    fn write_back(&self) -> bool;
    fn now_ms(&self) -> u64;
}

/// A pull request a session's work opened on its repository (see [`Hub::repo_pull`]).
#[derive(Debug, Clone)]
pub struct RepoPull {
    pub url: String,
    pub number: u64,
    pub files: Vec<RepoFile>,
}

/// One changed file of a [`RepoPull`]. `path` is repository-relative, as the file is after the change;
/// a text is None on the side the file is not on (added or removed).
#[derive(Debug, Clone)]
pub struct RepoFile {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
}

/// Who an editor connection acts for: the owner, or a hub MCP token of operate or govern scope and the
/// agents its allowlist names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    Owner,
    Token {
        id: String,
        /// The token's name; its id stands in when the name is empty.
        name: String,
        scope: Scope,
        /// The agents the token's `acp:<name or cid>` entries name, without the prefix. Empty means every agent.
        agents: Vec<String>,
    },
}

impl Caller {
    /// The `opener` of a session this caller opens: `owner`, or `token:<id>`.
    pub fn opener(&self) -> String {
        match self {
            Caller::Owner => "owner".to_string(),
            Caller::Token { id, .. } => format!("token:{id}"),
        }
    }

    /// Whether the caller may use `session`: the owner every session, a token only the ones it opened.
    pub fn owns(&self, session: &Session) -> bool {
        match self {
            Caller::Owner => true,
            Caller::Token { .. } => session.opener == self.opener(),
        }
    }

    /// Who a steer from this caller is noted as: `owner`, or the editor and the token it acts with.
    pub fn label(&self) -> String {
        match self {
            Caller::Owner => "owner".to_string(),
            Caller::Token { id, name, .. } => {
                let who = if name.is_empty() { id } else { name };
                format!("editor (token {who})")
            }
        }
    }

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
    /// Whether the editor's `initialize` advertised `fs.writeTextFile`: it can apply a pull request's files.
    pub editor_writes: &'h AtomicBool,
    /// The protocol version `initialize` negotiated: 1, or 2 for an editor on the ACP v2 draft.
    pub version: &'h AtomicU16,
    /// Whether the owner has turned on the ACP v2 draft (`acp_v2`): only then may an editor ask for it.
    pub allow_v2: bool,
    /// Runs a v2 prompt's turn once its reply is out, on a thread of its own (see [`Detached`]).
    pub detach: &'h dyn Fn(Detached),
    pub caller: Caller,
    /// The connection's running v2 turns, which it caps at [`MAX_TURNS`] (see [`TurnSlot`]).
    pub turns: &'h Arc<AtomicUsize>,
}

impl Conn<'_> {
    /// Whether this connection speaks the ACP v2 draft.
    fn is_v2(&self) -> bool {
        self.version.load(Ordering::Relaxed) == 2
    }

    /// How the updates of `work` (or of no work) reach this connection's editor.
    fn wire(&self, work: Option<u64>) -> Wire {
        Wire {
            v2: self.is_v2(),
            work,
        }
    }
}

/// A v2 prompt's turn, handed to a thread of its own: it gets the hub and the editor's peer when it runs.
/// The request is answered before the turn runs, so the turn's updates follow the reply.
pub type Detached = Box<dyn FnOnce(&dyn Hub, &Peer) + Send>;

/// The most v2 prompts one connection runs at once. A prompt past that is refused until a turn ends.
const MAX_TURNS: usize = 4;

/// One of a connection's running v2 turns. It is taken when the prompt is answered and given back when the
/// turn ends; the drop does that on a panic too, since the turn's thread unwinds through it.
pub struct TurnSlot(Arc<AtomicUsize>);

impl TurnSlot {
    /// A slot, or None when the connection already runs [`MAX_TURNS`] turns.
    pub fn take(running: &Arc<AtomicUsize>) -> Option<TurnSlot> {
        let mut n = running.load(Ordering::SeqCst);
        loop {
            if n >= MAX_TURNS {
                return None;
            }
            match running.compare_exchange_weak(n, n + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(TurnSlot(Arc::clone(running))),
                Err(now) => n = now,
            }
        }
    }
}

impl Drop for TurnSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How one update reaches the editor. v1 sends it as it is. v2 sends it as v2 updates (see
/// [`apz_acp::compat::update_to_v2`]), and a chunk takes the message id of the work it belongs to; without a
/// work (a command's reply, the commands) the chunk keeps the fixed id the translation gives it.
#[derive(Debug, Clone, Copy, Default)]
struct Wire {
    v2: bool,
    work: Option<u64>,
}

/// How often the stream checks the editor and the approvals when no transcript frame came.
const POLL: Duration = Duration::from_millis(200);
/// How often the approvals are checked on their own, between transcript frames.
const APPROVALS_EVERY: Duration = Duration::from_secs(2);
/// How long the editor has to answer an approval before its request is left pending.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long the owner has to say, in the editor, whether to apply a pull request to the project.
const APPLY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long the editor has to write one file of an applied pull request.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// The most attached file text one prompt carries; the rest of the files are left out.
pub(crate) const PROMPT_BYTES: usize = 256 * 1024;
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
    // v2 has neither `session/load` nor `session/set_mode`: a v2 editor does not ask for them.
    if conn.is_v2() && (method == names.session_load || method == names.session_set_mode) {
        return Err(types::Error::method_not_found());
    }
    if method == names.initialize {
        initialize(conn, &params)
    } else if method == names.session_new {
        new_session(conn, peer, &params)
    } else if method == names.session_prompt {
        prompt(conn, peer, &params)
    } else if method == names.session_load {
        load(conn, peer, &params)
    } else if method == names.session_resume {
        resume(conn, peer, &params)
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

/// `initialize`: protocol 1, or protocol 2 when the owner allows the v2 draft and the editor asks for it. The
/// version is kept on the connection, and a v2 request reaches the v1 code as its v1 request, whose reply is
/// then translated back.
fn initialize(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    let requested = params["protocolVersion"]
        .as_u64()
        .and_then(|version| u16::try_from(version).ok())
        .unwrap_or(1);
    let version = negotiate(requested, conn.allow_v2);
    conn.version.store(version, Ordering::Relaxed);
    if version != 2 {
        return initialize_v1(conn, params);
    }
    let reply = initialize_v1(conn, &initialize_request_to_v1(params))?;
    Ok(initialize_response_to_v2(&reply))
}

/// The v1 `initialize`: the editor's client name and file capability are kept, and the reply is v1.
fn initialize_v1(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    let name = params["clientInfo"]["name"].as_str().unwrap_or_default();
    let version = params["clientInfo"]["version"].as_str().unwrap_or_default();
    *lock(conn.client) = format!("{name} {version}").trim().to_string();
    let writes = params["clientCapabilities"]["fs"]["writeTextFile"]
        .as_bool()
        .unwrap_or(false);
    conn.editor_writes.store(writes, Ordering::Relaxed);
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
    let session = conn
        .sessions
        .open(cid, cwd, &client, &conn.caller.opener(), conn.hub.now_ms());
    if let Some(link) = repo_link(conn, peer, &session.id, params) {
        conn.sessions.set_repo(&session.id, link);
    }
    // The notice goes before the reply: the editor reads it as the session's first message.
    if params["mcpServers"]
        .as_array()
        .is_some_and(|servers| !servers.is_empty())
    {
        notify_update(
            peer,
            &session.id,
            conn.wire(None),
            json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": MCP_NOTICE},
            }),
        );
    }
    let reply = json!({
        "sessionId": session.id,
        "modes": modes(conn.hub, cid),
        "configOptions": config_options(conn.hub, cid),
    });
    // v2 takes the commands in the reply itself, so no `available_commands_update` is sent for it.
    if conn.is_v2() {
        return Ok(new_session_response_to_v2(&reply, &commands()));
    }
    // Before the reply: the editor takes the commands as the session's, and a notification may come
    // ahead of the reply to `session/new`.
    notify_update(peer, &session.id, Wire::default(), commands_update());
    Ok(reply)
}

fn prompt(conn: &Conn<'_>, peer: &Peer, params: &Value) -> Result<Value, types::Error> {
    let mut session = open_session_of(conn, params)?;
    if let Some(link) = repo_link(conn, peer, &session.id, params) {
        conn.sessions.set_repo(&session.id, link.clone());
        session.repo = Some(link);
    }
    let text = prompt_text(&params["prompt"]);
    if let Some((command, arg)) = slash_command(&text) {
        return command_turn(conn, peer, &session, command, arg);
    }
    // A v2 turn takes one of the connection's slots before its work is queued, so a refused prompt queues nothing.
    let slot = if conn.is_v2() {
        Some(
            TurnSlot::take(conn.turns)
                .ok_or_else(|| invalid("too many prompts running on this connection"))?,
        )
    } else {
        None
    };
    // Subscribed before the work is queued: the frames wait in the channel until the stream reads
    // them, so none is lost while `chat` returns.
    let frames = conn.hub.subscribe();
    // The agent is told where the editor's project lives; the session keeps the editor's own text.
    let chat = match &session.repo {
        Some(repo) => format!("{}\n\n{text}", repo_context(&session.id, repo)),
        None => text.clone(),
    };
    let work = conn
        .hub
        .chat(session.cid, &session.id, &chat)
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
                conn.wire(Some(work)),
                json!({
                    "sessionUpdate": "session_info_update",
                    "title": titled.title,
                    "updatedAt": rfc3339(titled.updated_ms),
                }),
            );
        }
    }
    if conn.is_v2() {
        // The turn runs after the answer: its updates are the `state_update`s and the work's stream.
        let turn = Turn {
            session,
            work,
            frames,
            govern: conn.caller.may_govern(),
            _slot: slot,
        };
        (conn.detach)(Box::new(move |hub: &dyn Hub, peer: &Peer| {
            turn.run(hub, peer)
        }));
        return Ok(json!({"messageId": format!("prompt-{work}")}));
    }
    let result = stream(
        conn.hub,
        peer,
        &session,
        work,
        &frames,
        conn.caller.may_govern(),
        Wire::default(),
    );
    // The turn's tokens are reported whatever its end: a failed work spent them too.
    report_usage(conn.hub, peer, &session.id, session.cid, work);
    // A pull request the work opened is reported whatever the end too: it is on the repository.
    if let Some(repo) = &session.repo {
        if let Some(pull) = report_repo_pull(conn.hub, peer, &session, repo, Wire::default()) {
            offer_write_back(conn, peer, &session, &pull);
        }
    }
    result
}

/// A v2 prompt's turn: the work's stream as state and updates, once the editor has its answer. It ends with
/// an idle `state_update` that says why the work stopped, and the usage. A failed work says why in the chat
/// first and ends as `_failed`.
struct Turn {
    session: Session,
    work: u64,
    frames: Receiver<Frame>,
    govern: bool,
    /// Given back when the turn ends (its drop), so the connection may run another prompt.
    _slot: Option<TurnSlot>,
}

impl Turn {
    /// Runs the turn on `hub` for the editor at `peer`. Called on the turn's own thread (see [`Detached`]).
    fn run(self, hub: &dyn Hub, peer: &Peer) {
        let wire = Wire {
            v2: true,
            work: Some(self.work),
        };
        send_update(peer, &self.session.id, running_update());
        let result = stream(
            hub,
            peer,
            &self.session,
            self.work,
            &self.frames,
            self.govern,
            wire,
        );
        // A pull request the work opened is reported whatever the end, as on v1. v2 offers no write-back.
        if let Some(repo) = &self.session.repo {
            report_repo_pull(hub, peer, &self.session, repo, wire);
        }
        let stop = match &result {
            Ok(reply) => reply["stopReason"]
                .as_str()
                .unwrap_or("end_turn")
                .to_string(),
            Err(err) => {
                let said = err
                    .data
                    .as_ref()
                    .and_then(Value::as_str)
                    .unwrap_or(err.message.as_str());
                notify_update(
                    peer,
                    &self.session.id,
                    wire,
                    json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": said},
                    }),
                );
                "_failed".to_string()
            }
        };
        let usage = hub.usage(self.session.cid, self.work).map(|usage| {
            let input = usage["input_tokens"].as_u64().unwrap_or(0);
            let output = usage["output_tokens"].as_u64().unwrap_or(0);
            json!({
                "totalTokens": input + output,
                "inputTokens": input,
                "outputTokens": output,
            })
        });
        send_update(peer, &self.session.id, idle_update(&stop, usage.as_ref()));
    }
}

/// The repository the editor linked in `_meta.agentplaza.repo` for session `session_id`, when the caller may
/// govern: an operate token's link is dropped without a word. A link that the plaza does not take (see
/// [`repo_link_problem`]) is dropped too, and the editor is told why.
fn repo_link(conn: &Conn<'_>, peer: &Peer, session_id: &str, params: &Value) -> Option<RepoLink> {
    if !conn.caller.may_govern() {
        return None;
    }
    let link = repo_of(params)?;
    match repo_link_problem(&link) {
        None => Some(link),
        Some(why) => {
            notify_update(
                peer,
                session_id,
                conn.wire(None),
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": format!("the repository link was ignored: {why}")},
                }),
            );
            None
        }
    }
}

/// Why the plaza does not take `link`, or None when it does. The name is a plaza repository's name: lower case
/// letters, digits, `.`, `_` and `-`, at most 100 characters, starting with a letter or digit (so `.` and `..`
/// are refused too). The branch is an `editor/` branch of at most 200 characters, with no `..` and no `//`.
fn repo_link_problem(link: &RepoLink) -> Option<&'static str> {
    if !valid_repo_name(&link.name) {
        return Some("the name is not a plaza repository name");
    }
    if !valid_editor_branch(&link.branch) {
        return Some("the branch is not an editor/ branch name");
    }
    None
}

fn valid_repo_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 100
        && matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn valid_editor_branch(branch: &str) -> bool {
    let Some(rest) = branch.strip_prefix("editor/") else {
        return false;
    };
    (1..=200).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
        && !rest.contains("..")
        && !rest.contains("//")
}

/// The repository the editor linked in `_meta.agentplaza.repo`, when it names a repository and a branch.
fn repo_of(params: &Value) -> Option<RepoLink> {
    let repo = &params["_meta"]["agentplaza"]["repo"];
    let name = repo["name"].as_str().filter(|s| !s.is_empty())?;
    let branch = repo["branch"].as_str().filter(|s| !s.is_empty())?;
    Some(RepoLink {
        name: name.to_string(),
        branch: branch.to_string(),
        commit: repo["commit"].as_str().unwrap_or_default().to_string(),
        dirty: repo["dirty"].as_bool().unwrap_or(false),
    })
}

/// The note put before the editor's text on a session linked to a repository: where the project is
/// mirrored, and what to do with the work (a branch of its own, and a pull request into the editor's).
fn repo_context(session_id: &str, repo: &RepoLink) -> String {
    let mut facts = format!("branch {}", repo.branch);
    if !repo.commit.is_empty() {
        let short: String = repo.commit.chars().take(7).collect();
        facts.push_str(&format!(", commit {short}"));
    }
    if repo.dirty {
        facts.push_str("; the editor had uncommitted changes that are not included");
    }
    format!(
        "The editor's project is mirrored to the plaza repository {name} ({facts}). Clone http://plaza.gateway/git/agentplaza/{name}.git, check out {branch}, and work on a branch acp/{session_id} made from it. Commit, push that branch, and open a pull request into {branch} with plaza_git (or update the open one). End your reply with the pull request's link.",
        name = repo.name,
        branch = repo.branch,
    )
}

/// Shows the editor the pull request the session's work opened on its repository: a completed
/// `tool_call` with each changed file's diff, and a message with the link. Nothing when there is none.
/// Returns the pull request it showed, for the write-back.
fn report_repo_pull(
    hub: &dyn Hub,
    peer: &Peer,
    session: &Session,
    repo: &RepoLink,
    wire: Wire,
) -> Option<RepoPull> {
    let head = format!("acp/{}", session.id);
    let pull = hub.repo_pull(&repo.name, &head)?;
    let mut content = Vec::new();
    let mut locations = Vec::new();
    for file in &pull.files {
        // A file that would leave the project is not shown.
        let Some(path) = project_path(&session.cwd, &file.path) else {
            continue;
        };
        content.push(json!({
            "type": "diff",
            "path": path,
            "oldText": file.old_text,
            "newText": file.new_text,
        }));
        locations.push(json!({"path": path}));
    }
    notify_update(
        peer,
        &session.id,
        wire,
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": format!("pr-{}", pull.number),
            "title": format!("Pull request #{} on {}", pull.number, repo.name),
            "kind": "edit",
            "status": "completed",
            "content": content,
            "locations": locations,
        }),
    );
    notify_update(
        peer,
        &session.id,
        wire,
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": format!("Pull request #{}: {}", pull.number, pull.url)},
        }),
    );
    Some(pull)
}

/// The absolute path of a pull request's file in the editor's project. ACP wants absolute paths, and the
/// project root is the repository's root. None for a file that would leave the project, and for a
/// project root that is not absolute.
fn project_path(cwd: &str, path: &str) -> Option<String> {
    let root = Path::new(cwd);
    (root.is_absolute() && repo_relative(path))
        .then(|| root.join(path).to_string_lossy().into_owned())
}

/// Whether `path` names a file inside the repository: relative, with no `..`, root or prefix part.
fn repo_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Asks the editor whether to apply the pull request's changed files to the project, and writes them
/// on the owner's yes. The editor does the writing (`fs/write_text_file`), file by file: the agent never
/// touches the project. Nothing is asked unless write-back is on, the editor can write files, the caller
/// may govern, and a changed file has new text. A deleted file is left for the owner to remove.
/// Never on a v2 connection: v2 has no `fs/write_text_file`, so the editor cannot write the project.
fn offer_write_back(conn: &Conn<'_>, peer: &Peer, session: &Session, pull: &RepoPull) {
    if conn.is_v2()
        || !conn.hub.write_back()
        || !conn.editor_writes.load(Ordering::Relaxed)
        || !conn.caller.may_govern()
    {
        return;
    }
    let mut writes = Vec::new();
    let mut deleted = Vec::new();
    for file in &pull.files {
        let Some(path) = project_path(&session.cwd, &file.path) else {
            continue;
        };
        match &file.new_text {
            Some(text) => writes.push((path, text.as_str())),
            None => deleted.push(path),
        }
    }
    if writes.is_empty() {
        return;
    }
    let id = format!("apply-pr-{}", pull.number);
    let locations: Vec<Value> = writes
        .iter()
        .map(|(path, _)| json!({"path": path}))
        .collect();
    let asked = peer.call(
        CLIENT_METHOD_NAMES.session_request_permission,
        json!({
            "sessionId": session.id,
            "toolCall": {
                "toolCallId": id,
                "title": format!(
                    "Apply pull request #{} to your project ({} files)",
                    pull.number,
                    writes.len(),
                ),
                "kind": "edit",
                "status": "pending",
                "locations": locations,
            },
            "options": [
                {"optionId": "apply", "name": "Apply to my project", "kind": "allow_once"},
                {"optionId": "skip", "name": "Keep it on the branch", "kind": "reject_once"},
            ],
        }),
        APPLY_TIMEOUT,
    );
    // A cancelled or failed ask writes nothing; the pull request stays on its branch.
    let Ok(reply) = asked else {
        return;
    };
    let outcome = &reply["outcome"];
    if outcome["outcome"].as_str() != Some("selected")
        || outcome["optionId"].as_str() != Some("apply")
    {
        return;
    }
    let mut applied = 0;
    let mut failure = None;
    for (path, text) in &writes {
        let written = peer.call(
            CLIENT_METHOD_NAMES.fs_write_text_file,
            json!({"sessionId": session.id, "path": path, "content": text}),
            WRITE_TIMEOUT,
        );
        if let Err(err) = written {
            failure = Some(format!("{path}: {err}"));
            break;
        }
        applied += 1;
    }
    let mut said = match &failure {
        None => format!("Applied {applied} files"),
        Some(failure) => format!(
            "Applied {applied} of {} files; writing {failure} failed",
            writes.len()
        ),
    };
    if !deleted.is_empty() {
        said.push_str(&format!(
            "; {} deleted files were left for you to remove: {}",
            deleted.len(),
            deleted.join(", "),
        ));
    }
    said.push('.');
    notify_update(
        peer,
        &session.id,
        Wire::default(),
        json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": if failure.is_some() { "failed" } else { "completed" },
            "content": [{"type": "content", "content": {"type": "text", "text": said}}],
        }),
    );
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
    // Extending the turns spends the owner's model calls, so it needs govern; the refusal is the command's answer.
    let said = if command == "extend-turns" && !conn.caller.may_govern() {
        "this token's scope is operate; extending turns needs govern".to_string()
    } else {
        match conn
            .hub
            .control(session.cid, None, command, arg, &conn.caller.label())
        {
            Ok(said) | Err(said) => said,
        }
    };
    if !conn.is_v2() {
        notify_update(
            peer,
            &session.id,
            Wire::default(),
            json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": said},
            }),
        );
        return Ok(json!({"stopReason": "end_turn"}));
    }
    // A command queues no work, so there is no turn to run later: its states go out here, before the answer,
    // which carries a message id as a prompt's answer does. Its text keeps the fixed id of its kind.
    let now = conn.hub.now_ms();
    send_update(peer, &session.id, running_update());
    notify_update(
        peer,
        &session.id,
        conn.wire(None),
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": said},
        }),
    );
    send_update(peer, &session.id, idle_update("end_turn", None));
    Ok(json!({"messageId": format!("prompt-command-{now}")}))
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

/// Tells the editor the work's tokens, when it made any model call. `size` is the model's context
/// window, or 0 when the hub does not know it. v1 only: a v2 turn reports its usage on its idle state.
fn report_usage(hub: &dyn Hub, peer: &Peer, session_id: &str, cid: u64, work: u64) {
    let Some(usage) = hub.usage(cid, work) else {
        return;
    };
    let input = usage["input_tokens"].as_u64().unwrap_or(0);
    let output = usage["output_tokens"].as_u64().unwrap_or(0);
    let window = hub.context_window(cid);
    // `used` counts both parts; the engine part (the gateway's calls) is reported apart as well.
    notify_update(
        peer,
        session_id,
        Wire::default(),
        json!({
            "sessionUpdate": "usage_update",
            "used": input + output,
            "size": window.unwrap_or(0),
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
/// they stay pending for the owner to decide in the console. An answer to an ask is decided as it comes in,
/// while the turn runs; an answer that comes after the turn ends is dropped, and the owner decides it there.
fn stream(
    hub: &dyn Hub,
    peer: &Peer,
    session: &Session,
    work: u64,
    frames: &Receiver<Frame>,
    govern: bool,
    wire: Wire,
) -> Result<Value, types::Error> {
    let mut feed = Feed::new(session.cid, work);
    let mut asked = HashSet::new();
    let mut checked = Instant::now();
    let (answers_tx, answers_rx) = mpsc::channel();
    loop {
        decide_answers(hub, &answers_rx);
        match frames.recv_timeout(POLL) {
            Ok(frame) => {
                let data: Value = serde_json::from_str(&frame.data).unwrap_or(Value::Null);
                for update in feed.frame(&frame.kind, &data) {
                    notify_update(peer, &session.id, wire, update);
                }
                if frame.kind == "transcript" {
                    if govern {
                        ask_approvals(
                            peer,
                            hub,
                            session.cid,
                            &session.id,
                            &mut asked,
                            &answers_tx,
                            wire.v2,
                        );
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
            ask_approvals(
                peer,
                hub,
                session.cid,
                &session.id,
                &mut asked,
                &answers_tx,
                wire.v2,
            );
            checked = Instant::now();
        }
    }
}

/// Asks the editor for each approval the session's thread holds that has not been asked yet. Each ask runs
/// on a thread of its own and sends its answer on `answers`: the turn does not wait for it. The ask may wait
/// the editor's whole [`PERMISSION_TIMEOUT`]; the thread ends then, and nothing waits on it.
fn ask_approvals(
    peer: &Peer,
    hub: &dyn Hub,
    cid: u64,
    session_id: &str,
    asked: &mut HashSet<String>,
    answers: &Sender<(String, bool)>,
    v2: bool,
) {
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
        let (peer, answers) = (peer.clone(), answers.clone());
        thread::spawn(move || {
            if let Some(approve) = ask_approval(&peer, params, v2) {
                let _ = answers.send((approval_id, approve));
            }
        });
    }
}

/// Asks the editor one approval and returns its answer: `Some(true)` to allow it, `Some(false)` to reject it.
/// None when the ask is cancelled, fails or times out, and the approval then stays pending for the owner to
/// decide in the console. On v2 the ask and its answer are the v2 shapes.
fn ask_approval(peer: &Peer, params: Value, v2: bool) -> Option<bool> {
    let asked = if v2 {
        permission_request_to_v2(&params)
    } else {
        params
    };
    let reply = peer
        .call(
            CLIENT_METHOD_NAMES.session_request_permission,
            asked,
            PERMISSION_TIMEOUT,
        )
        .ok()?;
    let reply = if v2 {
        permission_response_to_v1(&reply)
    } else {
        reply
    };
    let outcome = &reply["outcome"];
    if outcome["outcome"].as_str() != Some("selected") {
        return None;
    }
    match outcome["optionId"].as_str() {
        Some("allow") => Some(true),
        Some("reject") => Some(false),
        _ => None,
    }
}

/// Decides the answers the asks have brought back so far, for the owner.
fn decide_answers(hub: &dyn Hub, answers: &Receiver<(String, bool)>) {
    while let Ok((approval_id, approve)) = answers.try_recv() {
        let _ = hub.decide(&approval_id, approve, "from the editor");
    }
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
    if let Some(link) = repo_link(conn, peer, &session.id, params) {
        conn.sessions.set_repo(&session.id, link);
    }
    let entries = conn.hub.entries(session.cid, &session.works);
    for &work in &session.works {
        for update in feed::replay(work, &entries) {
            notify_update(peer, &session.id, Wire::default(), update);
        }
    }
    notify_update(peer, &session.id, Wire::default(), commands_update());
    Ok(json!({
        "modes": modes(conn.hub, session.cid),
        "configOptions": config_options(conn.hub, session.cid),
    }))
}

fn resume(conn: &Conn<'_>, peer: &Peer, params: &Value) -> Result<Value, types::Error> {
    let session = session_of(conn, params)?;
    if let Some(link) = repo_link(conn, peer, &session.id, params) {
        conn.sessions.set_repo(&session.id, link);
    }
    Ok(json!({"modes": modes(conn.hub, session.cid)}))
}

fn list(conn: &Conn<'_>, params: &Value) -> Result<Value, types::Error> {
    let sessions: Vec<Value> = conn
        .sessions
        .list(params["cwd"].as_str())
        .iter()
        .filter(|s| conn.caller.owns(s) && conn.caller.may_drive(conn.hub, s.cid))
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
        Wire::default(),
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
    let options = config_options(conn.hub, session.cid);
    if conn.is_v2() {
        // v2 names the option's id `configId`: the v1 options are translated the way `session/new` does.
        let v2 = new_session_response_to_v2(&json!({"configOptions": options}), &[]);
        return Ok(json!({"configOptions": v2["configOptions"]}));
    }
    Ok(json!({"configOptions": options}))
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

/// The plaza's commands in v1 shape, for the editor's command list.
fn commands() -> Vec<Value> {
    COMMANDS
        .iter()
        .map(|(name, description, hint)| {
            let mut command = json!({"name": name, "description": description});
            if let Some(hint) = hint {
                command["input"] = json!({"hint": hint});
            }
            command
        })
        .collect()
}

fn commands_update() -> Value {
    json!({"sessionUpdate": "available_commands_update", "availableCommands": commands()})
}

/// The session `params` name, open or closed, when the caller may drive its agent. A session another caller
/// opened is "no session" too, the same error as an unknown id, so an id tells the caller nothing.
fn session_of(conn: &Conn<'_>, params: &Value) -> Result<Session, types::Error> {
    let id = params["sessionId"].as_str().unwrap_or_default();
    let session = conn
        .sessions
        .get(id)
        .filter(|s| conn.caller.owns(s))
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

/// Sends one update to the editor as it is. The v2 states (`state_update`) go out this way too.
fn send_update(peer: &Peer, session_id: &str, update: Value) {
    let _ = peer.notify(
        CLIENT_METHOD_NAMES.session_update,
        json!({"sessionId": session_id, "update": update}),
    );
}

/// Sends a v1 update to the editor, in the shape `wire` names: as it is on v1, and on v2 as the v2 updates
/// it becomes (none when v2 has no counterpart). Every update of a session goes through here.
fn notify_update(peer: &Peer, session_id: &str, wire: Wire, update: Value) {
    if !wire.v2 {
        send_update(peer, session_id, update);
        return;
    }
    for update in update_to_v2(&update) {
        send_update(peer, session_id, with_message_id(update, wire.work));
    }
}

/// A v2 chunk with the message id of its work: the agent's text and its thoughts, and the user's message, each
/// its own message. Other updates, and a chunk with no work, are left as they are.
fn with_message_id(mut update: Value, work: Option<u64>) -> Value {
    let Some(work) = work else {
        return update;
    };
    let id = match update["sessionUpdate"].as_str() {
        Some("agent_message_chunk") => format!("work-{work}"),
        Some("agent_thought_chunk") => format!("thought-{work}"),
        Some("user_message_chunk") => format!("prompt-{work}"),
        _ => return update,
    };
    update["messageId"] = json!(id);
    update
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
    use apz_acp::testing::{ClientConfig, FakeClient, PermissionAnswer};
    use apz_acp::types::{PermissionOptionKind, SessionId};

    use super::*;

    /// The plaza as the tests see it: a fake hub that records what the methods ask of it, and the
    /// real session store.
    struct Plaza {
        hub: FakeHub,
        sessions: Sessions,
        client: Mutex<String>,
        editor_writes: AtomicBool,
        version: AtomicU16,
        allow_v2: bool,
        agent: Option<String>,
        caller: Caller,
        turns: Arc<AtomicUsize>,
    }

    struct FakeHub {
        calls: Mutex<Vec<String>>,
        /// The bus receivers, one handed out per `subscribe`, first in first out; the test keeps the senders.
        frames: Mutex<Vec<Receiver<Frame>>>,
        mode: Mutex<String>,
        /// What `usage` reports for any work (None: no model call).
        usage: Mutex<Option<Value>>,
        /// What `models` reports: the current model and the tier's list.
        models: Mutex<(String, Vec<String>)>,
        /// The approvals the session's thread holds (each as the approvals API shows it).
        approvals: Mutex<Vec<Value>>,
        /// What `repo_pull` reports: the pull request the work opened, if any.
        pull: Mutex<Option<RepoPull>>,
        /// What `write_back` reports: the owner's setting.
        write_back: Mutex<bool>,
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
            let mut frames = lock(&self.frames);
            assert!(!frames.is_empty(), "the fake hub has no receiver left");
            frames.remove(0)
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
            from: &str,
        ) -> Result<String, String> {
            self.record(format!(
                "control {cid} {work:?} {command}({arg}) from {from}"
            ));
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

        fn repo_pull(&self, name: &str, head: &str) -> Option<RepoPull> {
            self.record(format!("repo_pull {name} {head}"));
            lock(&self.pull).clone()
        }

        fn write_back(&self) -> bool {
            *lock(&self.write_back)
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
        let (plaza, frames) = plaza(agent, caller, false);
        let (server, client_in, client_out) = serve(&plaza);
        let client = FakeClient::spawn(client_in, client_out, config);
        Harness {
            client,
            plaza,
            frames,
            _server: server,
        }
    }

    /// The plaza a connection serves: a fake hub with the bus's sender kept by the test, and whether the
    /// owner allows the v2 draft.
    fn plaza(agent: Option<&str>, caller: Caller, allow_v2: bool) -> (Arc<Plaza>, Sender<Frame>) {
        let (frames, rx) = mpsc::channel();
        let plaza = Arc::new(Plaza {
            hub: FakeHub {
                calls: Mutex::default(),
                frames: Mutex::new(vec![rx]),
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
                pull: Mutex::default(),
                write_back: Mutex::new(false),
            },
            sessions: Sessions::default(),
            client: Mutex::default(),
            editor_writes: AtomicBool::new(false),
            version: AtomicU16::new(1),
            allow_v2,
            agent: agent.map(str::to_string),
            caller,
            turns: Arc::default(),
        });
        (plaza, frames)
    }

    /// The agent's side of a connection to `plaza`: the server Peer, and the editor's ends of its two pipes
    /// (what the editor reads, and what it writes). Pipes, not a socket pair: the hub builds and tests on
    /// Windows too.
    fn serve(plaza: &Arc<Plaza>) -> (Peer, std::io::PipeReader, std::io::PipeWriter) {
        let (server_in, client_out) = std::io::pipe().expect("a pipe");
        let (client_in, server_out) = std::io::pipe().expect("a pipe");
        let for_requests = Arc::clone(plaza);
        let for_notes = Arc::clone(plaza);
        let server = Peer::new(
            server_in,
            server_out,
            move |peer: &Peer, method: &str, params: Value| {
                let detach = detacher_for(&for_requests, peer);
                request(&conn_of(&for_requests, &detach), peer, method, params)
            },
            move |peer: &Peer, method: &str, params: Value| {
                let detach = detacher_for(&for_notes, peer);
                notification(&conn_of(&for_notes, &detach), method, params);
            },
        );
        (server, client_in, client_out)
    }

    /// How the tests run a v2 turn: on a thread of its own, with the plaza's hub, as the socket does.
    fn detacher_for(plaza: &Arc<Plaza>, peer: &Peer) -> impl Fn(Detached) {
        let (plaza, peer) = (Arc::clone(plaza), peer.clone());
        move |turn| {
            let (plaza, peer) = (Arc::clone(&plaza), peer.clone());
            thread::spawn(move || {
                let hub: &dyn Hub = &plaza.hub;
                turn(hub, &peer);
            });
        }
    }

    fn conn_of<'a>(plaza: &'a Plaza, detach: &'a dyn Fn(Detached)) -> Conn<'a> {
        Conn {
            hub: &plaza.hub,
            sessions: &plaza.sessions,
            agent: plaza.agent.clone(),
            client: &plaza.client,
            editor_writes: &plaza.editor_writes,
            version: &plaza.version,
            allow_v2: plaza.allow_v2,
            detach,
            caller: plaza.caller.clone(),
            turns: &plaza.turns,
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    /// An operate token with no allowlist: it may drive every agent.
    fn operate() -> Caller {
        Caller::Token {
            id: "mcp_test".into(),
            name: "editor".into(),
            scope: Scope::Operate,
            agents: Vec::new(),
        }
    }

    /// The work's end on the bus, as the agent's transcript sends it.
    fn work_end() -> Frame {
        Frame {
            kind: "transcript".to_string(),
            queued: std::time::Instant::now(),
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

    /// Prompts `session` on a thread, and ends the work once the answer to approval `ap1` is decided. The
    /// answer comes while the turn runs, so the turn is still open when it is decided.
    fn decide_during_the_turn(h: &Harness, session: SessionId) {
        let client = h.client.clone();
        let prompt = thread::spawn(move || client.prompt(&session, "write it"));
        let start = Instant::now();
        while !h
            .plaza
            .hub
            .calls()
            .contains(&"decide ap1 true from the editor".to_string())
        {
            assert!(
                start.elapsed() < WAIT,
                "the answer was not decided: {:?}",
                h.plaza.hub.calls()
            );
            thread::sleep(Duration::from_millis(5));
        }
        h.frames.send(work_end()).expect("the bus is open");
        prompt
            .join()
            .expect("the prompt thread")
            .expect("session/prompt");
    }

    #[test]
    fn the_owner_is_asked_the_approval_and_the_answer_decides_it() {
        // The control for the test above: the same approval, asked when the caller may govern.
        let h = harness_with(None, Caller::Owner, ClientConfig::default());
        *lock(&h.plaza.hub.approvals) = vec![json!({"id": "ap1", "subject": "Write a file"})];
        let session = h.client.new_session("/work/app").expect("session/new");
        decide_during_the_turn(&h, session);
    }

    #[test]
    fn a_govern_token_is_asked_the_approval_and_changes_the_mode() {
        let govern = Caller::Token {
            id: "mcp_test".into(),
            name: "editor".into(),
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
        decide_during_the_turn(&h, session);
    }

    #[test]
    fn an_allowlist_names_the_agents_a_token_may_drive_by_name_or_cid() {
        let h = harness(None);
        let named = Caller::Token {
            id: "mcp_test".into(),
            name: "editor".into(),
            scope: Scope::Operate,
            agents: vec!["research".into()],
        };
        let by_cid = Caller::Token {
            id: "mcp_test".into(),
            name: "editor".into(),
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
            name: "editor".into(),
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
    fn a_token_uses_only_the_sessions_it_opened_and_another_id_looks_the_same() {
        let mine = Caller::Token {
            id: "mcp_a".into(),
            name: "laptop".into(),
            scope: Scope::Operate,
            agents: Vec::new(),
        };
        let h = harness_with(None, mine, ClientConfig::default());
        h.client.new_session("/work/app").expect("session/new");
        let own = h.plaza.sessions.list(None)[0].id.clone();
        let owners = h.plaza.sessions.open(7, "/work/app", "", "owner", 0).id;
        let others = h
            .plaza
            .sessions
            .open(7, "/work/app", "", "token:mcp_b", 0)
            .id;
        let listed = h
            .client
            .peer()
            .call("session/list", json!({}), WAIT)
            .expect("session/list");
        let ids: Vec<&str> = listed["sessions"]
            .as_array()
            .expect("the sessions")
            .iter()
            .filter_map(|s| s["sessionId"].as_str())
            .collect();
        assert_eq!(ids, [own.as_str()]);
        // The owner's session, another token's and an unknown id are refused the same way.
        for id in [owners.as_str(), others.as_str(), "acp-nope"] {
            for (method, params) in [
                (
                    "session/load",
                    json!({"sessionId": id, "cwd": "/work/app", "mcpServers": []}),
                ),
                ("session/resume", json!({"sessionId": id})),
                (
                    "session/prompt",
                    json!({"sessionId": id, "prompt": [{"type": "text", "text": "hi"}]}),
                ),
                ("session/close", json!({"sessionId": id})),
            ] {
                let err = h
                    .client
                    .peer()
                    .call(method, params, WAIT)
                    .expect_err("another caller's session is refused");
                assert!(matches!(err, rpc::Error::Remote(_)), "{method}: {err:?}");
                assert!(
                    format!("{err:?}").contains(&format!("no session {id}")),
                    "{method}: {err:?}"
                );
            }
        }
        assert!(!h.plaza.sessions.get(&owners).unwrap().closed);
        assert!(!h.plaza.sessions.get(&others).unwrap().closed);
        // Its own session is still its to use.
        h.client
            .peer()
            .call("session/resume", json!({"sessionId": own}), WAIT)
            .expect("its own session resumes");
    }

    #[test]
    fn a_repo_link_needs_govern_and_a_name_the_plaza_does_not_take_is_ignored_with_why() {
        // An operate token's link is dropped without a word.
        let operating = harness_with(None, operate(), ClientConfig::default());
        let mut params = repo_meta("app", "editor/main", "", false);
        params["cwd"] = json!("/work/app");
        let created = operating
            .client
            .peer()
            .call("session/new", params, WAIT)
            .expect("session/new");
        let id = created["sessionId"].as_str().expect("a session id");
        assert_eq!(operating.plaza.sessions.get(id).unwrap().repo, None);
        let said = serde_json::to_string(&operating.client.updates()).expect("serialize");
        assert!(!said.contains("repository link"), "{said}");

        // A govern token's link is kept when the plaza takes it, and one it does not take is ignored with why.
        let govern = Caller::Token {
            id: "mcp_test".into(),
            name: "editor".into(),
            scope: Scope::Govern,
            agents: Vec::new(),
        };
        let h = harness_with(None, govern, ClientConfig::default());
        for (name, branch) in [
            ("Bad Name!", "editor/main"),
            ("..", "editor/main"),
            ("app", "main"),
            ("app", "editor/a..b"),
        ] {
            let mut params = repo_meta(name, branch, "", false);
            params["cwd"] = json!("/work/app");
            let created = h
                .client
                .peer()
                .call("session/new", params, WAIT)
                .expect("session/new");
            let id = created["sessionId"].as_str().expect("a session id");
            assert_eq!(
                h.plaza.sessions.get(id).unwrap().repo,
                None,
                "{name} {branch}"
            );
        }
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(
            said.contains(
                "the repository link was ignored: the name is not a plaza repository name"
            ),
            "{said}"
        );
        assert!(
            said.contains(
                "the repository link was ignored: the branch is not an editor/ branch name"
            ),
            "{said}"
        );
        let mut good = repo_meta("app", "editor/main", "abc1234def", false);
        good["cwd"] = json!("/work/app");
        let created = h
            .client
            .peer()
            .call("session/new", good, WAIT)
            .expect("session/new");
        let id = created["sessionId"].as_str().expect("a session id");
        assert_eq!(
            h.plaza.sessions.get(id).unwrap().repo.map(|link| link.name),
            Some("app".to_string())
        );
    }

    #[test]
    fn a_turn_ends_without_waiting_for_an_unanswered_ask() {
        let (plaza, frames) = plaza(None, Caller::Owner, false);
        let (_server, client_in, client_out) = serve(&plaza);
        // The editor holds its answer to the permission request until the test lets it go.
        let (release, held) = mpsc::channel::<()>();
        let held = Mutex::new(held);
        let editor = Peer::new(
            client_in,
            client_out,
            move |_peer: &Peer, method: &str, _params: Value| {
                if method != CLIENT_METHOD_NAMES.session_request_permission {
                    return Err(types::Error::method_not_found());
                }
                let _ = lock(&held).recv_timeout(WAIT);
                Ok(json!({"outcome": {"outcome": "selected", "optionId": "allow"}}))
            },
            |_peer: &Peer, _method: &str, _params: Value| {},
        );
        let created = editor
            .call(
                AGENT_METHOD_NAMES.session_new,
                json!({"cwd": "/work/app", "mcpServers": []}),
                WAIT,
            )
            .expect("session/new");
        let session = created["sessionId"].as_str().expect("a session id");
        *lock(&plaza.hub.approvals) = vec![json!({"id": "ap1", "subject": "Write a file"})];
        frames.send(work_end()).expect("the bus is open");
        // The turn ends although the ask is unanswered: the work's end does not wait for the editor.
        let answer = editor
            .call(
                AGENT_METHOD_NAMES.session_prompt,
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "write it"}]}),
                WAIT,
            )
            .expect("the turn ends");
        assert_eq!(answer["stopReason"], "end_turn");
        // The editor's answer comes after the turn, so it is dropped: the owner decides in the console.
        release.send(()).expect("the editor is waiting");
        thread::sleep(Duration::from_millis(100));
        let calls = plaza.hub.calls();
        assert!(!calls.iter().any(|c| c.starts_with("decide")), "{calls:?}");
    }

    #[test]
    fn a_token_steers_as_the_editor_and_cannot_extend_the_turns() {
        let h = harness_with(None, operate(), ClientConfig::default());
        let session = h.client.new_session("/work/app").expect("session/new");
        h.client
            .prompt(&session, "/steer use the other module")
            .expect("session/prompt");
        let calls = h.plaza.hub.calls();
        assert!(
            calls.contains(
                &"control 7 None steer(use the other module) from editor (token editor)"
                    .to_string()
            ),
            "{calls:?}"
        );
        h.client
            .prompt(&session, "/extend-turns 5")
            .expect("session/prompt");
        let calls = h.plaza.hub.calls();
        assert!(
            !calls
                .iter()
                .any(|c| c.starts_with("control 7 None extend-turns")),
            "{calls:?}"
        );
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(
            said.contains("this token's scope is operate; extending turns needs govern"),
            "{said}"
        );
    }

    #[test]
    fn a_fifth_v2_prompt_is_refused_while_four_turns_run() {
        let (plaza, editor, frames, _server) = v2_harness(true);
        // Each turn takes the next bus receiver; the test keeps the sender of each one.
        let extra: Vec<Sender<Frame>> = (0..4)
            .map(|_| {
                let (tx, rx) = mpsc::channel();
                lock(&plaza.hub.frames).push(rx);
                tx
            })
            .collect();
        editor
            .peer
            .call("initialize", initialize_v2(), WAIT)
            .expect("initialize");
        let session = editor
            .peer
            .call(
                "session/new",
                json!({"cwd": "/work/app", "mcpServers": []}),
                WAIT,
            )
            .expect("session/new")["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string();
        let prompt =
            |text: &str| json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]});
        for n in 0..4 {
            editor
                .peer
                .call("session/prompt", prompt(&format!("turn {n}")), WAIT)
                .expect("a prompt under the cap is answered");
        }
        let refused = editor
            .peer
            .call("session/prompt", prompt("turn 4"), WAIT)
            .expect_err("a fifth prompt is refused");
        assert!(
            format!("{refused:?}").contains("too many prompts running on this connection"),
            "{refused:?}"
        );
        // The four turns end; the connection then takes a prompt again.
        frames.send(work_end()).expect("the bus is open");
        for tx in &extra[..3] {
            tx.send(work_end()).expect("the bus is open");
        }
        wait_for_updates(&editor.updates, |seen| {
            seen.iter().filter(|u| u["state"] == "idle").count() >= 4
        });
        editor
            .peer
            .call("session/prompt", prompt("turn 5"), WAIT)
            .expect("a prompt after the turns end is answered");
        let chats = plaza
            .hub
            .calls()
            .iter()
            .filter(|c| c.starts_with("chat "))
            .count();
        assert_eq!(chats, 5);
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

    /// An editor on the ACP v2 draft's side of a connection: a Peer that records each session update as it
    /// arrives, as the wire has it (v2 states included).
    struct RawEditor {
        peer: Peer,
        updates: Arc<Mutex<Vec<Value>>>,
    }

    fn raw_editor(client_in: std::io::PipeReader, client_out: std::io::PipeWriter) -> RawEditor {
        let updates: Arc<Mutex<Vec<Value>>> = Arc::default();
        let recorded = Arc::clone(&updates);
        let peer = Peer::new(
            client_in,
            client_out,
            |_peer: &Peer, _method: &str, _params: Value| Err(types::Error::method_not_found()),
            move |_peer: &Peer, method: &str, params: Value| {
                if method == CLIENT_METHOD_NAMES.session_update {
                    lock(&recorded).push(params["update"].clone());
                }
            },
        );
        RawEditor { peer, updates }
    }

    /// A connection to a fake plaza whose owner allows the v2 draft or not, with the editor on its end.
    fn v2_harness(allow_v2: bool) -> (Arc<Plaza>, RawEditor, Sender<Frame>, Peer) {
        let (plaza, frames) = plaza(None, Caller::Owner, allow_v2);
        let (server, client_in, client_out) = serve(&plaza);
        (plaza, raw_editor(client_in, client_out), frames, server)
    }

    /// The `initialize` request an editor on the v2 draft sends.
    fn initialize_v2() -> Value {
        json!({
            "protocolVersion": 2,
            "info": {"name": "zed", "version": "0.300"},
            "capabilities": {},
        })
    }

    /// Waits until `done` holds of the updates the editor has, and returns them; the test fails after `WAIT`.
    fn wait_for_updates(
        updates: &Mutex<Vec<Value>>,
        done: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        let start = Instant::now();
        loop {
            let seen = lock(updates).clone();
            if done(&seen) {
                return seen;
            }
            assert!(start.elapsed() < WAIT, "the updates did not come: {seen:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn initialize_answers_protocol_two_when_the_owner_allows_it() {
        let (plaza, editor, _frames, _server) = v2_harness(true);
        let reply = editor
            .peer
            .call("initialize", initialize_v2(), WAIT)
            .expect("initialize");
        serde_json::from_value::<types::v2::InitializeResponse>(reply.clone()).expect("a v2 reply");
        assert_eq!(reply["protocolVersion"], 2);
        assert_eq!(reply["info"]["name"], "agentplaza");
        assert_eq!(*lock(&plaza.client), "zed 0.300");
        assert_eq!(plaza.version.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn initialize_answers_protocol_one_when_the_owner_has_not_allowed_v2() {
        let (plaza, editor, _frames, _server) = v2_harness(false);
        let reply = editor
            .peer
            .call("initialize", initialize_v2(), WAIT)
            .expect("initialize");
        assert_eq!(reply["protocolVersion"], 1);
        serde_json::from_value::<types::InitializeResponse>(reply).expect("a v1 reply");
        assert_eq!(plaza.version.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn v2_session_new_carries_the_commands_and_no_update_for_them() {
        let (_plaza, editor, _frames, _server) = v2_harness(true);
        editor
            .peer
            .call("initialize", initialize_v2(), WAIT)
            .expect("initialize");
        let reply = editor
            .peer
            .call(
                "session/new",
                json!({"cwd": "/work/app", "mcpServers": []}),
                WAIT,
            )
            .expect("session/new");
        assert!(reply["sessionId"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(
            reply["availableCommands"]
                .as_array()
                .is_some_and(|commands| !commands.is_empty())
        );
        assert!(reply.get("modes").is_none(), "v2 has no modes");
        assert!(
            lock(&editor.updates)
                .iter()
                .all(|update| update["sessionUpdate"] != "available_commands_update"),
            "the commands come in the reply"
        );
    }

    #[test]
    fn a_v2_prompt_is_answered_at_once_and_its_turn_follows_as_states() {
        let (_plaza, editor, frames, _server) = v2_harness(true);
        editor
            .peer
            .call("initialize", initialize_v2(), WAIT)
            .expect("initialize");
        let session = editor
            .peer
            .call(
                "session/new",
                json!({"cwd": "/work/app", "mcpServers": []}),
                WAIT,
            )
            .expect("session/new")["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string();
        // The answer comes before the work has any frame: the turn runs after it.
        let answer = editor
            .peer
            .call(
                "session/prompt",
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}),
                WAIT,
            )
            .expect("session/prompt");
        assert_eq!(answer, json!({"messageId": "prompt-42"}));
        // The turn starts with `running` and waits for the work.
        let before = wait_for_updates(&editor.updates, |seen| {
            seen.iter().any(|u| u["state"] == "running")
        });
        assert!(
            before.iter().all(|u| u["state"] != "idle"),
            "the turn is not over before its work ends"
        );
        frames.send(work_end()).expect("the work ends");
        let seen = wait_for_updates(&editor.updates, |seen| {
            seen.iter().any(|u| u["state"] == "idle")
        });
        let states: Vec<&Value> = seen
            .iter()
            .filter(|u| u["sessionUpdate"] == "state_update")
            .collect();
        assert_eq!(
            states.first().map(|u| u["state"].clone()),
            Some(json!("running"))
        );
        let last = states.last().expect("a state");
        assert_eq!(last["state"], "idle");
        assert_eq!(last["stopReason"], "end_turn");
    }

    #[test]
    fn v2_does_not_offer_load_or_set_mode() {
        let (_plaza, editor, _frames, _server) = v2_harness(true);
        editor
            .peer
            .call("initialize", initialize_v2(), WAIT)
            .expect("initialize");
        let load = editor
            .peer
            .call(
                "session/load",
                json!({"sessionId": "acp-none", "cwd": "/work/app", "mcpServers": []}),
                WAIT,
            )
            .expect_err("no load on v2");
        assert!(matches!(load, rpc::Error::Remote(_)), "{load:?}");
        let mode = editor
            .peer
            .call(
                "session/set_mode",
                json!({"sessionId": "acp-none", "modeId": "ask"}),
                WAIT,
            )
            .expect_err("no set_mode on v2");
        assert!(matches!(mode, rpc::Error::Remote(_)), "{mode:?}");
    }

    #[test]
    fn a_v2_chunk_takes_the_message_id_of_its_work() {
        let text = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}});
        assert_eq!(
            with_message_id(text.clone(), Some(42))["messageId"],
            "work-42"
        );
        let thought = json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}});
        assert_eq!(
            with_message_id(thought, Some(42))["messageId"],
            "thought-42"
        );
        let user = json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "hi"}});
        assert_eq!(with_message_id(user, Some(42))["messageId"], "prompt-42");
        // Without a work the translation's fixed id stays.
        let fixed = with_message_id(update_to_v2(&text).remove(0), None);
        assert_eq!(fixed["messageId"], "agent");
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
        let session = h.plaza.sessions.open(7, "/work/app", "", "owner", 0);
        h.plaza.sessions.add_work(&session.id, 42, "fix it", 0);
        let detach = detacher_for(&h.plaza, &h._server);
        notification(
            &conn_of(&h.plaza, &detach),
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
            calls.contains(&"control 7 None stop() from owner".to_string()),
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
                queued: std::time::Instant::now(),
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

    /// The session's `_meta.agentplaza.repo` as an editor sends it.
    fn repo_meta(name: &str, branch: &str, commit: &str, dirty: bool) -> Value {
        json!({"_meta": {"agentplaza": {"repo": {
            "name": name, "branch": branch, "commit": commit, "dirty": dirty,
        }}}})
    }

    /// Calls `method` with `params` merged over `{sessionId}`, and answers the result.
    fn call_with(h: &Harness, method: &str, session: &str, mut params: Value) -> Value {
        params["sessionId"] = json!(session);
        h.client
            .peer()
            .call(method, params, WAIT)
            .expect("the call succeeds")
    }

    #[test]
    fn a_repo_link_is_recorded_from_new_session_and_the_latest_wins() {
        let h = harness(None);
        let mut params = repo_meta("app", "editor/main", "abc1234def", false);
        params["cwd"] = json!("/work/app");
        let created = h
            .client
            .peer()
            .call("session/new", params, WAIT)
            .expect("session/new");
        let id = created["sessionId"].as_str().expect("a session id");
        assert_eq!(
            h.plaza.sessions.get(id).unwrap().repo,
            Some(RepoLink {
                name: "app".into(),
                branch: "editor/main".into(),
                commit: "abc1234def".into(),
                dirty: false,
            })
        );
        // A later resume that names another repository replaces the link; one that names none keeps it.
        call_with(
            &h,
            "session/resume",
            id,
            repo_meta("site", "editor/dev", "", true),
        );
        call_with(&h, "session/resume", id, json!({}));
        let link = h.plaza.sessions.get(id).unwrap().repo.unwrap();
        assert_eq!(
            (link.name.as_str(), link.branch.as_str()),
            ("site", "editor/dev")
        );
        assert!(link.dirty);
    }

    #[test]
    fn the_repo_note_is_prepended_to_the_chat_only_for_a_linked_session() {
        let h = harness(None);
        let session = h.client.new_session("/work/app").expect("session/new");
        h.frames.send(work_end()).expect("the bus is open");
        h.client.prompt(&session, "hello").expect("session/prompt");
        assert!(
            h.plaza
                .hub
                .calls()
                .iter()
                .any(|c| c.starts_with("chat 7 acp-") && c.ends_with(" hello")),
            "{:?}",
            h.plaza.hub.calls()
        );
        assert!(
            !h.plaza.hub.calls().iter().any(|c| c.contains("mirrored")),
            "{:?}",
            h.plaza.hub.calls()
        );
        // A linked session: the link comes on the prompt, the note goes before the text, and the
        // title is the editor's own first line.
        let h = harness(None);
        let created = h
            .client
            .peer()
            .call("session/new", json!({"cwd": "/work/app"}), WAIT)
            .expect("session/new");
        let id = created["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string();
        h.frames.send(work_end()).expect("the bus is open");
        call_with(&h, "session/prompt", &id, {
            let mut p = repo_meta("app", "editor/main", "abc1234def", true);
            p["prompt"] = json!([{"type": "text", "text": "Fix the parser"}]);
            p
        });
        let calls = h.plaza.hub.calls();
        let note = format!(
            "chat 7 {id} The editor's project is mirrored to the plaza repository app (branch editor/main, commit abc1234; the editor had uncommitted changes that are not included). Clone http://plaza.gateway/git/agentplaza/app.git, check out editor/main, and work on a branch acp/{id} made from it. Commit, push that branch, and open a pull request into editor/main with plaza_git (or update the open one). End your reply with the pull request's link.\n\nFix the parser"
        );
        assert!(calls.contains(&note), "{calls:?}");
        assert_eq!(h.plaza.sessions.get(&id).unwrap().title, "Fix the parser");
        // No pull request on the repository: nothing is reported for it.
        assert!(
            calls.contains(&format!("repo_pull app acp/{id}")),
            "{calls:?}"
        );
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(!said.contains("\"sessionUpdate\":\"tool_call\""), "{said}");
    }

    #[test]
    fn the_pull_request_comes_back_as_one_tool_call_of_absolute_diffs() {
        let h = harness(None);
        let cwd = if cfg!(windows) {
            r"C:\work\app"
        } else {
            "/work/app"
        };
        *lock(&h.plaza.hub.pull) = Some(RepoPull {
            url: "http://plaza.gateway/git/agentplaza/app/pulls/3".to_string(),
            number: 3,
            files: vec![
                RepoFile {
                    path: "src/lib.rs".to_string(),
                    old_text: Some("old\n".to_string()),
                    new_text: Some("new\n".to_string()),
                },
                RepoFile {
                    path: "docs/new.txt".to_string(),
                    old_text: None,
                    new_text: Some("hi\n".to_string()),
                },
                RepoFile {
                    path: "../escape.txt".to_string(),
                    old_text: None,
                    new_text: Some("out\n".to_string()),
                },
                RepoFile {
                    path: "/etc/passwd".to_string(),
                    old_text: None,
                    new_text: Some("out\n".to_string()),
                },
            ],
        });
        let mut params = repo_meta("app", "editor/main", "", false);
        params["cwd"] = json!(cwd);
        let created = h
            .client
            .peer()
            .call("session/new", params, WAIT)
            .expect("session/new");
        let id = created["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string();
        h.frames.send(work_end()).expect("the bus is open");
        call_with(
            &h,
            "session/prompt",
            &id,
            json!({"prompt": [{"type": "text", "text": "Add the docs"}]}),
        );
        assert!(
            h.plaza
                .hub
                .calls()
                .contains(&format!("repo_pull app acp/{id}")),
            "{:?}",
            h.plaza.hub.calls()
        );
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        // One tool call, with the two safe files as diffs and locations.
        assert_eq!(
            said.matches("\"sessionUpdate\":\"tool_call\"").count(),
            1,
            "{said}"
        );
        assert!(said.contains("\"toolCallId\":\"pr-3\""), "{said}");
        assert!(
            said.contains("\"title\":\"Pull request #3 on app\""),
            "{said}"
        );
        let lib = Path::new(cwd)
            .join("src/lib.rs")
            .to_string_lossy()
            .into_owned();
        let new = Path::new(cwd)
            .join("docs/new.txt")
            .to_string_lossy()
            .into_owned();
        for path in [&lib, &new] {
            let quoted = serde_json::to_string(path).expect("serialize");
            assert!(said.contains(&format!("\"path\":{quoted}")), "{said}");
        }
        // The added file has no old text: the editor's schema reads a missing or null one the same.
        assert!(said.contains("\"newText\":\"hi\\n\""), "{said}");
        assert!(said.contains("\"oldText\":\"old\\n\""), "{said}");
        // A path that leaves the repository is skipped, whatever its form.
        assert!(!said.contains("escape.txt"), "{said}");
        assert!(!said.contains("passwd"), "{said}");
        // The link to the pull request is the message.
        assert!(
            said.contains("Pull request #3: http://plaza.gateway/git/agentplaza/app/pulls/3"),
            "{said}"
        );
    }

    #[test]
    fn a_repo_path_is_relative_and_plain() {
        assert!(repo_relative("src/lib.rs"));
        assert!(!repo_relative(""));
        assert!(!repo_relative("../x"));
        assert!(!repo_relative("a/../../x"));
        assert!(!repo_relative("/etc/passwd"));
        assert!(!repo_relative("a\\b"));
    }

    /// A pull request on repository `app`: a changed file, an added file, a deleted file, and a file that
    /// leaves the project.
    fn pull_with_changes() -> RepoPull {
        let text = |s: &str| Some(s.to_string());
        RepoPull {
            url: "http://plaza.gateway/git/agentplaza/app/pulls/3".to_string(),
            number: 3,
            files: vec![
                RepoFile {
                    path: "src/lib.rs".to_string(),
                    old_text: text("old\n"),
                    new_text: text("new\n"),
                },
                RepoFile {
                    path: "docs/new.txt".to_string(),
                    old_text: None,
                    new_text: text("hi\n"),
                },
                RepoFile {
                    path: "old.txt".to_string(),
                    old_text: text("bye\n"),
                    new_text: None,
                },
                RepoFile {
                    path: "../escape.txt".to_string(),
                    old_text: None,
                    new_text: text("out\n"),
                },
            ],
        }
    }

    /// The editor's project root: absolute on every platform.
    fn project_root() -> &'static str {
        if cfg!(windows) {
            r"C:\work\app"
        } else {
            "/work/app"
        }
    }

    /// Runs one prompt of a session linked to repository `app`, in the project root; the work ends done.
    /// Returns the session's id.
    fn linked_turn(h: &Harness) -> String {
        let mut params = repo_meta("app", "editor/main", "", false);
        params["cwd"] = json!(project_root());
        let created = h
            .client
            .peer()
            .call("session/new", params, WAIT)
            .expect("session/new");
        let id = created["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string();
        h.frames.send(work_end()).expect("the bus is open");
        call_with(
            h,
            "session/prompt",
            &id,
            json!({"prompt": [{"type": "text", "text": "Add the docs"}]}),
        );
        id
    }

    /// The params of each question the agent asked the editor about applying pull request #3.
    fn apply_asks(h: &Harness) -> Vec<Value> {
        h.client
            .agent_requests()
            .into_iter()
            .filter(|(method, params)| {
                method == CLIENT_METHOD_NAMES.session_request_permission
                    && params["toolCall"]["toolCallId"] == "apply-pr-3"
            })
            .map(|(_, params)| params)
            .collect()
    }

    /// The params of each `fs/write_text_file` the agent made of the editor.
    fn writes_of(h: &Harness) -> Vec<Value> {
        h.client
            .agent_requests()
            .into_iter()
            .filter(|(method, _)| method == CLIENT_METHOD_NAMES.fs_write_text_file)
            .map(|(_, params)| params)
            .collect()
    }

    #[test]
    fn write_back_is_off_until_the_owner_turns_it_on() {
        let h = harness(None);
        *lock(&h.plaza.hub.pull) = Some(pull_with_changes());
        h.client.initialize().expect("initialize");
        linked_turn(&h);
        // The pull request is still shown; nothing asks the editor to apply it.
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(said.contains("\"toolCallId\":\"pr-3\""), "{said}");
        assert!(apply_asks(&h).is_empty(), "{:?}", h.client.agent_requests());
        assert!(writes_of(&h).is_empty());
    }

    #[test]
    fn write_back_needs_an_editor_that_writes_files() {
        let h = harness(None);
        *lock(&h.plaza.hub.pull) = Some(pull_with_changes());
        *lock(&h.plaza.hub.write_back) = true;
        // An editor whose initialize does not advertise fs.writeTextFile.
        h.client
            .peer()
            .call(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {"fs": {"readTextFile": true}},
                    "clientInfo": {"name": "zed", "version": "0.200"},
                }),
                WAIT,
            )
            .expect("initialize");
        linked_turn(&h);
        assert!(apply_asks(&h).is_empty(), "{:?}", h.client.agent_requests());
        assert!(writes_of(&h).is_empty());
    }

    #[test]
    fn write_back_is_for_the_owner_or_a_govern_token() {
        let h = harness_with(None, operate(), ClientConfig::default());
        *lock(&h.plaza.hub.pull) = Some(pull_with_changes());
        *lock(&h.plaza.hub.write_back) = true;
        h.client.initialize().expect("initialize");
        linked_turn(&h);
        assert!(apply_asks(&h).is_empty(), "{:?}", h.client.agent_requests());
        assert!(writes_of(&h).is_empty());
    }

    #[test]
    fn write_back_applies_the_files_when_the_owner_says_yes() {
        let h = harness(None);
        *lock(&h.plaza.hub.pull) = Some(pull_with_changes());
        *lock(&h.plaza.hub.write_back) = true;
        h.client.initialize().expect("initialize");
        let id = linked_turn(&h);
        // The owner is asked once, about the two files that have new text.
        let asks = apply_asks(&h);
        assert_eq!(asks.len(), 1, "{:?}", h.client.agent_requests());
        assert_eq!(asks[0]["sessionId"], json!(id));
        let tool_call = &asks[0]["toolCall"];
        assert_eq!(
            tool_call["title"],
            "Apply pull request #3 to your project (2 files)"
        );
        assert_eq!(tool_call["kind"], "edit");
        assert_eq!(asks[0]["options"][0]["optionId"], "apply");
        assert_eq!(asks[0]["options"][1]["optionId"], "skip");
        // Each changed file is written at its absolute path with its new text; the deleted one is not.
        let root = Path::new(project_root());
        let lib = root.join("src/lib.rs").to_string_lossy().into_owned();
        let new = root.join("docs/new.txt").to_string_lossy().into_owned();
        let writes = writes_of(&h);
        assert_eq!(writes.len(), 2, "{writes:?}");
        assert_eq!(writes[0]["sessionId"], json!(id));
        assert_eq!(writes[0]["path"], json!(lib));
        assert_eq!(writes[0]["content"], "new\n");
        assert_eq!(writes[1]["path"], json!(new));
        assert_eq!(writes[1]["content"], "hi\n");
        assert_eq!(h.client.file(&lib).as_deref(), Some("new\n"));
        assert_eq!(h.client.file(&new).as_deref(), Some("hi\n"));
        // The update says the files are applied, and names the deleted one for the owner.
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(said.contains("\"toolCallId\":\"apply-pr-3\""), "{said}");
        assert!(
            said.contains("Applied 2 files; 1 deleted files were left for you to remove:"),
            "{said}"
        );
        assert!(said.contains("\"status\":\"completed\""), "{said}");
        assert!(!said.contains("escape.txt"), "{said}");
    }

    #[test]
    fn write_back_skip_writes_nothing() {
        let config = ClientConfig {
            permission: PermissionAnswer::Kind(PermissionOptionKind::RejectOnce),
            ..ClientConfig::default()
        };
        let h = harness_with(None, Caller::Owner, config);
        *lock(&h.plaza.hub.pull) = Some(pull_with_changes());
        *lock(&h.plaza.hub.write_back) = true;
        h.client.initialize().expect("initialize");
        linked_turn(&h);
        // The owner is asked and keeps the pull request on its branch: no write, no update for it.
        assert_eq!(apply_asks(&h).len(), 1, "{:?}", h.client.agent_requests());
        assert!(writes_of(&h).is_empty());
        let said = serde_json::to_string(&h.client.updates()).expect("serialize");
        assert!(!said.contains("apply-pr-3"), "{said}");
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
                queued: std::time::Instant::now(),
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
                queued: std::time::Instant::now(),
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
