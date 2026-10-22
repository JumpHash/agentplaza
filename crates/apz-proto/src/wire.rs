//! The guest↔hub line protocol: one JSON object per `\n`-terminated line over
//! the VM's vsock connection.
//!
//! ```text
//! v1  demo session (one-shot):
//!   guest hello{vm,guest_cid} → host welcome → guest tool_call* ⇄ host tool_result → guest bye
//! v2  keepalive session (long-lived agent):
//!   guest hello{role,keepalive:true} → host welcome{role,keepalive}
//!   guest heartbeat*; host tool_call ⇄ guest tool_result (operator drive); host stop
//! v3  agent loop (inside a v2 session; welcome carries `profile`):
//!   host work{work,kind,thread,prompt}
//!     guest model_call{req,work,request}       → host model_result{req,ok,response|error}
//!     guest tool_request{work,call,name,input} → host tool_decision{call,decision,output}
//!         decision=allow → guest runs the tool → guest tool_report{work,call,ok,output}
//!         decision=done  → the host ran it (plaza tools); `output` is the result
//!         decision=deny  → `output` is the reason; the model sees an error result
//!   guest work_done{work,status,summary}
//!   host cancel{work} aborts the work item; host profile{profile} replaces the profile
//! v4  ACP engines (inside a v3 session; hello carries `caps`, the harness's own features):
//!   guest hello{…,caps:["acp"]} → a harness that can host an ACP engine says so
//!   guest tool_request{work,call:"<call>.e<n>",name:"engine.<kind>"} from the engine's session
//!     → host tool_decision{decision:pending{approval}} when the owner must decide; the request
//!       stays open, and when the owner decides the host pushes
//!       host permission_decision{work,call,allow,reason?} (a push, not an answer to a line)
//!     → host tool_decision{decision:allow|deny} otherwise, as in v3
//!   guest engine_update{work,call,engine,session,seq,update} streams one ACP session update
//!     (the payload as the engine sent it); the hub records some of them and publishes all
//! v5  one decision per model turn, and the policy the VM decides by (inside a v3 session):
//!   host policy{policy} after the profile, and again whenever what it allows changes: the tools the guest runs
//!     without asking, signed by the hub; an empty allow list holds them all (STOP ALL)
//!   guest tool_batch{work,calls:[{call,name,input}]} → host tool_batch_decision{work,decisions:[…]}, one answer for
//!     the turn's calls the policy does not allow; the guest runs the allowed ones itself and reports each one with
//!     tool_report{…,policy,name,input} (the hub records the call with the policy it was decided under)
//! ```
//!
//! The guest's identity is never taken from a line: the hub binds every
//! session to the CID its socket path names.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::chat::{ChatRequest, ChatResponse, ToolSpec};

/// `welcome.echo`.
pub const WELCOME_ECHO: &str = "plaza-orchestrator";

/// The longest line either side accepts (model requests carry history).
pub const MAX_LINE_BYTES: usize = 8 << 20;

/// The start of a model result's error that means "held, ask again later": the work's budget or a limit is reached,
/// or the plaza is held (STOP ALL). The harness's turn waits and asks again instead of failing (Engine 0.5).
pub const MODEL_PAUSED: &str = "paused: ";

/// The longest tool output the guest reports; longer output is cut head+tail.
pub const MAX_TOOL_OUTPUT: usize = 64 << 10;

