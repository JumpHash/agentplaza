//! The stdio bridge (`agentplaza mcp serve`): newline-delimited JSON-RPC from stdin, each message
//! POSTed to the hub's `/mcp` with the saved MCP token, and each answer written to stdout as one
//! line. The bridge keeps the `Mcp-Session-Id` the hub issues on `initialize`. When the hub answers
//! 404 (the session expired), it initializes again once and retries. It logs only to stderr.

use std::io::{self, BufRead, Write};
use std::time::Duration;

use apz_http::client::{self, Target};
use serde_json::{json, Value};

/// What the hub answered to one POST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    /// The `Mcp-Session-Id` header, when the answer carries one.
    pub session: Option<String>,
    pub body: Vec<u8>,
}

/// One POST of a JSON-RPC message (or a batch) to the hub's `/mcp`.
pub trait Transport {
    fn post(&mut self, message: &str, session: Option<&str>) -> Result<Answer, String>;
}

/// The real transport: the hub over HTTP(S), with the MCP token as the bearer.
pub struct HubTransport {
    target: Target,
    timeout: Duration,
}

impl HubTransport {
    pub fn new(target: Target, timeout: Duration) -> HubTransport {
        HubTransport { target, timeout }
    }
}

impl Transport for HubTransport {
    fn post(&mut self, message: &str, session: Option<&str>) -> Result<Answer, String> {
        let mut target = self.target.clone();
        target.headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json, text/event-stream".to_string()),
        ];
        if let Some(id) = session {
            target.headers.push(("Mcp-Session-Id".to_string(), id.to_string()));
        }
        let reply = client::request(&target, "POST", "/mcp", Some(message.as_bytes()), self.timeout)
            .map_err(|e| format!("{}: {e}", self.target.base))?;
        let session = reply
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("mcp-session-id"))
            .map(|(_, value)| value.clone());
        Ok(Answer { status: reply.status, session, body: reply.body })
    }
}

/// The relay state: the session, and the `initialize` and `notifications/initialized` messages to
/// replay when the session has to be started again.
pub struct Bridge<T: Transport> {
    transport: T,
    session: Option<String>,
    initialize: Option<String>,
    initialized: Option<String>,
}

impl<T: Transport> Bridge<T> {
    pub fn new(transport: T) -> Bridge<T> {
        Bridge { transport, session: None, initialize: None, initialized: None }
    }

    /// One message in, at most one line out. None means nothing goes to stdout (a notification the
    /// hub accepted with no answer).
    pub fn relay(&mut self, line: &str) -> Option<String> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => return Some(error_line(&Value::Null, -32700, "parse error")),
        };
        let expects_answer = expects_answer(&message);
        let id = request_id(&message);
        let method = message.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let is_initialize = method == "initialize";
        if is_initialize {
            self.initialize = Some(line.to_string());
        }
        if method == "notifications/initialized" {
            self.initialized = Some(line.to_string());
        }

        let mut answer = match self.send(line, !is_initialize) {
            Ok(answer) => answer,
            Err(err) => return fail(&err, &id, expects_answer),
        };
        if answer.status == 404 && !is_initialize && self.initialize.is_some() {
            eprintln!("agentplaza mcp: the session expired; initializing again");
            let retried = self.reinitialize().and_then(|()| self.send(line, true));
            answer = match retried {
                Ok(answer) => answer,
                Err(err) => return fail(&err, &id, expects_answer),
            };
        }
        reply_line(&answer, &id, expects_answer)
    }

    /// POSTs one message, with the session when `with_session`, and keeps any session the answer names.
    fn send(&mut self, line: &str, with_session: bool) -> Result<Answer, String> {
        let session = if with_session { self.session.clone() } else { None };
        let answer = self.transport.post(line, session.as_deref())?;
        if let Some(id) = &answer.session {
            self.session = Some(id.clone());
        }
        Ok(answer)
    }

    /// Starts the session again: `initialize` with no session, then the `initialized` notification
    /// if the client sent one. Neither answer goes to stdout.
    fn reinitialize(&mut self) -> Result<(), String> {
        self.session = None;
        let init = self.initialize.clone().unwrap_or_default();
        let answer = self.send(&init, false)?;
        if !(200..300).contains(&answer.status) {
            return Err(format!("the hub would not initialize again: HTTP {}", answer.status));
        }
        if let Some(note) = self.initialized.clone() {
            self.send(&note, true)?;
        }
        Ok(())
    }
}

