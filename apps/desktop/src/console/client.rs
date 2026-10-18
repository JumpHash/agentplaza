//! The console's hub client: JSON over `apz_http::client::request` (plain HTTP on loopback,
//! pinned TLS to a remote hub), with the owner token when one is set.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Client {
    pub target: apz_http::client::Target,
    pub timeout: Duration,
}

impl Client {
    pub fn new(base: &str, pin: Option<[u8; 32]>, bearer: Option<String>) -> Client {
        Client {
            target: apz_http::client::Target {
                base: base.trim_end_matches('/').to_string(),
                pin,
                bearer,
                headers: Vec::new(),
            },
            timeout: Duration::from_secs(15),
        }
    }

    /// The hub's base URL, for display.
    pub fn base(&self) -> &str {
        &self.target.base
    }

    /// One call; a non-2xx answer becomes `Err` with the hub's `error` text.
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let (status, value) =
            apz_http::client::request_json(&self.target, method, path, body, self.timeout)
                .map_err(|e| e.to_string())?;
        if (200..300).contains(&status) {
            Ok(value)
        } else {
            Err(value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP {status}")))
        }
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        serde_json::from_value(self.call("GET", path, None)?).map_err(|e| e.to_string())
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, String> {
        let value = self.call("POST", path, Some(body))?;
        serde_json::from_value(if value.is_null() {
            Value::Object(Default::default())
        } else {
            value
        })
        .map_err(|e| e.to_string())
    }

    pub fn patch<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, String> {
        serde_json::from_value(self.call("PATCH", path, Some(body))?).map_err(|e| e.to_string())
    }

    pub fn put<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, String> {
        serde_json::from_value(self.call("PUT", path, Some(body))?).map_err(|e| e.to_string())
    }

    pub fn delete(&self, path: &str) -> Result<Value, String> {
        self.call("DELETE", path, None)
    }

    /// A `PUT` whose body is raw bytes (a file's), with a JSON answer. Big files get a minute per 16 MB.
    pub fn put_bytes<T: DeserializeOwned>(&self, path: &str, bytes: &[u8]) -> Result<T, String> {
        let timeout = self.timeout + Duration::from_secs(60 * (bytes.len() as u64 >> 24));
        let reply = apz_http::client::request(&self.target, "PUT", path, Some(bytes), timeout)
            .map_err(|e| e.to_string())?;
        let value: Value = if reply.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&reply.body).map_err(|e| e.to_string())?
        };
        if !(200..300).contains(&reply.status) {
            return Err(value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP {}", reply.status)));
        }
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

/// Runs a blocking client call as a task, mapping its result to a message.
pub fn task<T, M>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
    map: impl Fn(Result<T, String>) -> M + Send + 'static,
) -> iced::Task<M>
where
    T: Send + 'static,
    M: Send + 'static,
{
    iced::Task::perform(async move { f() }, map)
}

/// Percent-encodes a query or path value (model ids carry slashes).
pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::enc;

    #[test]
    fn enc_escapes_model_ids() {
        assert_eq!(
            enc("anthropic/claude-haiku-5-5"),
            "anthropic%2Fclaude-haiku-5-5"
        );
    }
}
