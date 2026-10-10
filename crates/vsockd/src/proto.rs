//! The guest↔host wire protocol, host side.
//!
//! One message is one line of newline-delimited UTF-8 JSON (see
//! [`crate::line`]). The two directions are separate types: the orchestrator
//! only ever *receives* [`GuestMessage`] and only ever *sends*
//! [`HostMessage`], so a message of the wrong direction is a deserialization
//! error rather than a silently accepted one.
//!
//! # v1 — the frozen demo session
//!
//! ```text
//! guest → host  {"type":"hello","vm":<i>,"guest_cid":<cid>}
//! host  → guest {"type":"welcome","echo":"plaza-orchestrator"}
//! guest → host  {"type":"tool_call","call":<n>,"name":"echo","args":{"text":"<s>"}}
//! host  → guest {"type":"tool_result","call":<n>,"ok":true,"output":"<s>"}
//! guest → host  {"type":"bye","calls":<n>}
//! ```
//!
//! # v2 — the keepalive agent (additive, opt-in)
//!
//! A `hello` that carries `"keepalive": true` opens a long-lived session:
//!
//! ```text
//! guest → host  {"type":"hello","vm":<i>,"guest_cid":<cid>,"role":"<role>","keepalive":true}
//! host  → guest {"type":"welcome","echo":"plaza-orchestrator","role":"<role>","keepalive":true}
//! guest → host  {"type":"heartbeat","seq":<n>}
//! host  → guest {"type":"tool_call","call":<n>,"name":"<tool>","args":{...}}
//! guest → host  {"type":"tool_result","call":<n>,"ok":<bool>,"output":"<s>"}
//! host  → guest {"type":"stop","reason":"<s>"}
//! guest → host  {"type":"bye","calls":<n>}
//! ```
//!
//! The direction of the tool exchange is the keepalive session's: the hub
//! dispatches [`HostMessage::ToolCall`] at will and the agent answers with
//! [`GuestMessage::ToolResult`] (the v1 tools, `echo` and `uptime`, unchanged;
//! more tools are later slices).
//!
//! Two byte-shape rules keep v1 intact:
//!
//! * [`GuestMessage::Hello`]'s `role`/`keepalive` are optional, so v1's
//!   `{"type":"hello","vm":0,"guest_cid":3}` still parses (and a wrong-typed
//!   `role`/`keepalive` is a parse error, so the session is dropped rather
//!   than guessed at).
//! * [`HostMessage::Welcome`] skips both fields when they are `None`, so a v1
//!   hello gets v1's exact `{"type":"welcome","echo":"plaza-orchestrator"}`
//!   and only a hello that declared a role or keepalive gets the longer line.
//!
//! Field order is the order the fields are declared in, so the lines this side
//! writes are byte-for-byte the ones above.

use serde::{Deserialize, Serialize};

/// The host's handshake acknowledgement: `welcome.echo` is this string.
pub const WELCOME_ECHO: &str = "plaza-orchestrator";

/// A line the guest sends to the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestMessage {
    /// `hello` — the first line of every session: who the guest is.
    Hello {
        /// The VM's index within the spawn.
        vm: u32,
        /// The guest's vsock CID.
        guest_cid: u32,
        /// v2: the role the guest declares (`plaza.role=`); absent or empty
        /// when it has none. A wrong type is a parse error.
        #[serde(default)]
        role: Option<String>,
        /// v2: `true` when the guest runs a long-lived instance
        /// (`plaza.keepalive=1`). A wrong type is a parse error.
        #[serde(default)]
        keepalive: Option<bool>,
    },
    /// `tool_call` — a request for the orchestrator to run one tool (v1).
    ToolCall {
        /// The call's sequence number within the session, starting at 1.
        call: u64,
        /// The tool's name (`echo`, `uptime`).
        name: String,
        /// The tool's arguments.
        args: serde_json::Value,
    },
    /// `tool_result` — the agent's answer to a hub-issued `tool_call` (v2).
    ToolResult {
        /// The `call` number this answers.
        call: u64,
        /// Whether the tool succeeded.
        ok: bool,
        /// The tool's output, or the reason it failed.
        output: String,
    },
    /// `heartbeat` — the keepalive agent's liveness line (v2).
    Heartbeat {
        /// The agent's own sequence number, starting at 1.
        seq: u64,
    },
    /// `bye` — the session is over; the guest closes after this line.
    Bye {
        /// How many tool calls the guest served (or made).
        calls: u64,
    },
}