/// A line the guest sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestMessage {
    Hello {
        vm: u32,
        guest_cid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keepalive: Option<bool>,
        /// v4: the features this harness has (see [`CAP_ACP`]). An older harness omits it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        caps: Vec<String>,
    },
    /// v1: the guest asks the host to run a diagnostic tool.
    ToolCall {
        call: u64,
        name: String,
        args: Value,
    },
    /// v2: the answer to a host [`HostMessage::ToolCall`].
    ToolResult {
        call: u64,
        ok: bool,
        output: String,
    },
    Heartbeat {
        seq: u64,
        /// v3: whether a work item is in progress.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        busy: Option<bool>,
    },
    Bye {
        calls: u64,
    },
    /// v3: one model call for work item `work`.
    ModelCall {
        req: u64,
        work: u64,
        request: ChatRequest,
    },
    /// v3: the model asked for tool `name`; `call` is its tool_use id.
    ToolRequest {
        work: u64,
        call: String,
        name: String,
        input: Value,
    },
    /// v3: the result of a tool the host allowed and the guest ran. v5: a call the guest decided itself under a
    /// policy snapshot (`policy`, its id) carries the call's `name` and `input` too, so the hub records the call.
    ToolReport {
        work: u64,
        call: String,
        ok: bool,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
    /// v5: the calls of one model turn the policy does not allow, decided by the hub in one answer
    /// ([`HostMessage::ToolBatchDecision`]), in the calls' order.
    ToolBatch {
        work: u64,
        calls: Vec<BatchCall>,
    },
    /// v4: one session update from an ACP engine for work item `work`, under call `call` (the
    /// `code_engine` call's id, or the nested id of a request the engine made). `update` is the
    /// ACP session update as the engine sent it; `seq` counts the updates of `session`.
    EngineUpdate {
        work: u64,
        call: String,
        engine: String,
        session: String,
        seq: u64,
        update: Value,
    },
    /// v3: the work item is over.
    WorkDone {
        work: u64,
        status: WorkStatus,
        summary: String,
    },
    /// v3: the harness compressed work item `work`'s conversation: its size in bytes before
    /// and after, how many messages were folded into the summary, and why ("auto" or "asked").
    Compressed {
        work: u64,
        before: u64,
        after: u64,
        folded: u32,
        reason: String,
    },
    /// The harness's stats, about once a minute while it runs: the size and use of `/workspace` in
    /// MiB, and the busiest processes since the last report as (name, CPU %, RSS MiB).
    Stats {
        workspace_used_mib: u64,
        workspace_size_mib: u64,
        top: Vec<(String, f64, f64)>,
    },
}

/// An agent-made tool from the plaza's library, as the guest runs it: its files are
/// unpacked under `/workspace/.plaza/tools/<name>@<version>/`, its `requires` apk
/// packages installed, and `command` run there with the call's input JSON on stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub name: String,
    pub version: u32,
    /// SHA-256 over the files, so an unpacked copy can be reused.
    pub hash: String,
    pub files: Vec<ProgramFile>,
    /// argv, run in the unpacked directory (e.g. `["python3", "main.py"]`).
    pub command: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    pub timeout_s: u64,
}

/// How the VM starts one engine instance (`code_engine`) beyond the harness catalogue's defaults: the
/// owner's pinned install, launch arguments, plain environment, home folder and default model route
/// (`apid`'s `engine_instances.rs`). Sent with a `code_engine` call's [`HostMessage::ToolDecision`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSpec {
    /// A catalogue engine id, `acp:<registry id>` or `acp-local`.
    pub engine: String,
    /// None: the catalogue's own pinned install.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<EngineInstall>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// The engine's home folder name under the VM's engines home; None: the instance id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// The model the engine runs when a call names none, `<provider>/<model>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

/// An exact, pinned install of an engine: the VM refuses anything that does not match its pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EngineInstall {
    /// An npm package at an exact version, checked against the registry's `dist.integrity`.
    Npm {
        package: String,
        version: String,
        integrity: String,
    },
    /// A Python package's wheel at an exact version, checked against its sha256.
    Pip {
        package: String,
        version: String,
        file: String,
        sha256: String,
    },
    /// A release archive per VM architecture (`linux-x86_64`, `linux-aarch64`), each checked against its
    /// sha256; `cmd` is the binary inside it.
    Archive {
        version: String,
        assets: std::collections::BTreeMap<String, EngineAsset>,
    },
    /// A command already present in the agent's VM image or workspace.
    Command { command: String },
}

/// One architecture's archive of an [`EngineInstall::Archive`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineAsset {
    pub url: String,
    pub sha256: String,
    pub cmd: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// One file of a [`Program`], its bytes base64-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramFile {
    pub path: String,
    pub content_b64: String,
    #[serde(default)]
    pub executable: bool,
}

/// A task's lane in Weft: the change the worker writes, and the repo it is in (checked out at `<workspace>/<repo>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "store", derive(apz_store::Table))]
pub struct WorkLane {
    pub change: String,
    pub repo: String,
}

