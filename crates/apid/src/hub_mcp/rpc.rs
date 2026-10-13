//! JSON-RPC 2.0 for the hub's MCP server: the envelope, the error codes and version negotiation.

use serde_json::{json, Value};

use crate::mcp::client::PROTOCOLS;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// Over the per-token rate limit. `data.retryAfter` says how many milliseconds to wait.
pub const RATE_LIMITED: i64 = -32029;

/// The version to answer with: the client's, when this server speaks it, else the newest this server speaks.
pub fn negotiate(requested: &str) -> &'static str {
    PROTOCOLS.iter().copied().find(|offered| *offered == requested).unwrap_or(PROTOCOLS[0])
}

pub fn result(id: &Value, value: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": value })
}

pub fn failure(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// One message of a body, by what it asks for.
pub enum Message<'a> {
    /// A request: it has an id and gets an answer.
    Request { id: Value, method: &'a str, params: Value },
    /// A notification: no id, no answer.
    Notification,
    /// Not a JSON-RPC 2.0 request.
    Invalid,
}

pub fn classify(value: &Value) -> Message<'_> {
    let Some(object) = value.as_object() else {
        return Message::Invalid;
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Message::Invalid;
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Message::Invalid;
    };
    match object.get("id") {
        None => Message::Notification,
        Some(id @ (Value::String(_) | Value::Number(_))) => Message::Request {
            id: id.clone(),
            method,
            params: object.get("params").cloned().unwrap_or(Value::Null),
        },
        Some(_) => Message::Invalid,
    }
}
