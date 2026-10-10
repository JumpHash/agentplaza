//! The frozen guest↔host wire protocol, guest side.
//!
//! One message is one line of newline-delimited UTF-8 JSON (see
//! [`crate::line`]). The two directions are separate types: the guest only
//! ever *sends* [`GuestMessage`] and only ever *receives* [`HostMessage`], so
//! a message of the wrong direction is a deserialization error rather than a
//! silently accepted one.
//!
//! The v1 shapes are frozen by the slice contract and stay byte-for-byte what
//! the shipped harness writes:
//!
//! ```text
//! guest → host  {"type":"hello","vm":<i>,"guest_cid":<cid>}
//! host  → guest {"type":"welcome","echo":"plaza-orchestrator"}
//! guest → host  {"type":"tool_call","call":<n>,"name":"echo","args":{"text":"<s>"}}
//! host  → guest {"type":"tool_result","call":<n>,"ok":true,"output":"<s>"}
//! guest → host  {"type":"bye","calls":<n>}
//! ```
//!
//! Protocol v2 (the agent fleet) adds fields to `hello`/`welcome` and the
//! keepalive exchange; it is additive, so a v1 session's lines do not change:
//!
//! ```text
//! guest → host  {"type":"hello","vm":<i>,"guest_cid":<cid>,"role":"<role>","keepalive":true}
//! host  → guest {"type":"welcome","echo":"plaza-orchestrator","role":"<role>","keepalive":true}
//! guest → host  {"type":"heartbeat","seq":<n>}
//! host  → guest {"type":"tool_call","call":<n>,"name":"<tool>","args":{...}}
//! guest → host  {"type":"tool_result","call":<n>,"ok":<bool>,"output":"<s>"}
//! host  → guest {"type":"stop","reason":"<s>"}
//! ```
//!
//! Field order is the order the fields are declared in, so the lines this side
//! writes are byte-for-byte the ones above. `role` and `keepalive` are left out
//! of a `hello` that has no role and is not a keepalive session, which is
//! exactly what keeps the v1 line identical to the frozen one.

use serde::{Deserialize, Serialize};

/// The host's handshake acknowledgement: `welcome.echo` must be this string.
pub const WELCOME_ECHO: &str = "plaza-orchestrator";

/// A line the guest sends to the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestMessage {
    /// `hello` — the first line of every session: who we are and which VM we
    /// are. `role` and `keepalive` are v2's; both are absent in a v1 session.
    Hello {
        /// The VM's index within the spawn (`0` for the first VM).
        vm: u32,
        /// This VM's vsock CID.
        guest_cid: u32,
        /// The role this agent was started as, when its command line names one.
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        /// Whether this guest asks for the long-lived session.
        #[serde(skip_serializing_if = "is_false")]
        keepalive: bool,
    },
    /// `tool_call` — a request for the orchestrator to run one tool (the v1
    /// direction: the guest asks).
    ToolCall {
        /// The call's sequence number within the session, starting at 1.
        call: u64,
        /// The tool's name (`echo`, `uptime`).
        name: String,
        /// The tool's arguments, as the orchestrator's schema expects them.
        args: serde_json::Value,
    },
    /// `tool_result` — the answer to one of the hub's `tool_call`s (the v2
    /// direction: the hub asks, the agent serves).
    ToolResult {
        /// The `call` number this answers.
        call: u64,
        /// Whether the tool succeeded. A failed tool is data, not a protocol
        /// error.
        ok: bool,
        /// The tool's output, or why it could not produce one.
        output: String,
    },
    /// `heartbeat` — a keepalive session's liveness line.
    Heartbeat {
        /// The agent's heartbeat sequence number, starting at 1.
        seq: u64,
    },
    /// `bye` — the session is over; the guest closes the connection after
    /// this line.
    Bye {
        /// How many tool calls this session made (v1) or served (v2).
        calls: u64,
    },
}

/// A line the orchestrator sends to the guest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    /// `welcome` — the orchestrator accepted the handshake. A v1 hub echoes
    /// only `echo`; a v2 hub also says which role it accepted and whether the
    /// session is a keepalive one.
    Welcome {
        /// Must be [`WELCOME_ECHO`].
        echo: String,
        /// The role the hub accepted: the agent's identity for its lifetime,
        /// when the hub names one.
        #[serde(default)]
        role: Option<String>,
        /// Whether the hub serves this session as a keepalive one.
        #[serde(default)]
        keepalive: Option<bool>,
    },
    /// `tool_result` — the outcome of the guest's own `tool_call`.
    ToolResult {
        /// The `call` number this answers.
        call: u64,
        /// Whether the tool succeeded. A failed tool is data, not a protocol
        /// error.
        ok: bool,
        /// The tool's output (the echoed text, the host's uptime, or an error
        /// message when `ok` is false).
        output: String,
    },
    /// `tool_call` — the hub asks the agent to run one tool (v2).
    ToolCall {
        /// The call's sequence number within the session, starting at 1.
        call: u64,
        /// The tool's name.
        name: String,
        /// The tool's arguments.
        args: serde_json::Value,
    },
    /// `stop` — the hub ends a keepalive session; the agent answers `bye`.
    Stop {
        /// Why the hub stopped the agent, when it says.
        #[serde(default)]
        reason: Option<String>,
    },
}

/// A `skip_serializing_if` for "not a keepalive session": a v1 `hello` must
/// not grow a `"keepalive":false` field.
fn is_false(value: &bool) -> bool {
    !*value
}