/// A line the host sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    Welcome {
        echo: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keepalive: Option<bool>,
        /// v3: present when the plaza runs a model for this agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<AgentProfile>,
        /// v4: the host's clock and fresh entropy. A guest restored from a snapshot (woken, forked,
        /// started from a template) resumes with the clock and the random state it had when the
        /// snapshot was taken; the harness sets its clock and mixes the entropy in before it does
        /// anything else, so no two restored guests share a random stream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<HostFacts>,
    },
    /// v1: the answer to a guest [`GuestMessage::ToolCall`].
    ToolResult { call: u64, ok: bool, output: String },
    /// v2: the operator drives one tool in the guest.
    ToolCall {
        call: u64,
        name: String,
        args: Value,
    },
    Stop {
        /// Always written; a `stop` without one still parses.
        #[serde(default)]
        reason: String,
    },
    /// v3: a unit of work for the agent loop.
    Work {
        work: u64,
        kind: WorkKind,
        thread: String,
        prompt: String,
        /// The task's lane in Weft, when it has one: the harness checks it out into the workspace and snapshots
        /// it each turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane: Option<WorkLane>,
    },
    /// v3: the answer to a [`GuestMessage::ModelCall`].
    ModelResult {
        req: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<ChatResponse>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// v3: the answer to a [`GuestMessage::ToolRequest`].
    ToolDecision {
        call: String,
        decision: Decision,
        #[serde(default)]
        output: String,
        #[serde(default)]
        is_error: bool,
        /// A plaza library tool to run for this call (an agent-made tool): the guest
        /// unpacks it in its own VM and runs it with the call's input. `None` for
        /// built-in tools.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<Program>,
        /// For an allowed `code_engine` call on an engine instance the owner configured: how to start it.
        /// `None`: the harness catalogue's defaults for the named engine.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        engine: Option<EngineSpec>,
        /// v4: with `decision: pending`, the approval the owner decides; the answer follows as a
        /// [`HostMessage::PermissionDecision`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval: Option<String>,
    },
    /// v4: the owner decided a call held for approval (`decision: pending`). Pushed by the hub,
    /// not sent in answer to a line.
    PermissionDecision {
        work: u64,
        call: String,
        allow: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// v5: the answer to a [`GuestMessage::ToolBatch`]: one decision per call, in the calls' order, each as the
    /// call's [`HostMessage::ToolDecision`] would have been.
    ToolBatchDecision {
        work: u64,
        decisions: Vec<BatchDecision>,
    },
    /// v5: the policy the guest decides its allowed calls by (see [`PolicySnapshot`]). Pushed after the profile at
    /// hello and whenever what it allows changes; not an answer to a line.
    Policy { policy: PolicySnapshot },
    /// v3: abort work item `work` after the current step.
    Cancel { work: u64, reason: String },
    /// v3: replace the agent's profile (model, tools, limits).
    Profile { profile: AgentProfile },
    /// v3: steer the running work item `work` (the orchestrator, the mayor or the owner).
    Control { work: u64, control: Control },
}

/// What a [`HostMessage::Control`] asks of a running work item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Control {
    /// Fold the older part of the conversation into a summary before the next model call.
    Compress,
    /// Add a note to the conversation before the next model call: `from` says who (a name).
    Steer { from: String, text: String },
    /// Allow `turns` more model calls for this work item.
    ExtendTurns { turns: u32 },
}

/// What the agent is, as `apid` configured it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub role: String,
    /// The display name of the model (`provider/model`); informational, the
    /// host picks the model for every call.
    pub model: String,
    pub system: String,
    /// Every tool offered to the model (guest-run and host-run alike).
    pub tools: Vec<ToolSpec>,
    /// Model calls per work item before the agent stops as over budget.
    pub max_turns: u32,
    /// Compress the conversation once it is larger than this many bytes (serialized);
    /// 0 never compresses on its own.
    #[serde(default)]
    pub compress_at: u64,
    /// The most recent turns (an assistant message and the results after it) kept verbatim
    /// when compressing; older ones are folded into a summary.
    #[serde(default)]
    pub keep_recent: u32,
    /// v4: which ACP version the engines are offered: empty (ACP v1) or `acp2` (the v2 draft). Every
    /// engine streams as an ACP session with every action checked by the hub.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine_transport: String,
    /// v4: the hub's egress CA certificate (PEM), present when the owner gave this agent's role an
    /// egress credential rule. The hub's egress proxy terminates TLS only for hosts with such a rule
    /// (to add the credential the guest never sees) and signs their certificates with this CA, so
    /// the harness adds it to the guest's trust store. Every other host stays an opaque tunnel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress_ca: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "store", derive(apz_store::Column))]
