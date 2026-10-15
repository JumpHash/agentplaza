//! Http servers: the Streamable HTTP transport (docs/design/mcp.md, "Running a server").
//!
//! Each message is a POST of JSON-RPC with `Accept: application/json, text/event-stream`. The reply is a
//! JSON body or an event stream; the stream is read only up to the reply to the request. The session id
//! the server hands out in `Mcp-Session-Id` is sent back on every request to the same origin. A 404 on a
//! session request means the session expired: the client starts a new one.
//!
//! Egress: ureq's own redirect following is off. The transport follows at most [`MAX_REDIRECTS`] hops
//! itself, and every hop runs the full check ([`check_endpoint`]: blocklist, private and metadata
//! refusal unless `allow_private`, https unless `allow_private`). Connections go through
//! [`CheckedResolver`], which refuses a host whose resolved addresses include a refused one, so the
//! address that is connected is the one that was checked, even if DNS changes between checks.
//! Configured headers (credentials) and the session id are sent only to the endpoint's own origin,
//! never to a redirect target on another origin.

use std::io::{BufRead, BufReader, Read};
use std::net::ToSocketAddrs;
use std::time::Duration;

use serde_json::Value;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use super::client::{Error, PROTOCOLS, Transport};
use super::runtime::redact;
use super::stdio::read_line_capped;

/// The most redirects one request may follow.
pub const MAX_REDIRECTS: usize = 3;
/// The most a reply may be before it is refused (the tool output cap is applied later, per server).
const MAX_BODY: usize = 8 << 20;
/// The line cap for an event stream.
const MAX_LINE: usize = 1 << 20;

/// What the egress rules allow for one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub allow_private: bool,
    pub blocklist: Vec<String>,
}

/// The host part of a URL, without brackets.
fn host_of(uri: &Uri) -> String {
    uri.host()
        .unwrap_or("")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

/// Why an address is refused for this policy, if it is.
fn refused_address(policy: &Policy, ip: std::net::IpAddr) -> Option<&'static str> {
    if policy.allow_private {
        return None;
    }
    super::egress::refusal(ip)
}

/// The resolver every connection uses. It resolves the host the same way ureq would, then refuses the host
/// if any address it would connect to is refused. The addresses it returns are the ones connected to.
#[derive(Debug)]
pub struct CheckedResolver {
    policy: Policy,
}

impl Resolver for CheckedResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let host = host_of(uri);
        if crate::internal::on_fetch_blocklist(&self.policy.blocklist, &host) {
            return Err(ureq::Error::Io(std::io::Error::other(format!(
                "refused: {host} is on the fetch blocklist"
            ))));
        }
        let addresses = DefaultResolver::default().resolve(uri, config, timeout)?;
        if let Some(address) = addresses
            .iter()
            .find(|a| refused_address(&self.policy, a.ip()).is_some())
        {
            let why = refused_address(&self.policy, address.ip()).unwrap_or("private");
            return Err(ureq::Error::Io(std::io::Error::other(format!(
                "refused: {host} resolves to a {why} address ({}); set allow_private to reach it",
                address.ip()
            ))));
        }
        Ok(addresses)
    }
}

/// The agent a request is sent with: no redirects of its own, and the checked resolver.
fn agent_for(policy: &Policy, timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build();
    ureq::Agent::with_parts(
        config,
        DefaultConnector::default(),
        CheckedResolver {
            policy: policy.clone(),
        },
    )
}

/// One HTTP answer, as the transport reads it.
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub session: Option<String>,
    pub location: Option<String>,
    pub body: Box<dyn BufRead + Send>,
}

/// Sends one POST. The production exchange is ureq; tests script replies.
pub trait Exchange: Send {
    fn send(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
        timeout: Duration,
        policy: &Policy,
    ) -> Result<Reply, Error>;
}

/// The production exchange: a ureq request through [`agent_for`].
pub struct UreqExchange;

impl Exchange for UreqExchange {
    fn send(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
        timeout: Duration,
        policy: &Policy,
    ) -> Result<Reply, Error> {
        let mut builder = ureq::http::Request::builder().method("POST").uri(url);
        for (name, value) in headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let request = builder
            .body(body.to_vec())
            .map_err(|e| Error::Transport(format!("bad request: {e}")))?;
        let response = agent_for(policy, timeout).run(request).map_err(|e| {
            let text = e.to_string();
            if text.to_ascii_lowercase().contains("timed out") {
                Error::Timeout
            } else {
                Error::Transport(text)
            }
        })?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        Ok(Reply {
            status: response.status().as_u16(),
            content_type: header("content-type").unwrap_or_default(),
            session: header("mcp-session-id"),
            location: header("location"),
            body: Box::new(BufReader::new(response.into_body().into_reader())),
        })
    }
}

