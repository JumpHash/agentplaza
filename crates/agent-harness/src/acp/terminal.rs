//! The engine's `terminal/*` methods: commands it runs in the VM through the client.
//!
//! The caps match the IDE's runs: at most [`MAX_TERMINALS`] at once per engine session, output kept
//! in a ring of the engine's `outputByteLimit` (capped), each command's whole process group killed on
//! `kill`, `release` and when the session ends. Commands run as the agent, in a folder inside the
//! workspace. `terminal/create` is checked by the hub (`engine.exec`) before it gets here.

use std::time::Duration;

use serde_json::Value;

use crate::files::Workspace;

/// Terminals one engine session may have open at once.
pub const MAX_TERMINALS: usize = 4;
/// The output kept per terminal when the engine names no `outputByteLimit`.
pub const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
/// The most output kept per terminal, whatever the engine asks.
pub const MAX_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// How long `wait_for_exit` waits between checks of `cancelled`.
pub const WAIT_POLL: Duration = Duration::from_millis(50);

/// One engine session's terminals.
pub struct Terminals {
    workspace: Workspace,
}

impl Terminals {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }

    /// `terminal/create {sessionId, command, args?, env?: [{name, value}], cwd?, outputByteLimit?}`
    /// → `{"terminalId": …}`. `cwd` must be inside the workspace (default: its root).
    pub fn create(&self, params: &Value) -> Result<Value, String> {
        let _ = (&self.workspace, params);
        Err("terminals are not built yet".to_string())
    }

    /// `terminal/output {sessionId, terminalId}` → `{output, truncated, exitStatus?: {exitCode, signal}}`.
    pub fn output(&self, params: &Value) -> Result<Value, String> {
        let _ = params;
        Err("terminals are not built yet".to_string())
    }

    /// `terminal/wait_for_exit {sessionId, terminalId}` → `{exitCode, signal}`. Returns early with an
    /// error once `cancelled()` is true.
    pub fn wait_for_exit(
        &self,
        params: &Value,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Value, String> {
        let _ = (params, cancelled);
        Err("terminals are not built yet".to_string())
    }

    /// `terminal/kill {sessionId, terminalId}` → `{}`: kills the command's process group; the
    /// terminal stays until released.
    pub fn kill(&self, params: &Value) -> Result<Value, String> {
        let _ = params;
        Err("terminals are not built yet".to_string())
    }

    /// `terminal/release {sessionId, terminalId}` → `{}`: kills the command if it still runs and
    /// forgets the terminal.
    pub fn release(&self, params: &Value) -> Result<Value, String> {
        let _ = params;
        Err("terminals are not built yet".to_string())
    }

    /// Kills and forgets every terminal (the session is ending).
    pub fn release_all(&self) {}

    /// The commands run so far, in order, with their exit codes (`None` while running or when killed
    /// by a signal), for the turn's summary.
    pub fn ran(&self) -> Vec<(String, Option<i32>)> {
        Vec::new()
    }
}
