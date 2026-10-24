//! The MCP client: JSON-RPC 2.0 over a [`Transport`] (a child's stdio, or Streamable HTTP).
//!
//! The client sends `initialize` (2025-06-18, falling back to 2025-03-26 when the server answers with a
//! version it does not offer), `notifications/initialized`, the paged `tools/list`, `resources/list`
//! and `prompts/list`, and `tools/call`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::time::Duration;

/// The protocol versions this client speaks, newest first.
pub const PROTOCOLS: [&str; 2] = ["2025-06-18", "2025-03-26"];
/// Most pages of one listing the client follows (a server that never ends its cursor is cut off).
const MAX_PAGES: usize = 100;

/// What went wrong talking to a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No answer in time.
    Timeout,
    /// The HTTP session was dropped (a 404 on a session request): the caller re-initializes once.
    SessionLost,
    /// The connection failed, or the process exited.
    Transport(String),
    /// The server answered with a JSON-RPC error.
    Rpc { code: i64, message: String },
    /// The server answered something this client cannot use.
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Timeout => write!(f, "timed out"),
            Error::SessionLost => write!(f, "the session expired"),
            Error::Transport(why) => write!(f, "{why}"),
            Error::Rpc { code, message } => write!(f, "server error {code}: {message}"),
            Error::Protocol(why) => write!(f, "protocol error: {why}"),
        }
    }
}

/// One way of exchanging JSON-RPC messages with a server.
pub trait Transport: Send {
    /// Sends one message. For a request (it has an `id`), returns the messages received up to and
    /// including the response; a notification returns an empty list.
    fn send(&mut self, message: &Value, timeout: Duration) -> Result<Vec<Value>, Error>;
    /// Whether the connection can still be used (the child runs, the session is open).
    fn alive(&mut self) -> bool;
    /// Ends the connection: kills the child, or forgets the session.
    fn close(&mut self);
}

/// What `initialize` learned about the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol: String,
}

/// One tool a server offers, as its listing describes it. Its schema is content (kept as the server sent it); its
/// annotations are the MCP hints the hub reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    #[table(content)]
    pub input_schema: Value,
    #[table(flatten)]
    pub annotations: ToolAnnotations,
}

/// The MCP tool annotations the hub reads (the hints of the MCP spec, camelCase on the wire). A hint a server does
/// not send is absent: the risk then comes from the others.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

/// A client over one transport. Requests are answered in order; the client is used by one caller at a time.
pub struct Client {
    transport: Box<dyn Transport>,
    next_id: u64,
    pub protocol: String,
}

impl Client {
    pub fn new(transport: Box<dyn Transport>) -> Client {
        Client {
            transport,
            next_id: 1,
            protocol: String::new(),
        }
    }

    /// Whether the transport can still be used.
    pub fn alive(&mut self) -> bool {
        self.transport.alive()
    }

    /// Ends the connection.
    pub fn close(&mut self) {
        self.transport.close();
    }

    /// Sends a request and returns its `result`.
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let replies = self.transport.send(&message, timeout)?;
        for reply in replies {
            if reply.get("id") != Some(&json!(id)) || reply.get("method").is_some() {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(Error::Rpc {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(Error::Protocol(format!("no reply to {method}")))
    }

    /// Sends a notification (no answer is expected).
    pub fn notify(&mut self, method: &str, params: Value, timeout: Duration) -> Result<(), Error> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.transport.send(&message, timeout).map(|_| ())
    }

    /// The handshake: `initialize`, then `notifications/initialized`.
    pub fn initialize(
        &mut self,
        client_name: &str,
        client_version: &str,
        timeout: Duration,
    ) -> Result<ServerInfo, Error> {
        let mut last = Error::Protocol("no protocol version agreed".into());
        for wanted in PROTOCOLS {
            let params = json!({
                "protocolVersion": wanted,
                "capabilities": {},
                "clientInfo": {"name": client_name, "version": client_version},
            });
            let result = self.request("initialize", params, timeout)?;
            let agreed = result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !PROTOCOLS.contains(&agreed.as_str()) {
                last = Error::Protocol(format!("server offers protocol `{agreed}`"));
                continue;
            }
            let info = ServerInfo {
                name: result
                    .pointer("/serverInfo/name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                version: result
                    .pointer("/serverInfo/version")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                protocol: agreed.clone(),
            };
            self.protocol = agreed;
            self.notify("notifications/initialized", json!({}), timeout)?;
            return Ok(info);
        }
        Err(last)
    }

    /// Every item of a paged listing (`key` names the array in the result).
    fn list_all(
        &mut self,
        method: &str,
        key: &str,
        timeout: Duration,
    ) -> Result<Vec<Value>, Error> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let page = self.request(method, params, timeout)?;
            if let Some(list) = page.get(key).and_then(Value::as_array) {
                items.extend(list.iter().cloned());
            }
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                _ => return Ok(items),
            }
        }
        Err(Error::Protocol(format!(
            "{method} went on past {MAX_PAGES} pages"
        )))
    }

