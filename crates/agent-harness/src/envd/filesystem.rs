//! `filesystem.Filesystem/*` (E2B's `spec/envd/filesystem/filesystem.proto`): stat, list, make a
//! directory, move, remove, and watch a directory.

use serde_json::Value;

use super::{Ctx, RpcError, StreamSink};

/// A unary method (`Stat`, `MakeDir`, `Move`, `ListDir`, `Remove`, `CreateWatcher`,
/// `GetWatcherEvents`, `RemoveWatcher`).
pub fn unary(method: &str, request: &Value, ctx: &Ctx) -> Result<Value, RpcError> {
    let _ = (request, ctx);
    Err(RpcError::new(
        "unimplemented",
        format!("filesystem.Filesystem/{method}"),
    ))
}

/// A server-streaming method (`WatchDir`).
pub fn stream(
    method: &str,
    request: &Value,
    ctx: &Ctx,
    sink: &mut dyn StreamSink,
) -> Result<(), RpcError> {
    let _ = (request, ctx, sink);
    Err(RpcError::new(
        "unimplemented",
        format!("filesystem.Filesystem/{method}"),
    ))
}
