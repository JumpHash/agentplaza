//! The frozen guest↔host wire protocol, guest side.
//!
//! One message is one line of newline-delimited UTF-8 JSON (see
//! [`crate::line`]). The two directions are separate types: the guest only
//! ever *sends* [`GuestMessage`] and only ever *receives* [`HostMessage`], so
//! a message of the wrong direction is a deserialization error rather than a
//! silently accepted one.
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

/// The host's handshake acknowledgement: `welcome.echo` must be this string.
pub const WELCOME_ECHO: &str = "plaza-orchestrator";

/// A line the guest sends to the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestMessage {
    /// `hello` — the first line of every session: who we are and which VM we
    /// are.
    Hello {
        /// The VM's index within the spawn (`0` for the first VM).
        vm: u32,
        /// This VM's vsock CID.
        guest_cid: u32,
    },
    /// `tool_call` — a request for the orchestrator to run one tool.
    ToolCall {
        /// The call's sequence number within the session, starting at 1.
        call: u64,
        /// The tool's name (`echo`, `uptime`).
        name: String,
        /// The tool's arguments, as the orchestrator's schema expects them.
        args: serde_json::Value,
    },
    /// `bye` — the session is over; the guest closes the connection after
    /// this line.
    Bye {
        /// How many tool calls this session made.
        calls: u64,
    },
}

/// A line the orchestrator sends to the guest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    /// `welcome` — the orchestrator accepted the handshake.
    Welcome {
        /// Must be [`WELCOME_ECHO`].
        echo: String,
    },
    /// `tool_result` — the outcome of the `tool_call` numbered `call`.
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

    #[test]
    fn hello_is_byte_for_byte_the_frozen_line() {
        let line = GuestMessage::Hello {
            vm: 0,
            guest_cid: 3,
        }
        .to_line()
        .expect("hello encodes");
        assert_eq!(line, r#"{"type":"hello","vm":0,"guest_cid":3}"#);
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
    fn welcome_and_tool_result_parse() {
        assert_eq!(
            HostMessage::from_line(r#"{"type":"welcome","echo":"plaza-orchestrator"}"#)
                .expect("welcome parses"),
            HostMessage::Welcome {
                echo: WELCOME_ECHO.to_string()
            }
        );
        assert_eq!(
            HostMessage::from_line(r#"{"type":"tool_result","call":2,"ok":true,"output":"12.34 56.78"}"#)
                .expect("tool_result parses"),
            HostMessage::ToolResult {
                call: 2,
                ok: true,
                output: "12.34 56.78".to_string()
            }
        );
    }

    #[test]
    fn a_host_line_of_the_other_direction_does_not_parse() {
        // A `tool_call` is a guest line: receiving one as the guest is a
        // protocol error, not a silent no-op.
        assert!(
            HostMessage::from_line(r#"{"type":"tool_call","call":1,"name":"echo","args":{}}"#)
                .is_err()
        );
    }
}