    /// Every tool the server offers.
    pub fn list_tools(&mut self, timeout: Duration) -> Result<Vec<ToolInfo>, Error> {
        let mut tools = Vec::new();
        for item in self.list_all("tools/list", "tools", timeout)? {
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                return Err(Error::Protocol("a tool has no name".into()));
            };
            tools.push(ToolInfo {
                name: name.to_string(),
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                input_schema: item
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"})),
                annotations: item
                    .get("annotations")
                    .and_then(|a| serde_json::from_value(a.clone()).ok())
                    .unwrap_or_default(),
            });
        }
        Ok(tools)
    }

    /// The resources the server lists (for the console; agents are not offered them in v1).
    pub fn list_resources(&mut self, timeout: Duration) -> Result<Vec<Value>, Error> {
        self.list_all("resources/list", "resources", timeout)
    }

    /// The prompts the server lists (for the console; agents are not offered them in v1).
    pub fn list_prompts(&mut self, timeout: Duration) -> Result<Vec<Value>, Error> {
        self.list_all("prompts/list", "prompts", timeout)
    }

    /// Calls a tool. The result is the server's `{content, isError}`, unread.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, Error> {
        self.request(
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            timeout,
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A scripted transport: each request gets the next reply, built from the request's id.
    pub struct Script {
        pub sent: Arc<Mutex<Vec<Value>>>,
        replies: VecDeque<Box<dyn Fn(&Value) -> Vec<Value> + Send>>,
    }

    impl Script {
        pub fn new(
            replies: Vec<Box<dyn Fn(&Value) -> Vec<Value> + Send>>,
        ) -> (Script, Arc<Mutex<Vec<Value>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            (
                Script {
                    sent: Arc::clone(&sent),
                    replies: replies.into(),
                },
                sent,
            )
        }
    }

    impl Transport for Script {
        fn send(&mut self, message: &Value, _timeout: Duration) -> Result<Vec<Value>, Error> {
            self.sent.lock().unwrap().push(message.clone());
            if message.get("id").is_none() {
                return Ok(Vec::new());
            }
            match self.replies.pop_front() {
                Some(reply) => Ok(reply(message)),
                None => Err(Error::Timeout),
            }
        }
        fn alive(&mut self) -> bool {
            true
        }
        fn close(&mut self) {}
    }

    pub fn ok(result: Value) -> Box<dyn Fn(&Value) -> Vec<Value> + Send> {
        Box::new(move |req: &Value| {
            vec![json!({"jsonrpc": "2.0", "id": req["id"], "result": result})]
        })
    }

    fn timeout() -> Duration {
        Duration::from_secs(5)
    }

    /// The client agrees a protocol version with the server and sends the initialized note; a server
    /// that answers with an older protocol is agreed to that protocol.
    #[test]
    fn initialize_negotiates_the_protocol_version() {
        // The server's protocol is agreed, and the initialized notification is sent without an id.
        {
            let (script, sent) = Script::new(vec![ok(
                json!({"protocolVersion": "2025-06-18", "serverInfo": {"name": "fx", "version": "1"}}),
            )]);
            let mut client = Client::new(Box::new(script));
            let info = client.initialize("agentplaza", "0", timeout()).unwrap();
            assert_eq!(
                (info.name.as_str(), info.protocol.as_str()),
                ("fx", "2025-06-18")
            );
            let sent = sent.lock().unwrap();
            assert_eq!(sent[0]["method"], "initialize");
            assert_eq!(sent[1]["method"], "notifications/initialized");
            assert!(sent[1].get("id").is_none());
        }
        // An older protocol from the server is agreed, and the client asks for it in the next request.
        {
            let (script, sent) = Script::new(vec![
                ok(json!({"protocolVersion": "2024-11-05", "serverInfo": {}})),
                ok(json!({"protocolVersion": "2025-03-26", "serverInfo": {"name": "old"}})),
            ]);
            let mut client = Client::new(Box::new(script));
            let info = client.initialize("agentplaza", "0", timeout()).unwrap();
            assert_eq!(info.protocol, "2025-03-26");
            assert_eq!(
                sent.lock().unwrap()[1]["params"]["protocolVersion"],
                "2025-03-26"
            );
        }
    }

    #[test]
    fn tools_are_listed_across_pages() {
        let page_one =
            ok(json!({"tools": [{"name": "a", "description": "first"}], "nextCursor": "1"}));
        let page_two = ok(json!({"tools": [{"name": "b", "annotations": {"readOnlyHint": true}}]}));
        let (script, sent) = Script::new(vec![page_one, page_two]);
        let mut client = Client::new(Box::new(script));
        let tools = client.list_tools(timeout()).unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(tools[1].annotations.read_only_hint, Some(true));
        assert_eq!(sent.lock().unwrap()[1]["params"]["cursor"], "1");
    }

    /// A tool call returns its reply: a JSON-RPC error is reported with its code, notifications and
    /// replies with other ids before the reply are skipped, and a silent server is a timeout.
    #[test]
    fn call_tool_reports_errors_skips_notifications_and_times_out() {
        // A JSON-RPC error is reported with its code and message.
        {
            let reply: Box<dyn Fn(&Value) -> Vec<Value> + Send> = Box::new(|req| {
                vec![
                    json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32602, "message": "no such tool"}}),
                ]
            });
            let (script, _) = Script::new(vec![reply]);
            let mut client = Client::new(Box::new(script));
            assert_eq!(
                client.call_tool("nope", json!({}), timeout()),
                Err(Error::Rpc {
                    code: -32602,
                    message: "no such tool".into()
                })
            );
        }
        // Server notifications before the reply are skipped, and so is a reply with another id.
        {
            let reply: Box<dyn Fn(&Value) -> Vec<Value> + Send> = Box::new(|req| {
                vec![
                    json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {}}),
                    json!({"jsonrpc": "2.0", "id": 99, "result": {"wrong": true}}),
                    json!({"jsonrpc": "2.0", "id": req["id"], "result": {"content": [], "isError": false}}),
                ]
            });
            let (script, _) = Script::new(vec![reply]);
            let mut client = Client::new(Box::new(script));
            let result = client.call_tool("echo", json!({}), timeout()).unwrap();
            assert_eq!(result["isError"], false);
        }
        // A silent server is a timeout.
        {
            let (script, _) = Script::new(vec![]);
            let mut client = Client::new(Box::new(script));
            assert_eq!(
                client.call_tool("slow", json!({}), timeout()),
                Err(Error::Timeout)
            );
        }
    }
}