/// A line the orchestrator sends to the guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    /// `welcome` — the handshake is accepted.
    Welcome {
        /// Always [`WELCOME_ECHO`].
        echo: String,
        /// v2: the role the hub accepted (its identity for the session's
        /// lifetime); skipped entirely when `None`, which is what keeps v1's
        /// welcome byte-for-byte.
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        /// v2: whether the hub accepted a keepalive session; skipped when
        /// `None`.
        #[serde(skip_serializing_if = "Option::is_none")]
        keepalive: Option<bool>,
    },
    /// `tool_result` — the outcome of the `tool_call` numbered `call`.
    ToolResult {
        /// The `call` number this answers.
        call: u64,
        /// Whether the tool succeeded. A refused or failed tool is data
        /// (`ok: false` plus a message), not a protocol error.
        ok: bool,
        /// The tool's output, or the reason it was refused.
        output: String,
    },
    /// `tool_call` — the hub dispatching one tool to a keepalive agent (v2).
    ToolCall {
        /// The call's sequence number within the session, starting at 1.
        call: u64,
        /// The tool's name (`echo`, `uptime`).
        name: String,
        /// The tool's arguments.
        args: serde_json::Value,
    },
    /// `stop` — the hub ending a keepalive session; the agent answers `bye`
    /// (v2).
    Stop {
        /// Why the hub ended the session.
        reason: String,
    },
}

