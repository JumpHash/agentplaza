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
    /// v3: the result of a tool the host allowed and the guest ran.
    ToolReport {
        work: u64,
        call: String,
        ok: bool,
        output: String,
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

/// One file of a [`Program`], its bytes base64-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramFile {
    pub path: String,
    pub content_b64: String,
    #[serde(default)]
    pub executable: bool,
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
    },
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    /// A plaza task assigned to this agent.
    Task,
    /// A message from the operator (a chat turn).
    Chat,
    /// A message from another agent.
    Message,
    /// A council motion this agent's seat votes on.
    Motion,
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
}

/// What the host tells a guest in its welcome about the world outside the VM (see
/// [`HostMessage::Welcome`]'s `host`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    /// The host's wall clock, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    /// 32 bytes from the host's CSPRNG, hex-encoded, for the guest to mix into its own pool.
    pub entropy: String,
}

impl HostFacts {
    /// The facts as of now: the host clock and 32 bytes of `/dev/urandom` (none when it cannot be
    /// read, which leaves the guest's pool as it is).
    pub fn now() -> Self {
        use std::io::Read;
        let time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        let mut bytes = [0u8; 32];
        let entropy =
            match std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)) {
                Ok(()) => bytes.iter().map(|b| format!("{b:02x}")).collect(),
                Err(_) => String::new(),
            };
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
        };
        assert_eq!(
            HostMessage::from_line(&decision.to_line().unwrap()).unwrap(),
            decision
        );
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
