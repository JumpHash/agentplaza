//! Loopback JSON calls to `noded` and the hub.

use std::time::Duration;

use apz_http::Response;
use serde_json::Value;

/// A daemon on `127.0.0.1:<port>`.
#[derive(Debug, Clone, Copy)]
pub struct Upstream {
    pub port: u16,
}

/// The default timeout for an upstream call.
pub const TIMEOUT: Duration = Duration::from_secs(10);

impl Upstream {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// `method path` with an optional JSON body; `(status, json)`.
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        let bytes = body.map(Value::to_string);
        let reply = apz_http::client::send(self.port, method, path, bytes.as_deref().map(str::as_bytes), TIMEOUT)
            .map_err(|e| format!("127.0.0.1:{} unreachable: {e}", self.port))?;
        let json = if reply.body.is_empty() { Value::Null } else { reply.json().unwrap_or(Value::Null) };
        Ok((reply.status, json))
    }

    pub fn get(&self, path: &str) -> Result<(u16, Value), String> {
        self.call("GET", path, None)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<(u16, Value), String> {
        self.call("POST", path, Some(body))
    }

    /// Passes the upstream answer through (502 when it is unreachable).
    pub fn proxy(&self, method: &str, path: &str, body: &[u8]) -> Response {
        let body = (!body.is_empty()).then_some(body);
        match apz_http::client::send(self.port, method, path, body, TIMEOUT) {
            Ok(reply) => Response::new(reply.status, apz_http::JSON, reply.body),
            Err(e) => Response::error(502, &format!("127.0.0.1:{} unreachable: {e}", self.port)),
        }
    }
}