pub enum WorkKind {
    /// A plaza task assigned to this agent.
    Task,
    /// A message from the operator (a chat turn).
    Chat,
    /// A message from another agent.
    Message,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Done,
    Failed,
    /// Cancelled by the host or stopped by a stop condition.
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    Done,
    /// v4: held until the owner decides; the decision's `approval` names it.
    Pending,
}

/// One call of a [`GuestMessage::ToolBatch`]: the model's tool_use `call`, its tool `name` and its `input`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchCall {
    pub call: String,
    pub name: String,
    pub input: Value,
}

/// The hub's decision for one call of a [`HostMessage::ToolBatchDecision`]: the fields of a
/// [`HostMessage::ToolDecision`], for the call `call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchDecision {
    pub call: String,
    pub decision: Decision,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<Program>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

/// The tools the guest may run without asking the hub, signed by the hub (see [`HostMessage::Policy`]). `id` is the
/// SHA-256 hex of the canonical body (the agent's CID, `issued_ms` and the sorted `allow` list); `signature` is the
/// hub's Ed25519 signature of `id`, base64url without padding. An empty `allow` holds every call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySnapshot {
    pub id: String,
    pub issued_ms: u64,
    pub allow: Vec<String>,
    pub signature: String,
}

/// The `caps` entry a harness with ACP engine support sends in its hello. The hub answers an ACP
/// engine's held call with `pending` only to a harness that sent it.
pub const CAP_ACP: &str = "acp";
/// What the host tells a guest in its welcome about the world outside the VM (see
/// [`HostMessage::Welcome`]'s `host`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    /// The host's wall clock, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    /// 32 bytes from the host's CSPRNG, hex-encoded, for the guest to mix into its own pool.
    pub entropy: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl HostFacts {
    /// The facts as of now: the host clock and 32 bytes of the OS CSPRNG (none when it fails,
    /// which leaves the guest's pool as it is). The CSPRNG, not `/dev/urandom`: Windows hosts
    /// have no such file.
    pub fn now() -> Self {
        let time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        let entropy = apz_os::random::hex(32).unwrap_or_default();
        Self { time_ms, entropy }
    }
}

impl GuestMessage {
    pub fn from_line(line: &str) -> serde_json::Result<Self> {
        serde_json::from_str(line)
    }

    pub fn to_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

impl HostMessage {
    pub fn from_line(line: &str) -> serde_json::Result<Self> {
        serde_json::from_str(line)
    }

    pub fn to_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    pub fn welcome(
        role: Option<String>,
        keepalive: Option<bool>,
        profile: Option<AgentProfile>,
    ) -> Self {
        HostMessage::Welcome {
            echo: WELCOME_ECHO.to_string(),
            role,
            keepalive,
            profile,
            host: None,
        }
    }

