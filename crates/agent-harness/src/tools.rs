//! The tools a keepalive agent serves for the hub.
//!
//! The v1 session had the *orchestrator* run the tools (`echo`, `uptime`) and
//! the guest ask for them, because the point of that session was the wire
//! rather than the work. In the agent fleet the direction is the other way
//! round: the hub asks, the agent serves, and these are the tools it serves
//! today — `echo` and `uptime`, the message channel (`conversation`, which
//! writes the message to the agent's own `/workspace/messages.log`), and the
//! file tools that work on the guest's writable `/workspace` volume (see
//! [`crate::files`]).
//!
//! A tool's failure is data, not a protocol error: it is answered as
//! `{"ok":false,"output":"<why>"}`, so the hub can tell an agent that refused
//! from an agent that went silent. The reason is prefixed with the taxonomy
//! entry it is (`denied:`, `not found:`, `invalid input:`, `limit exceeded:`,
//! `unavailable:`, `internal:`), which is the machine-readable half of the
//! answer.
//!
//! The request/response path does not branch on tool names — a tool is an entry
//! in [`TOOLS`], and adding one is that entry plus its function. The table is
//! also the whole of the agent's authority: a tool the hub names that is not in
//! it is refused, never guessed at.

use std::fmt;
use std::fs;

use serde_json::Value;

use crate::egress;
use crate::files::{self, Workspace};
use crate::ide;
use crate::term;

/// The tools, by name. The `ide_*` tools are the owner's (see [`crate::ide`]); the model never gets them.
const TOOLS: [Tool; 27] = [
    Tool {
        name: "echo",
        run: echo,
    },
    Tool {
        name: "uptime",
        run: uptime,
    },
    Tool {
        name: "conversation",
        run: files::conversation,
    },
    Tool {
        name: "read_file",
        run: files::read_file,
    },
    Tool {
        name: "write_file",
        run: files::write_file,
    },
    Tool {
        name: "edit_file",
        run: files::edit_file,
    },
    Tool {
        name: "append_file",
        run: files::append_file,
    },
    Tool {
        name: "list",
        run: files::list,
    },
    Tool {
        name: "search",
        run: files::search,
    },
    Tool {
        name: "fetch_url",
        run: egress::fetch_url,
    },
    Tool {
        name: "download",
        run: files::download,
    },
    Tool {
        name: "ide_list",
        run: ide::list,
    },
    Tool {
        name: "ide_read",
        run: ide::read,
    },
    Tool {
        name: "ide_write",
        run: ide::write,
    },
    Tool {
        name: "ide_changes",
        run: ide::changes,
    },
    Tool {
        name: "ide_checkpoints",
        run: ide::list_checkpoints,
    },
    Tool {
        name: "ide_restore",
        run: ide::restore,
    },
    Tool {
        name: "ide_search",
        run: ide::search,
    },
    Tool {
        name: "ide_run_start",
        run: ide::run_start,
    },
    Tool {
        name: "ide_run_poll",
        run: ide::run_poll,
    },
    Tool {
        name: "ide_run_stop",
        run: ide::run_stop,
    },
    Tool {
        name: "ide_term_open",
        run: term::open,
    },
    Tool {
        name: "ide_term_write",
        run: term::write,
    },
    Tool {
        name: "ide_term_read",
        run: term::read,
    },
    Tool {
        name: "ide_term_resize",
        run: term::resize,
    },
    Tool {
        name: "ide_term_close",
        run: term::close,
    },
    Tool {
        name: "ide_term_list",
        run: term::list,
    },
];

/// One tool the agent serves.
struct Tool {
    /// The name the hub calls it by.
    name: &'static str,
    /// The tool itself: the workspace it works in, arguments in, output out.
    ///
    /// The workspace is passed to every tool so the table is uniform: a tool
    /// that does not use it says so with `_workspace`, and adding one that does
    /// needs no change to [`serve`] or to the session.
    run: fn(&Workspace, &Value) -> Result<String, ToolError>,
}

