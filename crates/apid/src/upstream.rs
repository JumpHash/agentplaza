//! Loopback JSON calls to `noded` and the hub.

use std::time::Duration;

use apz_http::Response;
use apz_http::auth::InternalToken;
use serde_json::Value;

/// A daemon on `127.0.0.1:<port>`, and the machine's internal token it expects on every call.
#[derive(Debug, Clone)]
pub struct Upstream {
    pub port: u16,
    pub token: InternalToken,
}

/// The default timeout for an upstream call.
pub const TIMEOUT: Duration = Duration::from_secs(10);

impl Upstream {
    /// A daemon that takes no token (an open daemon: tests, development).
    #[allow(dead_code)] // apid's own daemons are all guarded; the tests use it
    pub fn new(port: u16) -> Self {
        Self::with_token(port, InternalToken::none())
    }

    /// A daemon guarded by `token`: every call sends its header.
    pub fn with_token(port: u16, token: InternalToken) -> Self {
        Self { port, token }
    }

    /// One request to the daemon, with the internal token's header when one is named; the raw
    /// reply.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> std::io::Result<apz_http::client::Reply> {
        let token = self.token.header();
        let headers: Vec<(&str, &str)> = token
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        apz_http::client::send_with(self.port, method, path, &headers, body, timeout)
    }

    /// `method path` with an optional JSON body; `(status, json)`.
    pub fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), String> {
        let bytes = body.map(Value::to_string);
        let reply = self
            .send(method, path, bytes.as_deref().map(str::as_bytes), TIMEOUT)
            .map_err(|e| format!("127.0.0.1:{} unreachable: {e}", self.port))?;
        let json = if reply.body.is_empty() {
            Value::Null
        } else {
            reply.json().unwrap_or(Value::Null)
        };
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
        match self.send(method, path, body, TIMEOUT) {
            Ok(reply) => Response::new(reply.status, apz_http::JSON, reply.body),
            Err(e) => Response::error(502, &format!("127.0.0.1:{} unreachable: {e}", self.port)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_http::{Request, Server};

    /// A daemon on port 0 that answers 200 to anything, guarded by a token file holding `s3cret`.
    fn guarded_daemon(name: &str) -> (apz_http::Handle, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("apid-upstream-{name}-{}", std::process::id()));
        std::fs::write(&path, "s3cret\n").expect("the token file");
        let handler = InternalToken::file(&path)
            .guard(|_: &Request| Response::json(200, &serde_json::json!({"ok": true})));
        let handle = Server::bind(0)
            .expect("a port on loopback")
            .spawn(handler)
            .expect("the daemon serves");
        (handle, path)
    }

    #[test]
    fn with_token_is_admitted_and_no_token_is_refused() {
        let (daemon, path) = guarded_daemon("auth");
        let with = Upstream::with_token(daemon.port(), InternalToken::file(&path));
        let (status, body) = with.get("/v1/status").expect("the daemon answers");
        assert_eq!(status, 200);
        assert_eq!(body, serde_json::json!({"ok": true}));
        let without = Upstream::new(daemon.port());
        assert_eq!(
            without.get("/v1/status").expect("the daemon answers").0,
            401
        );
        let _ = std::fs::remove_file(&path);
    }
}