    /// The same welcome carrying `facts` (see [`HostFacts`]).
    pub fn with_host(mut self, facts: HostFacts) -> Self {
        if let HostMessage::Welcome { host, .. } = &mut self {
            *host = Some(facts);
        }
        self
    }
}

/// Cuts `text` to at most `max` bytes, keeping the head and the tail.
pub fn cap_output(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let keep = max / 2;
    let mut head = keep;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - keep;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n[... {} bytes cut ...]\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn v1_and_v2_lines_are_byte_stable() {
        let hello = GuestMessage::Hello {
            vm: 0,
            guest_cid: 3,
            role: None,
            keepalive: None,
            caps: Vec::new(),
        };
        assert_eq!(
            hello.to_line().unwrap(),
            r#"{"type":"hello","vm":0,"guest_cid":3}"#
        );
        assert_eq!(
            HostMessage::welcome(None, None, None).to_line().unwrap(),
            r#"{"type":"welcome","echo":"plaza-orchestrator"}"#
        );
        assert_eq!(
            HostMessage::welcome(Some("builder".into()), Some(true), None)
                .to_line()
                .unwrap(),
            r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#
        );
        assert_eq!(
            GuestMessage::Heartbeat { seq: 7, busy: None }
                .to_line()
                .unwrap(),
            r#"{"type":"heartbeat","seq":7}"#
        );
        let keepalive = GuestMessage::Hello {
            vm: 0,
            guest_cid: 7,
            role: None,
            keepalive: Some(true),
            caps: Vec::new(),
        };
        assert_eq!(
            keepalive.to_line().unwrap(),
            r#"{"type":"hello","vm":0,"guest_cid":7,"keepalive":true}"#
        );
        let call = GuestMessage::ToolCall {
            call: 1,
            name: "echo".into(),
            args: json!({"text": "hi"}),
        };
        assert_eq!(
            call.to_line().unwrap(),
            r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"hi"}}"#
        );
        let result = GuestMessage::ToolResult {
            call: 2,
            ok: false,
            output: "no such tool: x".into(),
        };
        assert_eq!(
            result.to_line().unwrap(),
            r#"{"type":"tool_result","call":2,"ok":false,"output":"no such tool: x"}"#
        );
        assert_eq!(
            GuestMessage::Bye { calls: 2 }.to_line().unwrap(),
            r#"{"type":"bye","calls":2}"#
        );
        let host_result = HostMessage::ToolResult {
            call: 2,
            ok: true,
            output: "12.34 56.78".into(),
        };
        assert_eq!(
            host_result.to_line().unwrap(),
            r#"{"type":"tool_result","call":2,"ok":true,"output":"12.34 56.78"}"#
        );
        let host_call = HostMessage::ToolCall {
            call: 3,
            name: "uptime".into(),
            args: json!({}),
        };
        assert_eq!(
            host_call.to_line().unwrap(),
            r#"{"type":"tool_call","call":3,"name":"uptime","args":{}}"#
        );
        let stop = HostMessage::Stop {
            reason: "fleet shutdown".into(),
        };
        assert_eq!(
            stop.to_line().unwrap(),
            r#"{"type":"stop","reason":"fleet shutdown"}"#
        );
        assert_eq!(
            HostMessage::from_line(r#"{"type":"stop"}"#).unwrap(),
            HostMessage::Stop {
                reason: String::new()
            }
        );
    }

    #[test]
    fn wrong_types_and_wrong_directions_do_not_parse() {
        for line in [
            r#"{"type":"hello","vm":0,"guest_cid":3,"role":7}"#,
            r#"{"type":"hello","vm":0,"guest_cid":3,"keepalive":"yes"}"#,
            r#"{"type":"welcome","echo":"plaza-orchestrator"}"#,
        ] {
            assert!(GuestMessage::from_line(line).is_err(), "{line}");
        }
        for line in [
            r#"{"type":"heartbeat","seq":1}"#,
            r#"{"type":"bye","calls":2}"#,
        ] {
            assert!(HostMessage::from_line(line).is_err(), "{line}");
        }
    }

    #[test]
    fn v3_lines_round_trip() {
        let call = GuestMessage::ToolRequest {
            work: 1,
            call: "toolu_1".into(),
            name: "bash".into(),
            input: json!({"command": "ls"}),
        };
        assert_eq!(
            GuestMessage::from_line(&call.to_line().unwrap()).unwrap(),
            call
        );
        let decision = HostMessage::ToolDecision {
            call: "toolu_1".into(),
            decision: Decision::Allow,
            output: String::new(),
            is_error: false,
            program: None,
            engine: None,
            approval: None,
        };
        assert_eq!(
            HostMessage::from_line(&decision.to_line().unwrap()).unwrap(),
            decision
        );
    }

