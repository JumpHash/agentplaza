//! `process.Process/*` (E2B's `spec/envd/process/process.proto`): start a process (optionally with a
//! PTY), stream its output and exit, list running ones, attach to one, send input and signals.

use serde_json::Value;

use super::{Ctx, RpcError, StreamSink};

/// A unary method (`List`, `SendInput`, `SendSignal`, `CloseStdin`, `Update`).
pub fn unary(method: &str, request: &Value, ctx: &Ctx) -> Result<Value, RpcError> {
    let _ = (request, ctx);
    Err(RpcError::new(
        "unimplemented",
        format!("process.Process/{method}"),
    ))
}

/// A server-streaming method (`Start`, `Connect`): writes its events to `sink` until the process
/// ends or the client goes away (a failed write).
pub fn stream(
    method: &str,
    request: &Value,
    ctx: &Ctx,
    sink: &mut dyn StreamSink,
) -> Result<(), RpcError> {
    let _ = (request, ctx, sink);
    Err(RpcError::new(
        "unimplemented",
        format!("process.Process/{method}"),
    ))
}
