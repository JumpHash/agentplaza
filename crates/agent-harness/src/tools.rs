//! The tools a keepalive agent serves for the hub.
//!
//! The v1 session had the *orchestrator* run the tools (`echo`, `uptime`) and
//! the guest ask for them, because the point of that session was the wire
//! rather than the work. In the agent fleet the direction is the other way
//! round: the hub asks, the agent serves, and these are the tools it serves
//! today — `echo` and `uptime`, plus the file tools that work on the guest's
//! writable `/workspace` volume (see [`crate::files`]).
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

use crate::files::{self, Workspace};

/// The tools, by name.
const TOOLS: [Tool; 8] = [
    Tool {
        name: "echo",
        run: echo,
    },
    Tool {
        name: "uptime",
        run: uptime,
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
            Self::Internal { reason } => write!(formatter, "internal: {reason}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Runs the tool `name`, in `workspace`, with `args`.
pub fn serve(workspace: &Workspace, name: &str, args: &Value) -> Result<String, ToolError> {
    match TOOLS.iter().find(|tool| tool.name == name) {
        Some(tool) => (tool.run)(workspace, args),
        None => Err(ToolError::UnknownTool(name.to_string())),
    }
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

    /// The tools that do not touch the filesystem are asked in the guest's own
    /// workspace, which they never resolve.
    fn workspace() -> Workspace {
        Workspace::default()
    }

    #[test]
    fn echo_says_back_the_text_it_is_given() {
        assert_eq!(
            serve(
                &workspace(),
                "echo",
                &json!({ "text": "plaza-guest-3-hello" })
            )
            .expect("echo serves"),
            "plaza-guest-3-hello"
        );
    }

    #[test]
    fn echo_refuses_arguments_that_are_not_its_schema() {
        assert!(matches!(
            serve(&workspace(), "echo", &json!({})),
            Err(ToolError::InvalidInput { .. })
        ));
        assert!(matches!(
            serve(&workspace(), "echo", &json!({ "text": 12 })),
            Err(ToolError::InvalidInput { .. })
        ));
    }

    #[test]
    fn an_unknown_tool_names_itself() {
        let err = serve(&workspace(), "exec", &json!({})).expect_err("exec is not served");
        assert_eq!(err, ToolError::UnknownTool("exec".to_string()));
        assert_eq!(err.to_string(), "no such tool: exec");
    }

    #[test]
    fn the_taxonomy_reads_as_the_hub_sees_it() {
        assert_eq!(
            ToolError::InvalidInput {
                reason: "x".to_string()
            }
            .to_string(),
            "invalid input: x"
        );
        assert_eq!(
            ToolError::Denied {
                reason: "x".to_string()
            }
            .to_string(),
            "denied: x"
        );
        assert_eq!(
            ToolError::NotFound {
                reason: "x".to_string()
            }
            .to_string(),
            "not found: x"
        );
        assert_eq!(
            ToolError::LimitExceeded {
                reason: "x".to_string()
            }
            .to_string(),
            "limit exceeded: x"
        );
        assert_eq!(
            ToolError::Unavailable {
                reason: "x".to_string()
            }
            .to_string(),
            "unavailable: x"
        );
        assert_eq!(
            ToolError::Internal {
                reason: "x".to_string()
            }
            .to_string(),
            "internal: x"
        );
    }

    #[test]
    fn the_served_tools_are_echo_uptime_and_the_file_tools() {
        let names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
        assert_eq!(
            names,
            vec![
                "echo",
                "uptime",
                "read_file",
                "write_file",
                "edit_file",
                "append_file",
                "list",
                "search"
            ]
        );
    }

    #[test]
    fn uptime_is_the_first_two_fields_of_proc_uptime() {
        assert_eq!(
            uptime_fields("12.34 56.78 0.00 0\n").as_deref(),
            Some("12.34 56.78")
        );
        assert_eq!(uptime_fields("12.34\n").as_deref(), None);
        assert_eq!(uptime_fields("up days\n").as_deref(), None);
        assert_eq!(uptime_fields("").as_deref(), None);
    }

    #[test]
    fn this_guest_serves_its_own_uptime() {
        // The shipped tool reads the guest's /proc/uptime; on the Linux guests
        // this harness runs in, that is two numbers.
        let output = serve(&workspace(), "uptime", &json!({})).expect("the guest has an uptime");
        assert_eq!(output.split_whitespace().count(), 2, "{output}");
        assert!(output.split_whitespace().all(|f| f.parse::<f64>().is_ok()));
    }
}