    #[test]
    fn a_stats_report_round_trips_with_its_processes_as_arrays() {
        let stats = GuestMessage::Stats {
            workspace_used_mib: 812,
            workspace_size_mib: 4096,
            top: vec![("node".into(), 37.5, 220.25), ("bash".into(), 0.0, 3.0)],
        };
        let line = stats.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"stats","workspace_used_mib":812,"workspace_size_mib":4096,"top":[["node",37.5,220.25],["bash",0.0,3.0]]}"#
        );
        assert_eq!(GuestMessage::from_line(&line).unwrap(), stats);
    }

    #[test]
    fn v4_lines_round_trip() {
        let engine = GuestMessage::EngineUpdate {
            work: 4,
            call: "toolu_9".into(),
            engine: "claude".into(),
            session: "s-1".into(),
            seq: 12,
            update: json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}}),
        };
        let line = engine.to_line().unwrap();
        assert!(
            line.starts_with(r#"{"type":"engine_update","work":4,"#),
            "{line}"
        );
        assert_eq!(GuestMessage::from_line(&line).unwrap(), engine);

        let decision = HostMessage::PermissionDecision {
            work: 4,
            call: "toolu_9.e1".into(),
            allow: false,
            reason: Some("not today".into()),
        };
        let line = decision.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"permission_decision","work":4,"call":"toolu_9.e1","allow":false,"reason":"not today"}"#
        );
        assert_eq!(HostMessage::from_line(&line).unwrap(), decision);
        let bare = HostMessage::PermissionDecision {
            work: 4,
            call: "toolu_9.e1".into(),
            allow: true,
            reason: None,
        };
        assert_eq!(
            bare.to_line().unwrap(),
            r#"{"type":"permission_decision","work":4,"call":"toolu_9.e1","allow":true}"#
        );

        let pending = HostMessage::ToolDecision {
            call: "toolu_9.e1".into(),
            decision: Decision::Pending,
            output: "waiting for the owner's approval (a7)".into(),
            is_error: false,
            program: None,
            engine: None,
            approval: Some("a7".into()),
        };
        let line = pending.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"tool_decision","call":"toolu_9.e1","decision":"pending","output":"waiting for the owner's approval (a7)","is_error":false,"approval":"a7"}"#
        );
        assert_eq!(HostMessage::from_line(&line).unwrap(), pending);
    }

    #[test]
    fn hello_carries_caps_and_an_old_hello_still_parses() {
        let acp = GuestMessage::Hello {
            vm: 0,
            guest_cid: 3,
            role: Some("builder".into()),
            keepalive: Some(true),
            caps: vec![CAP_ACP.to_string()],
        };
        let line = acp.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder","keepalive":true,"caps":["acp"]}"#
        );
        assert_eq!(GuestMessage::from_line(&line).unwrap(), acp);

        // A harness from before v4 omits caps: it parses with none, and its line is unchanged.
        let old = r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder","keepalive":true}"#;
        match GuestMessage::from_line(old).unwrap() {
            GuestMessage::Hello { caps, role, .. } => {
                assert!(caps.is_empty());
                assert_eq!(role.as_deref(), Some("builder"));
            }
            other => panic!("not a hello: {other:?}"),
        }
    }

    #[test]
    fn an_old_tool_decision_without_approval_parses_as_before() {
        let old = r#"{"type":"tool_decision","call":"toolu_1","decision":"allow"}"#;
        match HostMessage::from_line(old).unwrap() {
            HostMessage::ToolDecision {
                decision,
                approval,
                output,
                ..
            } => {
                assert_eq!(decision, Decision::Allow);
                assert_eq!(approval, None);
                assert!(output.is_empty());
            }
            other => panic!("not a tool decision: {other:?}"),
        }
        // The v3 decision lines are byte-stable: no approval key appears on them.
        let deny = HostMessage::ToolDecision {
            call: "toolu_1".into(),
            decision: Decision::Deny,
            output: "no".into(),
            is_error: true,
            program: None,
            engine: None,
            approval: None,
        };
        assert!(!deny.to_line().unwrap().contains("approval"));
    }

    #[test]
    fn v5_batch_lines_round_trip_and_stay_stable() {
        let batch = GuestMessage::ToolBatch {
            work: 3,
            calls: vec![
                BatchCall {
                    call: "toolu_1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
                BatchCall {
                    call: "toolu_2".into(),
                    name: "glob".into(),
                    input: json!({"pattern": "*.rs"}),
                },
            ],
        };
        let line = batch.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"tool_batch","work":3,"calls":[{"call":"toolu_1","name":"bash","input":{"command":"ls"}},{"call":"toolu_2","name":"glob","input":{"pattern":"*.rs"}}]}"#
        );
        assert_eq!(GuestMessage::from_line(&line).unwrap(), batch);

        let decided = HostMessage::ToolBatchDecision {
            work: 3,
            decisions: vec![
                BatchDecision {
                    call: "toolu_1".into(),
                    decision: Decision::Allow,
                    output: String::new(),
                    is_error: false,
                    program: None,
                    engine: None,
                    approval: None,
                },
                BatchDecision {
                    call: "toolu_2".into(),
                    decision: Decision::Pending,
                    output: "waiting for the owner's approval (a7)".into(),
                    is_error: false,
                    program: None,
                    engine: None,
                    approval: Some("a7".into()),
                },
            ],
        };
        let line = decided.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"tool_batch_decision","work":3,"decisions":[{"call":"toolu_1","decision":"allow","output":"","is_error":false},{"call":"toolu_2","decision":"pending","output":"waiting for the owner's approval (a7)","is_error":false,"approval":"a7"}]}"#
        );
        assert_eq!(HostMessage::from_line(&line).unwrap(), decided);
    }

    #[test]
    fn a_policy_line_round_trips_and_an_empty_allow_list_holds_every_call() {
        let policy = HostMessage::Policy {
            policy: PolicySnapshot {
                id: "ab12".into(),
                issued_ms: 1_700_000_000_000,
                allow: vec!["glob".into(), "read_file".into()],
                signature: "c2ln".into(),
            },
        };
        let line = policy.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"policy","policy":{"id":"ab12","issued_ms":1700000000000,"allow":["glob","read_file"],"signature":"c2ln"}}"#
        );
        assert_eq!(HostMessage::from_line(&line).unwrap(), policy);
        let held = HostMessage::Policy {
            policy: PolicySnapshot {
                id: "cd34".into(),
                issued_ms: 5,
                allow: Vec::new(),
                signature: "c2ln".into(),
            },
        };
        let line = held.to_line().unwrap();
        assert!(line.contains(r#""allow":[]"#), "{line}");
        assert_eq!(HostMessage::from_line(&line).unwrap(), held);
    }

    #[test]
    fn a_tool_report_with_a_policy_carries_the_call_and_an_old_one_is_unchanged() {
        let plain = GuestMessage::ToolReport {
            work: 1,
            call: "toolu_1".into(),
            ok: true,
            output: "x".into(),
            policy: None,
            name: None,
            input: None,
        };
        assert_eq!(
            plain.to_line().unwrap(),
            r#"{"type":"tool_report","work":1,"call":"toolu_1","ok":true,"output":"x"}"#
        );
        let local = GuestMessage::ToolReport {
            work: 1,
            call: "toolu_2".into(),
            ok: true,
            output: "a.rs".into(),
            policy: Some("ab12".into()),
            name: Some("glob".into()),
            input: Some(json!({"pattern": "*.rs"})),
        };
        let line = local.to_line().unwrap();
        assert_eq!(
            line,
            r#"{"type":"tool_report","work":1,"call":"toolu_2","ok":true,"output":"a.rs","policy":"ab12","name":"glob","input":{"pattern":"*.rs"}}"#
        );
        assert_eq!(GuestMessage::from_line(&line).unwrap(), local);
        // A harness from before v5 sends no policy: its report parses with none.
        let old = r#"{"type":"tool_report","work":1,"call":"toolu_1","ok":true,"output":"x"}"#;
        assert_eq!(GuestMessage::from_line(old).unwrap(), plain);
    }

    #[test]
    fn cap_output_keeps_head_and_tail() {
        let text = "a".repeat(100) + &"b".repeat(100);
        let capped = cap_output(&text, 40);
        assert!(capped.starts_with(&"a".repeat(20)));
        assert!(capped.ends_with(&"b".repeat(20)));
        assert_eq!(cap_output("short", 40), "short");
    }
}
