//! The frozen guest↔host wire protocol, host side.
//!
//! One message is one line of newline-delimited UTF-8 JSON (see
//! [`crate::line`]). The two directions are separate types: the orchestrator
//! only ever *receives* [`GuestMessage`] and only ever *sends*
//! [`HostMessage`], so a message of the wrong direction is a deserialization
//! error rather than a silently accepted one.
//!
//! The shapes are frozen by the slice contract:
//!
//! ```text
//! guest → host  {"type":"hello","vm":<i>,"guest_cid":<cid>}
//! host  → guest {"type":"welcome","echo":"plaza-orchestrator"}
//! guest → host  {"type":"tool_call","call":<n>,"name":"echo","args":{"text":"<s>"}}
//! host  → guest {"type":"tool_result","call":<n>,"ok":true,"output":"<s>"}
//! guest → host  {"type":"bye","calls":<n>}
//! ```
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
    },
    /// `tool_call` — a request for the orchestrator to run one tool.
    ToolCall {
        /// The call's sequence number within the session, starting at 1.
        call: u64,
        /// The tool's name (`echo`, `uptime`).
        name: String,
        /// The tool's arguments.
        args: serde_json::Value,
    },
    /// `bye` — the session is over; the guest closes after this line.
    Bye {
        /// How many tool calls the guest made.
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
                guest_cid: 3
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
                echo: WELCOME_ECHO.to_string()
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
}
