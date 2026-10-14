//! The owner's terminals: a shell on a pseudo-terminal in this VM, served as operator tools.
//!
//! The terminals are [`apz_ops::Terminals`], which the workstation daemon shares: a login shell on a PTY, a ring
//! of its output, an idle janitor. This module reads the tools' arguments, gives the shell the agent's environment
//! (see [`crate::bash::agent_env`]) and serves the replies as JSON. The tool names and replies are unchanged:
//! `ide_term_open`, `ide_term_write`, `ide_term_read`, `ide_term_resize`, `ide_term_close`, `ide_term_list`.

use std::sync::LazyLock;

use apz_ops::{Root, Terminals};
use serde_json::Value;

use crate::bash;
use crate::files::{self, Workspace};
use crate::tools::{ToolError, reply_json};

/// The terminals this harness holds. One process, one workspace: a static is enough.
static TERMINALS: LazyLock<Terminals> = LazyLock::new(Terminals::new);

/// `ide_term_open {cols, rows, cwd?}`: opens a shell in a new terminal.
///
/// Output: `{"term": "term-<n>"}`. `cwd` is a directory of the workspace, relative to its root, and defaults
/// to the root. At most four terminals are open.
pub fn open(workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let cwd = match args.get("cwd") {
        None | Some(Value::Null) => "",
        Some(Value::String(cwd)) => cwd.as_str(),
        Some(_) => return Err(files::invalid("\"cwd\" must be a string")),
    };
    let env = bash::agent_env(workspace.dir()).map_err(internal)?;
    reply_json(&TERMINALS.open(
        &Root::new(workspace.dir()),
        &env,
        size(args, "cols"),
        size(args, "rows"),
        cwd,
    )?)
}

/// `ide_term_write {term, data_b64}`: writes bytes to the terminal, at most 64 KiB of them.
///
/// Output: `{"ok", "written"}`. `ok` is false when not everything went in before the write's wait ran out; the
/// caller sends the rest from `written` on, in a later call.
pub fn write(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let data = files::string_arg(args, "data_b64")?;
    let term = files::string_arg(args, "term")?;
    reply_json(&TERMINALS.write(&term, &data)?)
}

/// `ide_term_read {term, offset?}`: the terminal's output from `offset`, at most 47 KiB of it.
///
/// Output: `{"term", "offset", "next", "data_b64", "exited", "exit_code", "lost"}`. `offset` is where `data_b64`
/// starts, which is later than asked by `lost` bytes when the ring dropped them.
pub fn read(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let offset = match args.get("offset") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| files::invalid("\"offset\" must be a whole number of bytes"))?,
        ),
    };
    let term = files::string_arg(args, "term")?;
    reply_json(&TERMINALS.read(&term, offset)?)
}

/// `ide_term_resize {term, cols, rows}`: sets the terminal's size.
pub fn resize(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let term = files::string_arg(args, "term")?;
    reply_json(&TERMINALS.resize(&term, size(args, "cols"), size(args, "rows"))?)
}

/// `ide_term_close {term}`: closes the terminal and ends its shell and its process group.
/// Output: `{"term", "closed": true}`.
pub fn close(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let term = files::string_arg(args, "term")?;
    reply_json(&TERMINALS.close(&term)?)
}

/// `ide_term_list {}`: the open terminals. Output: `{"terminals": [{"term", "cols", "rows", "started_ms", "exited"}]}`.
pub fn list(_workspace: &Workspace, _args: &Value) -> Result<String, ToolError> {
    reply_json(&TERMINALS.list()?)
}

/// A size argument. One that is not a whole number is passed on as out of range, which the operation refuses
/// with its own message.
fn size(args: &Value, key: &str) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn internal(reason: impl Into<String>) -> ToolError {
    ToolError::Internal {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> (PathBuf, Workspace) {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-term-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        let workspace = Workspace::at(&dir);
        (dir, workspace)
    }

    fn reply(result: Result<String, ToolError>) -> Value {
        serde_json::from_str(&result.expect("the tool succeeds")).expect("the reply is JSON")
    }

    #[test]
    fn a_terminal_opens_runs_a_command_and_closes_through_its_names() {
        let (_dir, ws) = scratch("round-trip");
        let opened = reply(tools::serve(
            &ws,
            "ide_term_open",
            &json!({ "cols": 80, "rows": 24 }),
        ));
        let term = opened["term"].as_str().unwrap().to_string();
        let listed = reply(tools::serve(&ws, "ide_term_list", &json!({})));
        assert_eq!(listed["terminals"][0]["term"], json!(term));

        let typed = reply(tools::serve(
            &ws,
            "ide_term_write",
            &json!({ "term": term, "data_b64": base64_encode("echo hi$((1+1))\n") }),
        ));
        assert_eq!(typed["ok"], json!(true));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut offset = 0;
        let mut seen = String::new();
        while !seen.contains("hi2") {
            let got = reply(tools::serve(
                &ws,
                "ide_term_read",
                &json!({ "term": term, "offset": offset }),
            ));
            offset = got["next"].as_u64().unwrap();
            let bytes = base64_decode(got["data_b64"].as_str().unwrap());
            seen.push_str(&String::from_utf8_lossy(&bytes));
            assert!(std::time::Instant::now() < deadline, "no output: {seen:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let closed = reply(tools::serve(
            &ws,
            "ide_term_close",
            &json!({ "term": term }),
        ));
        assert_eq!(closed, json!({ "term": term, "closed": true }));
        assert!(tools::serve(&ws, "ide_term_close", &json!({ "term": term })).is_err());
    }

    fn base64_encode(text: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    fn base64_decode(text: &str) -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .expect("base64")
    }
}