/// Relays every line from `input` through a bridge over `transport`, writing one line per answer.
pub fn serve<T: Transport>(transport: T, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut bridge = Bridge::new(transport);
    for line in input.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(reply) = bridge.relay(line) {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// A JSON-RPC error answer for `id`.
pub fn error_line(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// A failure to deliver: logged to stderr, and answered when the message wanted an answer.
fn fail(err: &str, id: &Value, expects_answer: bool) -> Option<String> {
    eprintln!("agentplaza mcp: {err}");
    expects_answer.then(|| error_line(id, -32000, err))
}

/// The line to write for an answer: the body as one compact line on 2xx; otherwise an error for
/// a request, and a log line for a notification.
fn reply_line(answer: &Answer, id: &Value, expects_answer: bool) -> Option<String> {
    let ok = (200..300).contains(&answer.status);
    if ok {
        if let Ok(value) = serde_json::from_slice::<Value>(&answer.body) {
            return Some(value.to_string());
        }
        if answer.body.iter().all(u8::is_ascii_whitespace) {
            // 202 Accepted: a notification or a response, which has no answer.
            return expects_answer.then(|| error_line(id, -32603, "the hub sent no answer"));
        }
    }
    let text = String::from_utf8_lossy(&answer.body).replace(['\n', '\r'], " ");
    let text = text.trim();
    let detail = if text.is_empty() { String::new() } else { format!(": {}", text.chars().take(200).collect::<String>()) };
    let message = format!("the hub answered HTTP {}{detail}", answer.status);
    if expects_answer {
        Some(error_line(id, -32000, &message))
    } else {
        eprintln!("agentplaza mcp: {message}");
        None
    }
}

/// Whether a message (or a batch) holds a request, which is owed an answer.
fn expects_answer(message: &Value) -> bool {
    let is_request = |m: &Value| m.get("method").is_some() && m.get("id").is_some();
    match message {
        Value::Array(items) => items.iter().any(is_request),
        other => is_request(other),
    }
}

/// The id to answer with: a request's own id; a batch has none.
fn request_id(message: &Value) -> Value {
    match message {
        Value::Array(_) => Value::Null,
        other => other.get("id").cloned().unwrap_or(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::Cursor;

    use super::*;

    const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
    const NOTE: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    const LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    const LIST3: &str = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#;
    const ANSWER: &str = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#;

    /// A transport that records each POST and answers from a script.
    #[derive(Default)]
    struct Fake {
        posts: Vec<(String, Option<String>)>,
        replies: VecDeque<Result<Answer, String>>,
    }

    impl Fake {
        fn scripted(replies: Vec<Result<Answer, String>>) -> Fake {
            Fake { posts: Vec::new(), replies: replies.into() }
        }
    }

    impl Transport for Fake {
        fn post(&mut self, message: &str, session: Option<&str>) -> Result<Answer, String> {
            self.posts.push((message.to_string(), session.map(str::to_string)));
            self.replies.pop_front().unwrap_or_else(|| Err("no scripted reply".to_string()))
        }
    }

    fn answer(status: u16, session: Option<&str>, body: &str) -> Result<Answer, String> {
        Ok(Answer { status, session: session.map(str::to_string), body: body.as_bytes().to_vec() })
    }

    fn ok(body: &str) -> Result<Answer, String> {
        answer(200, None, body)
    }

    #[test]
    fn the_session_is_captured_and_sent_on_later_requests() {
        let mut bridge = Bridge::new(Fake::scripted(vec![answer(200, Some("s1"), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#), ok(ANSWER)]));
        assert!(bridge.relay(INIT).is_some());
        assert!(bridge.relay(LIST).is_some());
        assert_eq!(bridge.transport.posts[0], (INIT.to_string(), None), "initialize carries no session");
        assert_eq!(bridge.transport.posts[1], (LIST.to_string(), Some("s1".to_string())));
    }

    #[test]
    fn a_notification_gets_no_output_line() {
        let mut out = Vec::new();
        serve(Fake::scripted(vec![answer(202, None, "")]), Cursor::new(format!("{NOTE}\n")), &mut out).unwrap();
        assert!(out.is_empty(), "a 202 to a notification writes nothing: {out:?}");

        let mut bridge = Bridge::new(Fake::scripted(vec![answer(202, None, "")]));
        assert_eq!(bridge.relay(NOTE), None);
    }

    #[test]
    fn a_404_triggers_one_reinitialize_and_a_retry() {
        let mut bridge = Bridge::new(Fake::scripted(vec![
            answer(200, Some("s1"), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            answer(202, None, ""),
            answer(404, None, "unknown or expired session"),
            answer(200, Some("s2"), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            answer(202, None, ""),
            ok(ANSWER),
        ]));
        assert!(bridge.relay(INIT).is_some());
        assert_eq!(bridge.relay(NOTE), None);
        let written = bridge.relay(LIST).expect("the retry is answered");
        assert_eq!(serde_json::from_str::<Value>(&written).unwrap(), serde_json::from_str::<Value>(ANSWER).unwrap(), "the retry's answer is the one written");

        let posts = &bridge.transport.posts;
        assert_eq!(posts.len(), 6, "initialize, notification, failed list, initialize again, notification, retry");
        assert_eq!(posts[2], (LIST.to_string(), Some("s1".to_string())));
        assert_eq!(posts[3], (INIT.to_string(), None), "the initialize is sent again with no session");
        assert_eq!(posts[4], (NOTE.to_string(), Some("s2".to_string())), "initialized is replayed on the new session");
        assert_eq!(posts[5], (LIST.to_string(), Some("s2".to_string())));
    }

    #[test]
    fn a_second_404_is_an_error_answer_not_a_loop() {
        let mut bridge = Bridge::new(Fake::scripted(vec![
            answer(200, Some("s1"), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            answer(404, None, "gone"),
            answer(200, Some("s2"), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            answer(404, None, "gone"),
        ]));
        bridge.relay(INIT);
        let line = bridge.relay(LIST3).expect("a request is owed an answer");
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 3);
        assert_eq!(value["error"]["code"], -32000);
        assert_eq!(bridge.transport.posts.len(), 4, "one retry, then the error");
    }

    #[test]
    fn a_batch_passes_through_as_one_line() {
        let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"tools/list"}]"#;
        let pretty = "[\n  {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}},\n  {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}\n]";
        let mut bridge = Bridge::new(Fake::scripted(vec![ok(pretty)]));
        let line = bridge.relay(batch).expect("a batch with requests is answered");
        assert_eq!(bridge.transport.posts[0].0, batch, "the batch is forwarded as it came");
        assert!(!line.contains('\n'), "one line: {line}");
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value.as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn unparseable_input_is_a_parse_error_and_never_posted() {
        let mut bridge = Bridge::new(Fake::scripted(Vec::new()));
        let line = bridge.relay("{not json").expect("a parse error is answered");
        assert!(line.contains("-32700"));
        assert!(bridge.transport.posts.is_empty());
    }

    #[test]
    fn a_transport_failure_answers_the_request_and_only_logs_a_notification() {
        let mut bridge = Bridge::new(Fake::scripted(vec![Err("connection refused".into()), Err("connection refused".into())]));
        let line = bridge.relay(LIST).expect("a request is answered");
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 2);
        assert_eq!(bridge.relay(NOTE), None);
    }

    #[test]
    fn serve_writes_one_line_per_answer_and_stops_when_stdin_closes() {
        let input = format!("{INIT}\n\n{NOTE}\n{LIST}\n");
        let transport = Fake::scripted(vec![answer(200, Some("s1"), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#), answer(202, None, ""), ok(ANSWER)]);
        let mut out = Vec::new();
        serve(transport, Cursor::new(input), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2, "initialize and tools/list answer; the notification does not");
    }
}