/// Checks an endpoint against the egress rules: the fetch blocklist always, https unless `allow_private`
/// (http is allowed only with it), and private or loopback addresses unless `allow_private`. The host is
/// resolved, so a name that points at a private address is refused too.
pub fn check_endpoint(url: &str, allow_private: bool, blocklist: &[String]) -> Result<(), String> {
    let uri: Uri = url.parse().map_err(|e| format!("bad URL: {e}"))?;
    let host = host_of(&uri);
    if crate::internal::on_fetch_blocklist(blocklist, &host) {
        return Err(format!("refused: {host} is on the fetch blocklist"));
    }
    if uri.scheme_str() == Some("http") && !allow_private {
        return Err(format!(
            "refused: {host} is reached over http; https is required unless allow_private is set"
        ));
    }
    if allow_private {
        return Ok(());
    }
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") {
            80
        } else {
            443
        });
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match super::egress::refusal(ip) {
            Some(why) => Err(format!(
                "refused: {host} is a {why} address; set allow_private to reach it"
            )),
            None => Ok(()),
        };
    }
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| format!("cannot resolve {host}"))?;
    for address in addresses {
        if let Some(why) = super::egress::refusal(address.ip()) {
            return Err(format!(
                "refused: {host} resolves to a {why} address; set allow_private to reach it"
            ));
        }
    }
    Ok(())
}

/// The scheme and authority of a URL: `https://host:port`.
fn origin_of(url: &str) -> &str {
    match url.find("://") {
        Some(scheme) => {
            let rest = &url[scheme + 3..];
            let end = rest.find('/').unwrap_or(rest.len());
            &url[..scheme + 3 + end]
        }
        None => url,
    }
}

/// Where a redirect points: an absolute http(s) URL, or a path on the same origin. Anything else is refused.
fn next_url(current: &str, location: &str) -> Result<String, String> {
    if location.starts_with("https://") || location.starts_with("http://") {
        Ok(location.to_string())
    } else if location.starts_with('/') && !location.starts_with("//") {
        Ok(format!("{}{location}", origin_of(current)))
    } else {
        Err(format!(
            "refused: a redirect to `{location}` is not an absolute URL or a path"
        ))
    }
}

/// The http transport for one endpoint.
pub struct HttpTransport {
    /// The endpoint. It moves to a redirect's target once a request there succeeds.
    url: String,
    /// Header name and value, resolved from the vault (the values are never logged).
    headers: Vec<(String, String)>,
    /// The secret values, for scrubbing error text.
    secrets: Vec<String>,
    session: Option<String>,
    protocol: Option<String>,
    policy: Policy,
    exchange: Box<dyn Exchange>,
}

impl HttpTransport {
    pub fn new(
        url: String,
        headers: Vec<(String, String)>,
        secrets: Vec<String>,
        policy: Policy,
    ) -> HttpTransport {
        HttpTransport::with_exchange(url, headers, secrets, policy, Box::new(UreqExchange))
    }

    pub fn with_exchange(
        url: String,
        headers: Vec<(String, String)>,
        secrets: Vec<String>,
        policy: Policy,
        exchange: Box<dyn Exchange>,
    ) -> HttpTransport {
        HttpTransport {
            url,
            headers,
            secrets,
            session: None,
            protocol: None,
            policy,
            exchange,
        }
    }

    fn scrub(&self, text: &str) -> String {
        redact(text, &self.secrets)
    }

    /// The headers for a request to `url`: the protocol header always, and the configured headers and the
    /// session id only when `url` is on the endpoint's own origin.
    fn headers_for(&self, same_origin: bool) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Accept".to_string(),
                "application/json, text/event-stream".to_string(),
            ),
        ];
        if let Some(protocol) = &self.protocol {
            headers.push(("MCP-Protocol-Version".to_string(), protocol.clone()));
        }
        if same_origin {
            headers.extend(self.headers.iter().cloned());
            if let Some(session) = &self.session {
                headers.push(("Mcp-Session-Id".to_string(), session.clone()));
            }
        }
        headers
    }
}

