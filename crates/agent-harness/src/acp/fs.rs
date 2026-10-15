//! The engine's `fs/read_text_file` and `fs/write_text_file`, confined to the workspace.
//!
//! ACP names files by absolute path. The engine runs with its cwd inside the workspace
//! (`/workspace/<path>`), so a path it may touch starts with the workspace root. Anything else, a
//! `..` component, a symlink that resolves outside, or the harness's own `.plaza/` is refused: the
//! same rules the file tools follow (`crate::files`). Both methods are checked by the hub before
//! they run (`engine.read`, `engine.edit`); this module only does the confined I/O.

use serde_json::Value;

use crate::files::Workspace;

/// The most `fs/read_text_file` returns, in bytes, after `line` and `limit` are applied.
pub const MAX_READ_BYTES: usize = 4 * 1024 * 1024;
/// The most `fs/write_text_file` writes, in bytes.
pub const MAX_WRITE_BYTES: usize = 8 * 1024 * 1024;

/// The workspace-relative label of the absolute `path` an engine names (what the hub's check and the
/// transcript show), or why it is refused.
pub fn label(workspace: &Workspace, path: &str) -> Result<String, String> {
    let _ = (workspace, path);
    Err("fs is not built yet".to_string())
}

/// `fs/read_text_file {sessionId, path, line?, limit?}` → `{"content": …}`. `line` is 1-based and
/// `limit` counts lines; a file that is not UTF-8 or is larger than [`MAX_READ_BYTES`] is refused.
pub fn read_text_file(workspace: &Workspace, params: &Value) -> Result<Value, String> {
    let _ = (workspace, params);
    Err("fs is not built yet".to_string())
}

/// `fs/write_text_file {sessionId, path, content}` → `null`. Creates missing parent folders and
/// writes atomically; refuses content larger than [`MAX_WRITE_BYTES`].
pub fn write_text_file(workspace: &Workspace, params: &Value) -> Result<Value, String> {
    let _ = (workspace, params);
    Err("fs is not built yet".to_string())
}