impl GuestMessage {
    /// Encodes this message as one protocol line, without its newline.
    ///
    /// Fails only if `args` cannot be represented in JSON — which the type
    /// makes impossible today, but the caller decides what to do rather than
    /// this function panicking.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl HostMessage {
    /// Parses one protocol line.
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The v1 hello, with the v2 fields absent: byte-for-byte the frozen line.
    fn v1_hello(vm: u32, guest_cid: u32) -> GuestMessage {
        GuestMessage::Hello {
            vm,
            guest_cid,
            role: None,
            keepalive: false,
        }
    }

    #[test]
    fn hello_is_byte_for_byte_the_frozen_line() {
        let line = v1_hello(0, 3).to_line().expect("hello encodes");
        assert_eq!(line, r#"{"type":"hello","vm":0,"guest_cid":3}"#);
    }

    #[test]
    fn a_keepalive_hello_carries_the_role_and_the_flag() {
        let line = GuestMessage::Hello {
            vm: 4,
            guest_cid: 104,
            role: Some("builder".to_string()),
            keepalive: true,
        }
        .to_line()
        .expect("hello encodes");
        assert_eq!(
            line,
            r#"{"type":"hello","vm":4,"guest_cid":104,"role":"builder","keepalive":true}"#
        );
    }

    #[test]
    fn a_keepalive_hello_without_a_role_still_says_keepalive() {
        let line = GuestMessage::Hello {
            vm: 0,
            guest_cid: 7,
            role: None,
            keepalive: true,
        }
        .to_line()
        .expect("hello encodes");
        assert_eq!(
            line,
            r#"{"type":"hello","vm":0,"guest_cid":7,"keepalive":true}"#
        );
    }

    #[test]
    fn tool_call_is_byte_for_byte_the_frozen_line() {
        let line = GuestMessage::ToolCall {
            call: 1,
            name: "echo".to_string(),
            args: json!({"text": "plaza-guest-3-hello"}),
        }
        .to_line()
        .expect("tool_call encodes");
        assert_eq!(
            line,
            r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza-guest-3-hello"}}"#
        );
    }

    #[test]
    fn bye_is_byte_for_byte_the_frozen_line() {
        let line = GuestMessage::Bye { calls: 2 }
            .to_line()
            .expect("bye encodes");
        assert_eq!(line, r#"{"type":"bye","calls":2}"#);
    }

    #[test]
    fn the_v2_lines_this_side_writes_are_the_contracts() {
        assert_eq!(
            GuestMessage::Heartbeat { seq: 7 }
                .to_line()
                .expect("heartbeat encodes"),
            r#"{"type":"heartbeat","seq":7}"#
        );
        assert_eq!(
            GuestMessage::ToolResult {
                call: 1,
                ok: true,
                output: "plaza-guest-3-hello".to_string(),
            }
            .to_line()
            .expect("tool_result encodes"),
            r#"{"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}"#
        );
        assert_eq!(
            GuestMessage::ToolResult {
                call: 2,
                ok: false,
                output: "no such tool: exec".to_string(),
            }
            .to_line()
            .expect("tool_result encodes"),
            r#"{"type":"tool_result","call":2,"ok":false,"output":"no such tool: exec"}"#
        );
    }

    #[test]
    fn welcome_and_tool_result_parse() {
        assert_eq!(
            HostMessage::from_line(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .expect("welcome parses"),
            HostMessage::Welcome {
                echo: WELCOME_ECHO.to_string(),
                role: None,
                keepalive: None,
            }
        );
        assert_eq!(
            HostMessage::from_line(
                r#"{"type":"tool_result","call":2,"ok":true,"output":"12.34 56.78"}"#
            )
            .expect("tool_result parses"),
            HostMessage::ToolResult {
                call: 2,
                ok: true,
                output: "12.34 56.78".to_string()
            }
        );
    }

    #[test]
    fn a_v2_welcome_carries_the_accepted_role() {
        assert_eq!(
            HostMessage::from_line(
                r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#
            )
            .expect("welcome parses"),
            HostMessage::Welcome {
                echo: WELCOME_ECHO.to_string(),
                role: Some("builder".to_string()),
                keepalive: Some(true),
            }
        );
    }

    #[test]
    fn the_hubs_v2_lines_parse() {
        assert_eq!(
            HostMessage::from_line(r#"{"type":"tool_call","call":3,"name":"uptime","args":{}}"#)
                .expect("tool_call parses"),
            HostMessage::ToolCall {
                call: 3,
                name: "uptime".to_string(),
                args: json!({}),
            }
        );
        assert_eq!(
            HostMessage::from_line(r#"{"type":"stop","reason":"fleet shutdown"}"#)
                .expect("stop parses"),
            HostMessage::Stop {
                reason: Some("fleet shutdown".to_string())
            }
        );
        // A stop without a reason is a stop, not a parse failure.
        assert_eq!(
            HostMessage::from_line(r#"{"type":"stop"}"#).expect("stop parses"),
            HostMessage::Stop { reason: None }
        );
    }

    #[test]
    fn a_host_line_of_the_other_direction_does_not_parse() {
        // `heartbeat`, `bye` and `welcome` belong to the other side of the
        // protocol: receiving one is a parse error, not a silent no-op.
        for line in [
            r#"{"type":"heartbeat","seq":1}"#,
            r#"{"type":"bye","calls":2}"#,
            r#"{"type":"hello","vm":0,"guest_cid":3}"#,
        ] {
            assert!(HostMessage::from_line(line).is_err(), "{line}");
        }
    }
}
