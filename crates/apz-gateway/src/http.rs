//! The one HTTP client every adapter uses: retries with backoff, error bodies
//! scrubbed of the credential, and a server-sent-events reader.

use std::io::{BufRead, BufReader, Read};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use ureq::Body;
use ureq::http::Response;

use crate::error::{GatewayError, redact, retryable_status, truncate};

/// Longest provider error body kept in a [`GatewayError::Http`].
const MAX_ERROR_BODY: usize = 2000;
/// Longest `retry-after` honoured.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// One request, built by an adapter.
pub(crate) struct Req {
    post: bool,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The credential this request carries; scrubbed from error bodies.
    secret: Option<String>,
}

impl Req {
    pub fn get(url: impl Into<String>) -> Self {
        Req { post: false, url: url.into(), headers: Vec::new(), body: Vec::new(), secret: None }
    }

    pub fn post_json(url: impl Into<String>, body: &Value) -> Self {
        let mut req = Req { post: true, body: body.to_string().into_bytes(), ..Req::get(url) };
        req.set_header("content-type", "application/json");
        req
    }

    /// Sets a header, replacing any of the same name.
    pub fn set_header(&mut self, name: &str, value: &str) {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), value.to_string()));
    }

    #[cfg(test)]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.set_header(name, value);
        self
    }

    /// A header that carries `secret` (the secret is scrubbed from errors).
    pub fn secret_header(mut self, name: &str, value: &str, secret: &str) -> Self {
        self.set_header(name, value);
        self.set_secret(secret);
        self
    }

    /// The credential to scrub from this request's errors.
    pub fn set_secret(&mut self, secret: &str) {
        self.secret = Some(secret.to_string());
    }

    pub fn bearer(self, token: &str) -> Self {
        self.secret_header("authorization", &format!("Bearer {token}"), token)
    }

    /// Applies a provider's `extra_headers`.
    pub fn extra_headers<'a>(mut self, extra: impl IntoIterator<Item = (&'a String, &'a String)>) -> Self {
        for (name, value) in extra {
            self.set_header(name, value);
        }
        self
    }

    pub fn method(&self) -> &'static str {
        if self.post { "POST" } else { "GET" }
    }

    #[cfg(test)]
    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The HTTP client: one connection pool, a retry policy.
pub(crate) struct Http {
    agent: ureq::Agent,
    retries: u32,
    base_delay: Duration,
}

impl Http {
    pub fn new(retries: u32, base_delay: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            // Long generations: time to the first byte, then the whole call.
            .timeout_recv_response(Some(Duration::from_secs(15 * 60)))
            .timeout_global(Some(Duration::from_secs(60 * 60)))
            .user_agent("agentplaza-gateway/0.1")
            .build();
        Http { agent: ureq::Agent::new_with_config(config), retries, base_delay }
    }

    /// Sends `req`, retrying connection errors and 408/409/429/5xx up to the
    /// policy's limit. A success status returns the response unread.
    pub fn send(&self, req: &Req) -> Result<Response<Body>, GatewayError> {
        let mut attempt = 0;
        loop {
            let delay = self.base_delay * 2u32.pow(attempt);
            match self.send_once(req) {
                Ok(resp) if resp.status().as_u16() < 400 => return Ok(resp),
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retryable = retryable_status(status);
                    let wait = retry_after(&resp).unwrap_or(delay);
                    let body = read_error_body(resp, req.secret.as_deref());
                    if retryable && attempt < self.retries {
                        attempt += 1;
                        thread::sleep(wait);
                        continue;
                    }
                    return Err(GatewayError::Http { status, body, retryable });
                }
                Err(e) => {
                    let retryable = matches!(
                        e,
                        ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::Timeout(_)
                    );
                    if retryable && attempt < self.retries {
                        attempt += 1;
                        thread::sleep(delay);
                        continue;
                    }
                    return Err(GatewayError::Transport(redact(&e.to_string(), req.secret.as_deref())));
                }
            }
        }
    }

    /// Sends `req` and parses the whole answer as JSON.
    pub fn json(&self, req: &Req) -> Result<Value, GatewayError> {
        let resp = self.send(req)?;
        let mut text = String::new();
        resp.into_body()
            .into_reader()
            .read_to_string(&mut text)
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| GatewayError::decode(format!("{e}: {}", truncate(&text, 200))))
    }

    fn send_once(&self, req: &Req) -> Result<Response<Body>, ureq::Error> {
        if req.post {
            let mut rb = self.agent.post(&req.url);
            for (name, value) in &req.headers {
                rb = rb.header(name, value);
            }
            rb.send(&req.body[..])
        } else {
            let mut rb = self.agent.get(&req.url);
            for (name, value) in &req.headers {
                rb = rb.header(name, value);
            }
            rb.call()
        }
    }
}

fn retry_after(resp: &Response<Body>) -> Option<Duration> {
    let header = |name| resp.headers().get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok());
    let wait = header("retry-after-ms").map(|ms| ms / 1000.0).or_else(|| header("retry-after"))?;
    (wait.is_finite() && wait >= 0.0).then(|| Duration::from_secs_f64(wait).min(MAX_RETRY_AFTER))
}

fn read_error_body(resp: Response<Body>, secret: Option<&str>) -> String {
    let mut bytes = Vec::new();
    let _ = resp.into_body().into_reader().take(64 * 1024).read_to_end(&mut bytes);
    truncate(redact(String::from_utf8_lossy(&bytes).trim(), secret).as_str(), MAX_ERROR_BODY)
}

/// The response body as a reader.
pub(crate) fn reader(resp: Response<Body>) -> impl Read {
    resp.into_body().into_reader()
}

/// Reads server-sent events, calling `on_event(event, data)` for each until
/// it returns `Ok(false)` or the stream ends. Multi-line data is joined with
/// `\n`; comments and `id:`/`retry:` fields are ignored.
pub(crate) fn sse(
    body: impl Read,
    mut on_event: impl FnMut(&str, &str) -> Result<bool, GatewayError>,
) -> Result<(), GatewayError> {
    let mut reader = BufReader::new(body);
    let mut line = Vec::new();
    let (mut event, mut data) = (String::new(), String::new());
    let mut has_data = false;
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).map_err(|e| GatewayError::Transport(format!("stream: {e}")))?;
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        if n == 0 || text.is_empty() {
            if has_data && !on_event(&event, &data)? {
                return Ok(());
            }
            event.clear();
            data.clear();
            has_data = false;
            if n == 0 {
                return Ok(());
            }
            continue;
        }
        let (field, value) = text.split_once(':').unwrap_or((text, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event = value.to_string(),
            "data" => {
                if has_data {
                    data.push('\n');
                }
                data.push_str(value);
                has_data = true;
            }
            _ => {}
        }
    }
}