/// Why a tool could not produce an output.
///
/// The variants are the taxonomy the hub reads: what a `tool_result` carries is
/// `ok: false` and the [`Display`](fmt::Display) of one of these, whose first
/// word names the kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The agent serves no tool by that name.
    UnknownTool(String),
    /// The arguments are not what the tool's schema expects: {reason}
    InvalidInput {
        /// What was wrong with them.
        reason: String,
    },
    /// The tool needs something this guest does not have: {reason}
    Unavailable {
        /// What was missing.
        reason: String,
    },
    /// The call was refused — a path that leaves the workspace, a filesystem
    /// permission: {reason}
    Denied {
        /// Why it was refused.
        reason: String,
    },
    /// What the tool was asked for is not there: {reason}
    NotFound {
        /// What was not found.
        reason: String,
    },
    /// The call, or its result, is over a cap: {reason}
    LimitExceeded {
        /// Which cap, and by how much.
        reason: String,
    },
    /// The file changed since the caller read it: {reason} is its current hash, or `missing`
    Conflict {
        /// The file's current hash, or `missing` when there is no file.
        reason: String,
    },
    /// The tool failed in a way that is this side's own: {reason}
    Internal {
        /// What failed.
        reason: String,
    },
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool(name) => write!(formatter, "no such tool: {name}"),
            Self::InvalidInput { reason } => write!(formatter, "invalid input: {reason}"),
            Self::Unavailable { reason } => write!(formatter, "unavailable: {reason}"),
            Self::Denied { reason } => write!(formatter, "denied: {reason}"),
            Self::NotFound { reason } => write!(formatter, "not found: {reason}"),
            Self::LimitExceeded { reason } => write!(formatter, "limit exceeded: {reason}"),
            Self::Conflict { reason } => write!(formatter, "conflict: {reason}"),
            Self::Internal { reason } => write!(formatter, "internal: {reason}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<apz_ops::OpError> for ToolError {
    /// The shared operations' errors keep their kind and their reason, so the taxonomy the hub reads is the same.
    fn from(err: apz_ops::OpError) -> Self {
        use apz_ops::OpError;
        match err {
            OpError::InvalidInput(reason) => Self::InvalidInput { reason },
            OpError::Denied(reason) => Self::Denied { reason },
            OpError::NotFound(reason) => Self::NotFound { reason },
            OpError::LimitExceeded(reason) => Self::LimitExceeded { reason },
            OpError::Conflict(reason) => Self::Conflict { reason },
            OpError::Unavailable(reason) => Self::Unavailable { reason },
            OpError::Internal(reason) => Self::Internal { reason },
        }
    }
}

/// A reply as the JSON text a tool returns.
pub(crate) fn reply_json<T: serde::Serialize>(reply: &T) -> Result<String, ToolError> {
    serde_json::to_string(reply).map_err(|err| ToolError::Internal {
        reason: format!("cannot encode the reply: {err}"),
    })
}

/// Runs the tool `name`, in `workspace`, with `args`.
pub fn serve(workspace: &Workspace, name: &str, args: &Value) -> Result<String, ToolError> {
    match TOOLS.iter().find(|tool| tool.name == name) {
        Some(tool) => (tool.run)(workspace, args),
        None => Err(ToolError::UnknownTool(name.to_string())),
    }
}

/// True when [`serve`] runs a tool called `name`.
pub fn serves(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}

/// `echo`: says back the text it is given.
fn echo(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    match args.get("text").and_then(Value::as_str) {
        Some(text) => Ok(text.to_string()),
        None => Err(ToolError::InvalidInput {
            reason: "\"text\" must be a string".to_string(),
        }),
    }
}

/// `uptime`: this guest's uptime, as `/proc/uptime`'s first two fields.
fn uptime(_workspace: &Workspace, _args: &Value) -> Result<String, ToolError> {
    let raw = fs::read_to_string("/proc/uptime").map_err(|err| ToolError::Unavailable {
        reason: format!("cannot read /proc/uptime: {err}"),
    })?;
    uptime_fields(&raw).ok_or_else(|| ToolError::Unavailable {
        reason: "/proc/uptime does not hold two numbers".to_string(),
    })
}

/// The first two whitespace-separated fields of `/proc/uptime`, when both are
/// numbers.
///
/// Split out from [`uptime`] so the parsing is testable without the host's
/// clock, and so a host whose `/proc/uptime` is something else is reported as
/// unavailable rather than passed on as if it were an uptime.
fn uptime_fields(raw: &str) -> Option<String> {
    let mut fields = raw.split_whitespace();
    let up = fields.next()?;
    let idle = fields.next()?;
    up.parse::<f64>().ok()?;
    idle.parse::<f64>().ok()?;
    Some(format!("{up} {idle}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn echo_serves_and_refuses_off_schema_arguments() {
        let workspace = Workspace::default();
        assert_eq!(
            serve(&workspace, "echo", &json!({ "text": "hi" })).unwrap(),
            "hi"
        );
        assert!(matches!(
            serve(&workspace, "echo", &json!({ "text": 12 })),
            Err(ToolError::InvalidInput { .. })
        ));
    }

    #[test]
    fn an_unknown_tool_names_itself_and_refusals_name_their_kind() {
        let err = serve(&Workspace::default(), "exec", &json!({})).expect_err("exec is not served");
        assert_eq!(err.to_string(), "no such tool: exec");
        let reason = || "x".to_string();
        assert_eq!(
            ToolError::Denied { reason: reason() }.to_string(),
            "denied: x"
        );
        assert_eq!(
            ToolError::LimitExceeded { reason: reason() }.to_string(),
            "limit exceeded: x"
        );
    }
}