/// Whether `value` is the reply to request `id`.
fn is_reply(value: &Value, id: &Value) -> bool {
    value.get("id") == Some(id)
        && value.get("method").is_none()
        && (value.get("result").is_some() || value.get("error").is_some())
}

impl Transport for HttpTransport {
    fn send(&mut self, message: &Value, timeout: Duration) -> Result<Vec<Value>, Error> {
        let body = serde_json::to_vec(message).map_err(|e| Error::Protocol(e.to_string()))?;
        let id = message.get("id").cloned();
        let endpoint = self.url.clone();
        let mut current = endpoint.clone();
        let mut hops = 0;
        let mut reply = loop {
            if let Err(why) =
                check_endpoint(&current, self.policy.allow_private, &self.policy.blocklist)
            {
                return Err(Error::Transport(self.scrub(&why)));
            }
            let headers = self.headers_for(origin_of(&current) == origin_of(&endpoint));
            let answer = match self
                .exchange
                .send(&current, &headers, &body, timeout, &self.policy)
            {
                Ok(answer) => answer,
                Err(Error::Transport(text)) => return Err(Error::Transport(self.scrub(&text))),
                Err(other) => return Err(other),
            };
            if !(300..400).contains(&answer.status) {
                break answer;
            }
            if hops == MAX_REDIRECTS {
                return Err(Error::Transport(format!(
                    "refused: more than {MAX_REDIRECTS} redirects"
                )));
            }
            let Some(location) = answer.location else {
                return Err(Error::Transport(format!(
                    "HTTP {}: a redirect with no Location",
                    answer.status
                )));
            };
            current =
                next_url(&current, &location).map_err(|why| Error::Transport(self.scrub(&why)))?;
            hops += 1;
        };
        let status = reply.status;
        if status == 404 && self.session.is_some() {
            self.session = None;
            return Err(Error::SessionLost);
        }
        if !(200..300).contains(&status) {
            let mut snippet = Vec::new();
            let _ = reply.body.by_ref().take(512).read_to_end(&mut snippet);
            let text = String::from_utf8_lossy(&snippet).into_owned();
            return Err(Error::Transport(
                self.scrub(&format!("HTTP {status}: {}", text.trim())),
            ));
        }
        if let Some(session) = reply.session.take() {
            self.session = Some(session);
        }
        if hops > 0 {
            // The endpoint moved: later requests go to the target, which the checks above just accepted.
            self.url = current;
        }
        let Some(id) = id else {
            // A notification: the server answers 202 with no body.
            return Ok(Vec::new());
        };
        let messages = if reply.content_type.starts_with("text/event-stream") {
            read_events(reply.body, &id)?
        } else if reply.content_type.starts_with("application/json") {
            let mut bytes = Vec::new();
            reply
                .body
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| Error::Transport(e.to_string()))?;
            if bytes.len() > MAX_BODY {
                return Err(Error::Protocol("a reply over 8 MiB".into()));
            }
            match serde_json::from_slice::<Value>(&bytes)
                .map_err(|e| Error::Protocol(e.to_string()))?
            {
                Value::Array(items) => items,
                single => vec![single],
            }
        } else {
            return Err(Error::Protocol(format!(
                "unexpected content type `{}`",
                reply.content_type
            )));
        };
        if message.get("method").and_then(Value::as_str) == Some("initialize") {
            if let Some(version) = messages
                .iter()
                .find(|m| is_reply(m, &id))
                .and_then(|m| m.pointer("/result/protocolVersion"))
                .and_then(Value::as_str)
            {
                if PROTOCOLS.contains(&version) {
                    self.protocol = Some(version.to_string());
                }
            }
        }
        Ok(messages)
    }

    fn alive(&mut self) -> bool {
        true
    }

    fn close(&mut self) {
        // The session is simply forgotten; a Streamable HTTP server expires it on its own.
        self.session = None;
    }
}