impl GuestMessage {
    /// Parses one protocol line.
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

impl HostMessage {
    /// Encodes this message as one protocol line, without its newline.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// The handshake acknowledgement for a keepalive `hello`: v1's welcome
    /// plus the role the hub accepted and the keepalive flag.
    ///
    /// `role` is echoed verbatim, including the empty role a guest with no
    /// `plaza.role=` sends — the acceptance the agent carries for its
    /// lifetime is exactly what it declared.
    pub fn keepalive_welcome(role: &str) -> Self {
        HostMessage::Welcome {
            echo: WELCOME_ECHO.to_string(),
            role: Some(role.to_string()),
            keepalive: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_frozen_guest_lines_parse() {
        assert_eq!(
            GuestMessage::from_line(r#"{"type":"hello","vm":0,"guest_cid":3}"#)
                .expect("hello parses"),
            GuestMessage::Hello {
                vm: 0,
                guest_cid: 3,
                role: None,
                keepalive: None,
            }
        );
        assert_eq!(
            GuestMessage::from_line(
                r#"{"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza-guest-3-hello"}}"#
            )
            .expect("tool_call parses"),
            GuestMessage::ToolCall {
                call: 1,
                name: "echo".to_string(),
                args: json!({ "text": "plaza-guest-3-hello" })
            }
        );
        assert_eq!(
            GuestMessage::from_line(r#"{"type":"bye","calls":2}"#).expect("bye parses"),
            GuestMessage::Bye { calls: 2 }
        );
    }

    #[test]
    fn the_frozen_host_lines_encode_byte_for_byte() {
        assert_eq!(
            HostMessage::Welcome {
                echo: WELCOME_ECHO.to_string(),
                role: None,
                keepalive: None,
            }
            .to_line()
            .expect("welcome encodes"),
            r#"{"type":"welcome","echo":"plaza-orchestrator"}"#
        );
        assert_eq!(
            HostMessage::ToolResult {
                call: 2,
                ok: true,
                output: "12.34 56.78".to_string()
            }
            .to_line()
            .expect("tool_result encodes"),
            r#"{"type":"tool_result","call":2,"ok":true,"output":"12.34 56.78"}"#
        );
    }

    #[test]
    fn a_guest_line_of_the_other_direction_does_not_parse() {
        assert!(GuestMessage::from_line(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#).is_err());
    }

    #[test]
    fn a_v2_hello_carries_the_role_and_the_keepalive_flag() {
        assert_eq!(
            GuestMessage::from_line(
                r#"{"type":"hello","vm":0,"guest_cid":3,"role":"builder","keepalive":true}"#
            )
            .expect("the keepalive hello parses"),
            GuestMessage::Hello {
                vm: 0,
                guest_cid: 3,
                role: Some("builder".to_string()),
                keepalive: Some(true),
            }
        );
        // An empty role is a role: a guest with no `plaza.role=` still says
        // keepalive, and the hub echoes the emptiness it accepted.
        assert_eq!(
            GuestMessage::from_line(r#"{"type":"hello","vm":0,"guest_cid":3,"role":"","keepalive":true}"#)
                .expect("the empty role parses"),
            GuestMessage::Hello {
                vm: 0,
                guest_cid: 3,
                role: Some(String::new()),
                keepalive: Some(true),
            }
        );
        // A role without keepalive is a v2 hello too.
        assert_eq!(
            GuestMessage::from_line(r#"{"type":"hello","vm":1,"guest_cid":4,"role":"builder"}"#)
                .expect("a bare role parses"),
            GuestMessage::Hello {
                vm: 1,
                guest_cid: 4,
                role: Some("builder".to_string()),
                keepalive: None,
            }
        );
    }

    #[test]
    fn a_wrong_typed_role_or_keepalive_is_a_parse_error() {
        // The session is dropped rather than guessed at: a `role` that is not
        // a string, or a `keepalive` that is not a bool, is a broken line.
        assert!(
            GuestMessage::from_line(r#"{"type":"hello","vm":0,"guest_cid":3,"role":7}"#).is_err()
        );
        assert!(
            GuestMessage::from_line(r#"{"type":"hello","vm":0,"guest_cid":3,"role":["a"]}"#).is_err()
        );
        assert!(
            GuestMessage::from_line(r#"{"type":"hello","vm":0,"guest_cid":3,"keepalive":"yes"}"#)
                .is_err()
        );
        assert!(
            GuestMessage::from_line(r#"{"type":"hello","vm":0,"guest_cid":3,"keepalive":1}"#)
                .is_err()
        );
    }

    #[test]
    fn the_v2_guest_lines_parse() {
        assert_eq!(
            GuestMessage::from_line(r#"{"type":"heartbeat","seq":7}"#).expect("heartbeat parses"),
            GuestMessage::Heartbeat { seq: 7 }
        );
        assert_eq!(
            GuestMessage::from_line(r#"{"type":"tool_result","call":1,"ok":true,"output":"hi"}"#)
                .expect("tool_result parses"),
            GuestMessage::ToolResult {
                call: 1,
                ok: true,
                output: "hi".to_string(),
            }
        );
    }

    #[test]
    fn the_keepalive_welcome_echoes_the_role_it_accepted() {
        assert_eq!(
            HostMessage::keepalive_welcome("builder")
                .to_line()
                .expect("welcome encodes"),
            r#"{"type":"welcome","echo":"plaza-orchestrator","role":"builder","keepalive":true}"#
        );
        // A guest with no role gets its emptiness echoed, not a made-up name.
        assert_eq!(
            HostMessage::keepalive_welcome("")
                .to_line()
                .expect("welcome encodes"),
            r#"{"type":"welcome","echo":"plaza-orchestrator","role":"","keepalive":true}"#
        );
    }

    #[test]
    fn the_v2_host_lines_encode_byte_for_byte() {
        assert_eq!(
            HostMessage::ToolCall {
                call: 3,
                name: "uptime".to_string(),
                args: json!({}),
            }
            .to_line()
            .expect("tool_call encodes"),
            r#"{"type":"tool_call","call":3,"name":"uptime","args":{}}"#
        );
        assert_eq!(
            HostMessage::Stop {
                reason: "slice proof".to_string()
            }
            .to_line()
            .expect("stop encodes"),
            r#"{"type":"stop","reason":"slice proof"}"#
        );
    }
}
