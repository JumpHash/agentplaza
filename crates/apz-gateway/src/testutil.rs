//! Test support: a scripted one-request-per-connection HTTP server on port 0
//! that records what it was sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use apz_proto::chat::{ChatRequest, Message};
use serde_json::Value;

use crate::{Auth, Gateway, ProviderConfig, ProviderKind};

/// One scripted answer.
pub struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    pub fn new(status: u16, content_type: &str, body: impl Into<String>) -> Self {
        Reply { status, headers: vec![("content-type".into(), content_type.into())], body: body.into() }
    }

    pub fn sse(body: impl Into<String>) -> Self {
        Reply::new(200, "text/event-stream", body)
    }

    pub fn json(status: u16, body: &Value) -> Self {
        Reply::new(status, "application/json", body.to_string())
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// A request the server received.
#[derive(Debug, Clone)]
pub struct Captured {
    pub method: String,
    /// Path and query.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Captured {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }
}

pub struct FakeServer {
    port: u16,
    requests: Arc<Mutex<Vec<Captured>>>,
}

impl FakeServer {
    /// Serves `replies` in order, one per connection, then stops listening.
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        thread::spawn(move || {
            for reply in replies {
                let Ok((stream, _)) = listener.accept() else { return };
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut lines = head.lines();
                let mut start = lines.next().unwrap_or("").split(' ');
                let (method, path) = (start.next().unwrap_or("").to_string(), start.next().unwrap_or("").to_string());
                let headers: Vec<(String, String)> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
                    .collect();
                let len = headers.iter().find(|(n, _)| n == "content-length").and_then(|(_, v)| v.parse().ok());
                let mut body = vec![0; len.unwrap_or(0)];
                reader.read_exact(&mut body).unwrap();
                log.lock().unwrap().push(Captured { method, path, headers, body });
                let mut out = format!("HTTP/1.1 {} X\r\nconnection: close\r\ncontent-length: {}\r\n", reply.status, reply.body.len());
                for (n, v) in &reply.headers {
                    out.push_str(&format!("{n}: {v}\r\n"));
                }
                out.push_str("\r\n");
                out.push_str(&reply.body);
                let mut stream = reader.into_inner();
                let _ = stream.write_all(out.as_bytes());
            }
        });
        FakeServer { port, requests }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<Captured> {
        self.requests.lock().unwrap().clone()
    }
}

/// A provider pointed at `server`.
pub fn provider(id: &str, kind: ProviderKind, server: &FakeServer, auth: Auth) -> ProviderConfig {
    ProviderConfig { base_url: Some(server.url()), auth, ..ProviderConfig::new(id, kind) }
}

/// A gateway whose environment has `env-<NAME>` for every non-AWS `*_KEY` /
/// `*_TOKEN` variable, the secret `s` = `sk-secret-value`, and fast retries.
pub fn gateway(providers: Vec<ProviderConfig>) -> Gateway {
    gateway_with_env(providers, &[])
}

/// [`gateway`] plus extra environment variables.
pub fn gateway_with_env(providers: Vec<ProviderConfig>, env: &[(&str, &str)]) -> Gateway {
    let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Gateway::builder()
        .providers(providers)
        .secrets(|name: &str, _: &crate::SecretRead<'_>| (name == "s").then(|| "sk-secret-value".to_string()))
        .env(move |name: &str| {
            if let Some((_, v)) = env.iter().find(|(k, _)| k == name) {
                return Some(v.clone());
            }
            let generic = !name.starts_with("AWS_") && (name.ends_with("_KEY") || name.ends_with("_TOKEN"));
            generic.then(|| format!("env-{name}"))
        })
        .retry(2, Duration::from_millis(5))
        .build()
}

/// A request with one user message and no model.
pub fn request(text: &str) -> ChatRequest {
    ChatRequest {
        model: String::new(),
        system: None,
        messages: vec![Message::user_text(text)],
        tools: Vec::new(),
        max_tokens: 0,
        temperature: None,
        effort: None,
    }
}

/// An SSE body of named events.
pub fn sse_body(events: &[(&str, &str)]) -> String {
    events.iter().map(|(e, d)| format!("event: {e}\ndata: {d}\n\n")).collect()
}

/// An SSE body of unnamed `data:` events.
pub fn sse_data(events: &[&str]) -> String {
    events.iter().map(|d| format!("data: {d}\r\n\r\n")).collect()
}