/// Reads the events of an event stream until the reply to `id` (the stream may stay open after it).
fn read_events(mut reader: impl BufRead, id: &Value) -> Result<Vec<Value>, Error> {
    let mut messages = Vec::new();
    let mut data = String::new();
    loop {
        let line =
            read_line_capped(&mut reader, MAX_LINE).map_err(|e| Error::Transport(e.to_string()))?;
        let finished = line.is_none();
        let line = line.unwrap_or_default();
        if line.is_empty() || finished {
            if !data.is_empty() {
                let value: Value =
                    serde_json::from_str(&data).map_err(|e| Error::Protocol(e.to_string()))?;
                let done = is_reply(&value, id);
                messages.push(value);
                if done {
                    return Ok(messages);
                }
                data.clear();
            }
            if finished {
                return Ok(messages);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// One scripted answer: a status, a Location for a redirect, and a body.
    pub struct Answer {
        pub status: u16,
        pub location: Option<&'static str>,
        pub body: &'static str,
        pub content_type: &'static str,
    }

    /// A scripted exchange. Records every URL and header set it is asked to send.
    pub struct Scripted {
        pub answers: VecDeque<Answer>,
        pub sent: Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>,
    }

    impl Exchange for Scripted {
        fn send(
            &mut self,
            url: &str,
            headers: &[(String, String)],
            _body: &[u8],
            _timeout: Duration,
            _policy: &Policy,
        ) -> Result<Reply, Error> {
            self.sent
                .lock()
                .unwrap()
                .push((url.to_string(), headers.to_vec()));
            let answer = self.answers.pop_front().expect("an unscripted request");
            Ok(Reply {
                status: answer.status,
                content_type: answer.content_type.to_string(),
                session: None,
                location: answer.location.map(str::to_string),
                body: Box::new(std::io::Cursor::new(answer.body.as_bytes().to_vec())),
            })
        }
    }

    fn policy(allow_private: bool, blocklist: &[&str]) -> Policy {
        Policy {
            allow_private,
            blocklist: blocklist.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn ok_json(id: u64) -> Answer {
        Answer {
            status: 200,
            location: None,
            body: if id == 1 {
                r#"{"jsonrpc":"2.0","id":1,"result":{}}"#
            } else {
                r#"{"jsonrpc":"2.0","id":2,"result":{}}"#
            },
            content_type: "application/json",
        }
    }

    fn redirect(to: &'static str) -> Answer {
        Answer {
            status: 307,
            location: Some(to),
            body: "",
            content_type: "text/plain",
        }
    }

    fn secured(
        answers: Vec<Answer>,
        policy: Policy,
    ) -> (
        HttpTransport,
        Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>,
    ) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let exchange = Scripted {
            answers: answers.into(),
            sent: Arc::clone(&sent),
        };
        let headers = vec![("Authorization".to_string(), "Bearer SECRET-XYZ".to_string())];
        let transport = HttpTransport::with_exchange(
            "https://8.8.8.8/mcp".into(),
            headers,
            vec!["SECRET-XYZ".into()],
            policy,
            Box::new(exchange),
        );
        (transport, sent)
    }

    fn request(id: u64) -> Value {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}})
    }

    #[test]
    fn private_and_blocklisted_endpoints_are_refused() {
        let evil = vec!["evil.example".to_string()];
        let none: Vec<String> = Vec::new();
        for (case, url, allow_private, blocklist, want_ok) in [
            (
                "loopback is refused unless private is allowed",
                "http://127.0.0.1:9000/mcp",
                false,
                &none,
                false,
            ),
            (
                "loopback is allowed once private is allowed",
                "http://127.0.0.1:9000/mcp",
                true,
                &none,
                true,
            ),
            (
                "a private address is refused unless private is allowed",
                "https://10.1.2.3/mcp",
                false,
                &none,
                false,
            ),
            (
                "IPv6 loopback is refused",
                "https://[::1]/mcp",
                false,
                &none,
                false,
            ),
            (
                "localhost is refused",
                "https://localhost/mcp",
                false,
                &none,
                false,
            ),
            (
                "the fetch blocklist applies to a subdomain even when private is allowed",
                "https://mcp.evil.example/mcp",
                true,
                &evil,
                false,
            ),
            (
                "the fetch blocklist applies to the host even when private is allowed",
                "https://evil.example/mcp",
                true,
                &evil,
                false,
            ),
        ] {
            assert_eq!(
                check_endpoint(url, allow_private, blocklist).is_ok(),
                want_ok,
                "{case}"
            );
        }
    }

    #[test]
    fn event_streams_are_parsed_into_jsonrpc_messages() {
        // An event stream is read up to the reply and no further.
        {
            let stream = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n\
                          event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n\
                          data: {\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{}}\n\n";
            let id = serde_json::json!(7);
            let messages =
                read_events(std::io::Cursor::new(stream.as_bytes().to_vec()), &id).unwrap();
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[1]["result"]["ok"], true);
        }
        // A multi-line event is joined before it is parsed.
        {
            let stream = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":3,\"result\":{}}\n\n";
            let messages = read_events(
                std::io::Cursor::new(stream.as_bytes().to_vec()),
                &serde_json::json!(3),
            )
            .unwrap();
            assert_eq!(messages[0]["id"], 3);
        }
    }

    #[test]
    fn redirects_are_followed_only_within_policy() {
        // A redirect to loopback is refused before anything is sent there.
        {
            let (mut transport, sent) =
                secured(vec![redirect("http://127.0.0.1:9/mcp")], policy(false, &[]));
            let err = transport
                .send(&request(2), Duration::from_secs(5))
                .unwrap_err();
            assert!(err.to_string().contains("refused"), "{err}");
            assert_eq!(
                sent.lock().unwrap().len(),
                1,
                "only the origin was asked; the loopback target was never sent to"
            );
        }
        // A redirect to a blocklisted host is refused.
        {
            let (mut transport, sent) = secured(
                vec![redirect("https://evil.example/mcp")],
                policy(false, &["evil.example"]),
            );
            let err = transport
                .send(&request(2), Duration::from_secs(5))
                .unwrap_err();
            assert!(err.to_string().contains("blocklist"), "{err}");
            assert_eq!(sent.lock().unwrap().len(), 1);
        }
        // An allowed hop works and the endpoint moves to it.
        {
            let (mut transport, sent) = secured(
                vec![redirect("https://1.1.1.1/mcp"), ok_json(2), ok_json(2)],
                policy(false, &[]),
            );
            let messages = transport.send(&request(2), Duration::from_secs(5)).unwrap();
            assert_eq!(messages[0]["id"], 2);
            transport.send(&request(2), Duration::from_secs(5)).unwrap();
            let urls: Vec<String> = sent
                .lock()
                .unwrap()
                .iter()
                .map(|(u, _)| u.clone())
                .collect();
            assert_eq!(
                urls,
                [
                    "https://8.8.8.8/mcp",
                    "https://1.1.1.1/mcp",
                    "https://1.1.1.1/mcp"
                ]
            );
        }
        // More than three redirects are refused; the fourth hop is never sent.
        {
            let answers = vec![
                redirect("https://1.1.1.1/a"),
                redirect("https://1.1.1.1/b"),
                redirect("https://1.1.1.1/c"),
                redirect("https://1.1.1.1/d"),
            ];
            let (mut transport, sent) = secured(answers, policy(false, &[]));
            let err = transport
                .send(&request(2), Duration::from_secs(5))
                .unwrap_err();
            assert!(err.to_string().contains("more than 3 redirects"), "{err}");
            assert_eq!(
                sent.lock().unwrap().len(),
                4,
                "the fourth hop is refused before it is sent"
            );
        }
        // A relative or scheme-changing redirect is refused; next_url resolves relative paths.
        {
            let (mut transport, _) =
                secured(vec![redirect("ftp://1.1.1.1/mcp")], policy(false, &[]));
            assert!(transport.send(&request(2), Duration::from_secs(5)).is_err());
            assert_eq!(
                next_url("https://8.8.8.8/mcp", "/other").unwrap(),
                "https://8.8.8.8/other"
            );
            assert!(next_url("https://8.8.8.8/mcp", "other").is_err());
        }
    }

    #[test]
    fn the_configured_headers_stay_on_the_origin() {
        let (mut transport, sent) = secured(
            vec![redirect("https://1.1.1.1/mcp"), ok_json(2)],
            policy(false, &[]),
        );
        transport.send(&request(2), Duration::from_secs(5)).unwrap();
        let log = sent.lock().unwrap();
        assert!(
            log[0]
                .1
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer SECRET-XYZ")
        );
        assert!(
            !log[1].1.iter().any(|(k, _)| k == "Authorization"),
            "a credential is never sent to another origin"
        );
    }

    #[test]
    fn the_connection_resolver_refuses_a_loopback_address_it_would_connect_to() {
        let agent = agent_for(&policy(false, &[]), Duration::from_secs(5));
        let err = agent.get("http://127.0.0.1:9/mcp").call().unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
    }
}
