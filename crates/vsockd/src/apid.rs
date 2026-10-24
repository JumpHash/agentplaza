//! The hub's client for `apid`'s internal surface (`docs/architecture.md`,
//! "Internal endpoints"): the hello that fetches an agent's profile, the
//! forward of each v3 guest line, and the `gone` at session end. Loopback
//! only. Every request carries the internal token. The token is read from its
//! file on first use and kept, shared by every clone of the `Apid`. When apid
//! refuses the kept token (403, which it answers before it runs a handler), the
//! file is read again and, if it holds another token, the request is sent once
//! more with that one. So an apid that restarted with a new token is picked up
//! on its next refusal, and the file is not read on every call.

use std::fmt;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_http::client::{Reply, send_with};
use apz_proto::wire::{
    AgentProfile, BatchDecision, Decision, GuestMessage, HostMessage, PolicySnapshot,
};
use serde_json::json;

/// The header that carries the internal token.
pub const TOKEN_HEADER: &str = "x-apz-internal";
/// apid's internal surface when `--apid` is not given.
pub const DEFAULT_APID: &str = "http://127.0.0.1:7700";
/// The hello runs on the session thread, so it is kept short.
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
/// A forward may wait out a whole model call.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// The session-end notice is best effort.
const GONE_TIMEOUT: Duration = Duration::from_secs(2);
/// An egress check is made before a connection is dialled, so it is kept short.
const EGRESS_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
/// A leaf certificate is fetched once per host and kept, so this is rarely waited on.
const EGRESS_CERT_TIMEOUT: Duration = Duration::from_secs(5);
/// An injection is asked per intercepted request, and the request waits for it.
const EGRESS_INJECT_TIMEOUT: Duration = Duration::from_secs(5);
/// An E2B request waits on the sandbox lookup before it can be relayed, so it is kept short.
const E2B_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// apid's decision on one egress connection: allowed, or refused with the reason the guest is told.
/// `intercept` says a credential rule could match this agent and host, so the proxy must terminate
/// the guest's TLS and ask per request (see `egress_tls`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressAnswer {
    pub allow: bool,
    pub reason: String,
    pub intercept: bool,
    /// Blocks (address and prefix length) that no resolved address of the connection may lie in: a
    /// sandbox's denyOut. Empty otherwise.
    pub deny_cidrs: Vec<(IpAddr, u8)>,
}

/// The leaf certificate and key apid signed for one host. Holds a private key, so it has no `Debug`
/// and nothing logs it.
pub struct EgressLeaf {
    pub cert_pem: String,
    pub key_pem: String,
}

/// The header apid says to add to one intercepted request. The value is a secret: no `Debug`, and
/// nothing logs it.
pub struct Injection {
    pub header: String,
    pub value: String,
}

/// A plaza sandbox as apid names it to the E2B gateway: the guest CID its VM is served at, and the
/// access token its requests must carry (empty when the sandbox is not secured).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2bSandbox {
    pub cid: u32,
    pub access_token: String,
}

/// Where apid's internal surface is, and the file that holds its token.
/// Without a token file, forwarding is off and nothing is sent. The token is
/// kept once read, and every clone of this value shares it.
#[derive(Debug, Clone)]
pub struct Apid {
    port: u16,
    token_file: Option<PathBuf>,
    token: Arc<TokenCache>,
}

/// The token last read from the file, `None` before the first read. Its `Debug` never shows it.
#[derive(Default)]
struct TokenCache(Mutex<Option<String>>);

impl fmt::Debug for TokenCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenCache(..)")
    }
}

impl Apid {
    /// Forwarding off (the daemon's own is `new` without a token file).
    #[cfg(test)]
    pub fn off() -> Self {
        Self {
            port: 0,
            token_file: None,
            token: Arc::default(),
        }
    }

    /// Forwarding to `url` (`http://127.0.0.1:PORT`) with the token in `token_file`.
    pub fn new(url: &str, token_file: Option<PathBuf>) -> Result<Self, String> {
        Ok(Self {
            port: port_of(url)?,
            token_file,
            token: Arc::default(),
        })
    }

    /// Whether v3 lines and hellos go to apid at all.
    pub fn enabled(&self) -> bool {
        self.token_file.is_some()
    }

    /// The agent's profile from apid (`None` when apid has none for it), and the policy its session starts with.
    /// `caps` are the harness's features from its hello; apid keeps them on the agent.
    pub fn hello(
        &self,
        cid: u32,
        role: &str,
        vm: u32,
        caps: &[String],
    ) -> Result<HelloReply, String> {
        self.hello_as(cid, role, vm, caps, None)
    }

    /// [`Apid::hello`] for a session of `kind` (a base build's VM is `base`, see `KIND_BASE`): apid registers no agent
    /// for it and answers with no profile.
    pub fn hello_as(
        &self,
        cid: u32,
        role: &str,
        vm: u32,
        caps: &[String],
        kind: Option<&str>,
    ) -> Result<HelloReply, String> {
        let body = hello_body(role, vm, caps, kind);
        let reply = self.post(
            &format!("/internal/v1/agents/{cid}/hello"),
            &body,
            HELLO_TIMEOUT,
        )?;
        let profile: Option<AgentProfile> =
            serde_json::from_value(reply.get("profile").cloned().unwrap_or_default())
                .map_err(|err| format!("apid's profile is malformed: {err}"))?;
        let policy: Option<PolicySnapshot> =
            serde_json::from_value(reply.get("policy").cloned().unwrap_or_default())
                .map_err(|err| format!("apid's policy is malformed: {err}"))?;
        Ok(HelloReply { profile, policy })
    }

    /// Forwards one guest line (`line`, the message's JSON) and returns apid's reply, if any.
    pub fn message(&self, cid: u32, line: &str) -> Result<Option<HostMessage>, String> {
        let reply = self.post(
            &format!("/internal/v1/agents/{cid}/message"),
            line,
            FORWARD_TIMEOUT,
        )?;
        serde_json::from_value(reply.get("reply").cloned().unwrap_or_default())
            .map_err(|err| format!("apid's reply is not a host message: {err}"))
    }

    /// Tells apid the agent's session ended and why.
    pub fn gone(&self, cid: u32, reason: &str) -> Result<(), String> {
        let body = json!({ "reason": reason }).to_string();
        self.post(
            &format!("/internal/v1/agents/{cid}/gone"),
            &body,
            GONE_TIMEOUT,
        )
        .map(drop)
    }

    /// Sends one egress audit record (`record`, its JSON) for the agent at `cid`.
    pub fn egress(&self, cid: u32, record: &str) -> Result<(), String> {
        self.post(
            &format!("/internal/v1/agents/{cid}/egress"),
            record,
            Duration::from_secs(2),
        )
        .map(drop)
    }

    /// Asks apid whether the agent at `cid` may connect to `host:port`: its fetch blocklist and its
    /// laws decide. Every failure is an `Err`, and the caller refuses.
    pub fn check_egress(&self, cid: u32, host: &str, port: u16) -> Result<EgressAnswer, String> {
        let body = json!({ "cid": cid, "host": host, "port": port }).to_string();
        let reply = self.post("/internal/egress/check", &body, EGRESS_CHECK_TIMEOUT)?;
        egress_answer_of(&reply)
    }

    /// The leaf certificate apid signs for `host` as the agent at `cid` (`POST /internal/egress/cert`).
    /// apid answers 403 when the agent may not be intercepted for that host.
    pub fn egress_cert(&self, cid: u32, host: &str) -> Result<EgressLeaf, String> {
        let body = json!({ "cid": cid, "host": host }).to_string();
        let reply = self.post("/internal/egress/cert", &body, EGRESS_CERT_TIMEOUT)?;
        let text = |key: &str| {
            reply
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("apid's egress certificate names no {key}"))
        };
        Ok(EgressLeaf {
            cert_pem: text("cert_pem")?,
            key_pem: text("key_pem")?,
        })
    }

    /// The header to add to one request of the agent at `cid` to `host:port` (`method`, origin-form
    /// `path` without its query): `Ok(None)` when no rule matches. The path is what the rules match.
    pub fn egress_inject(
        &self,
        cid: u32,
        host: &str,
        port: u16,
        method: &str,
        path: &str,
    ) -> Result<Option<Injection>, String> {
        let body =
            json!({ "cid": cid, "host": host, "port": port, "method": method, "path": path })
                .to_string();
        let reply = self.post("/internal/egress/inject", &body, EGRESS_INJECT_TIMEOUT)?;
        injection_of(&reply)
    }

    /// Asks apid for the sandbox `id`: `Ok(None)` when apid does not know it, `Err` when apid
    /// cannot be asked or answers anything but 200 or 404. The caller validates `id` first, since
    /// it becomes part of the path.
    pub fn e2b_sandbox(&self, id: &str) -> Result<Option<E2bSandbox>, String> {
        let path = format!("/internal/e2b/sandboxes/{id}");
        let reply = self.send_authorized("GET", &path, None, E2B_LOOKUP_TIMEOUT)?;
        match reply.status {
            404 => Ok(None),
            200 => {
                let body = reply
                    .json()
                    .map_err(|err| format!("apid's sandbox answer: {err}"))?;
                let cid = body
                    .get("cid")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|cid| u32::try_from(cid).ok())
                    .ok_or("apid's sandbox answer names no cid")?;
                let access_token = body
                    .get("access_token")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Ok(Some(E2bSandbox { cid, access_token }))
            }
            other => Err(format!("apid answered {other} to the sandbox lookup")),
        }
    }

    /// A plain TCP connection to apid's internal surface, for a relay that
    /// writes its own request (the egress gateway). The caller sends the token.
    pub fn connect(&self, timeout: Duration) -> Result<TcpStream, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        TcpStream::connect_timeout(&addr, timeout).map_err(|err| format!("apid unreachable: {err}"))
    }

    /// One authenticated POST whose answer must be 200 and JSON.
    fn post(&self, path: &str, body: &str, timeout: Duration) -> Result<serde_json::Value, String> {
        let reply = self.send_authorized("POST", path, Some(body.as_bytes()), timeout)?;
        if reply.status != 200 {
            return Err(format!("apid answered {} to {path}", reply.status));
        }
        reply.json().map_err(|err| format!("apid's answer: {err}"))
    }

    /// Sends one request with the kept token and returns apid's answer, whatever its status. A 403
    /// is apid refusing the token, and the request is then sent once more with the token the file
    /// holds now, if that is a different one. A file that cannot be read leaves the 403 as it is.
    fn send_authorized(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Reply, String> {
        let token = self.token()?;
        let reply = self.send_once(method, path, &token, body, timeout)?;
        if reply.status != 403 {
            return Ok(reply);
        }
        let Ok(fresh) = self.reload_token() else {
            return Ok(reply);
        };
        if fresh == token {
            return Ok(reply);
        }
        self.send_once(method, path, &fresh, body, timeout)
    }

    fn send_once(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Reply, String> {
        send_with(
            self.port,
            method,
            path,
            &[(TOKEN_HEADER, token)],
            body,
            timeout,
        )
        .map_err(|err| format!("apid unreachable: {err}"))
    }

    /// The token, from the file on first use and kept after that. Never echoed in an error.
    pub fn token(&self) -> Result<String, String> {
        let mut cached = self.cache();
        if let Some(token) = cached.as_ref() {
            return Ok(token.clone());
        }
        let token = self.read_token_file()?;
        *cached = Some(token.clone());
        Ok(token)
    }

    /// Reads the token file again, keeps what it holds and returns it. An unreadable file is an
    /// `Err`, and the kept token stays as it was.
    pub(crate) fn reload_token(&self) -> Result<String, String> {
        let token = self.read_token_file()?;
        *self.cache() = Some(token.clone());
        Ok(token)
    }

    /// The token as the file holds it now (trimmed). Never echoed in an error.
    fn read_token_file(&self) -> Result<String, String> {
        let path = self
            .token_file
            .as_ref()
            .ok_or("forwarding is off: no --internal-token-file")?;
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("cannot read the internal token: {err}"))?;
        let token = text.trim();
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("the internal token file holds no usable token".to_string());
        }
        Ok(token.to_string())
    }

    fn cache(&self) -> MutexGuard<'_, Option<String>> {
        self.token.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The decision in apid's answer (`{allow, reason, intercept, deny_cidrs}`). A deny rule that does not
/// read as a block makes the whole answer a refusal, so a rule apid meant cannot leave its block open.
fn egress_answer_of(reply: &serde_json::Value) -> Result<EgressAnswer, String> {
    let allow = reply
        .get("allow")
        .and_then(serde_json::Value::as_bool)
        .ok_or("apid's egress answer names no allow")?;
    let reason = reply
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("refused")
        .to_string();
    // Absent means no interception: the tunnel stays opaque, as before rules existed.
    let intercept = reply
        .get("intercept")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    // Absent means no deny blocks (only sandboxes have them).
    let deny_cidrs: Option<Vec<(IpAddr, u8)>> = match reply.get("deny_cidrs") {
        None | Some(serde_json::Value::Null) => Some(Vec::new()),
        Some(serde_json::Value::Array(rules)) => rules
            .iter()
            .map(|rule| rule.as_str().and_then(cidr_of))
            .collect(),
        Some(_) => None,
    };
    Ok(match deny_cidrs {
        Some(deny_cidrs) => EgressAnswer {
            allow,
            reason,
            intercept,
            deny_cidrs,
        },
        None => EgressAnswer {
            allow: false,
            reason: "refused: apid sent a bad deny rule".to_string(),
            intercept: false,
            deny_cidrs: Vec::new(),
        },
    })
}

/// A deny rule `addr/len` (`"8.8.8.0/24"`, `"2001:db8::/32"`), or `None` when it does not read as a
/// block. An IPv4-mapped IPv6 block whose prefix is 96 bits or more is stored as the IPv4 block.
fn cidr_of(text: &str) -> Option<(IpAddr, u8)> {
    let (addr, len) = text.split_once('/')?;
    let addr: IpAddr = addr.parse().ok()?;
    let len: u8 = len.parse().ok()?;
    match addr {
        IpAddr::V4(_) => (len <= 32).then_some((addr, len)),
        IpAddr::V6(v6) => {
            if len > 128 {
                return None;
            }
            match v6.to_ipv4_mapped() {
                Some(v4) if len >= 96 => Some((IpAddr::V4(v4), len - 96)),
                _ => Some((addr, len)),
            }
        }
    }
}

/// The injection in apid's answer (`{header, value, rule}`), or `None` for the `{}` of no rule. The
/// header must be a plain token and the value free of line breaks, so a bad answer cannot split the
/// guest's request. Errors never repeat the value.
fn injection_of(reply: &serde_json::Value) -> Result<Option<Injection>, String> {
    let Some(header) = reply.get("header").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let value = reply
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or("apid's injection names no value")?;
    let plain_header = !header.is_empty()
        && header
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if !plain_header
        || value
            .bytes()
            .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
    {
        return Err("apid's injection is not a header".to_string());
    }
    Ok(Some(Injection {
        header: header.to_string(),
        value: value.to_string(),
    }))
}

/// The port of `http://127.0.0.1:PORT` (a trailing `/` is allowed).
fn port_of(url: &str) -> Result<u16, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("{url:?} is not an http:// URL"))?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| format!("{url:?} names no port"))?;
    if host != "127.0.0.1" {
        return Err(format!(
            "{url:?}: apid's internal surface is loopback only (use http://127.0.0.1:PORT)"
        ));
    }
    port.parse()
        .map_err(|err| format!("{url:?}: invalid port: {err}"))
}

/// What apid answers an agent's hello with: its profile, and the policy the session starts with.
#[derive(Debug)]
pub struct HelloReply {
    pub profile: Option<AgentProfile>,
    pub policy: Option<PolicySnapshot>,
}

/// The body of an agent's hello to apid. `caps` are added only when the harness sent some, so a
/// harness from before v4 produces the body it always did.
fn hello_body(role: &str, vm: u32, caps: &[String], kind: Option<&str>) -> String {
    let mut body = json!({ "role": role, "vm": vm });
    if !caps.is_empty() {
        body["caps"] = json!(caps);
    }
    if let Some(kind) = kind {
        body["kind"] = json!(kind);
    }
    body.to_string()
}

/// The reply that unblocks a guest whose forward failed: a model call fails
/// with `reason`, a tool request is denied with it. Lines that expect no
/// answer get none.
pub fn refusal(message: &GuestMessage, reason: &str) -> Option<HostMessage> {
    match message {
        GuestMessage::ModelCall { req, .. } => Some(HostMessage::ModelResult {
            req: *req,
            ok: false,
            response: None,
            error: Some(reason.to_string()),
            window: None,
        }),
        GuestMessage::ToolRequest { call, .. } => Some(HostMessage::ToolDecision {
            call: call.clone(),
            decision: Decision::Deny,
            output: reason.to_string(),
            is_error: true,
            program: None,
            engine: None,
            approval: None,
        }),
        GuestMessage::ToolBatch { work, calls } => Some(HostMessage::ToolBatchDecision {
            work: *work,
            decisions: calls
                .iter()
                .map(|call| BatchDecision {
                    call: call.call.clone(),
                    decision: Decision::Deny,
                    output: reason.to_string(),
                    is_error: true,
                    program: None,
                    engine: None,
                    approval: None,
                })
                .collect(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hello_body_carries_caps_only_when_the_harness_sent_some() {
        // Compared as JSON: the key order depends on serde_json's features in the build.
        let parsed = |body: String| serde_json::from_str::<serde_json::Value>(&body).unwrap();
        assert_eq!(
            parsed(hello_body("builder", 0, &[], None)),
            serde_json::json!({"role": "builder", "vm": 0})
        );
        assert_eq!(
            parsed(hello_body("builder", 0, &["acp".to_string()], None)),
            serde_json::json!({"role": "builder", "vm": 0, "caps": ["acp"]})
        );
    }

    #[test]
    fn apid_is_reached_on_loopback_only_over_plain_http() {
        assert_eq!(port_of(DEFAULT_APID), Ok(7700));
        assert_eq!(port_of("http://127.0.0.1:19090/"), Ok(19090));
        assert!(
            port_of("http://10.0.0.5:7700").is_err(),
            "a routable host is refused"
        );
        assert!(
            port_of("https://127.0.0.1:7700").is_err(),
            "plain http only"
        );
        assert!(
            port_of("http://127.0.0.1:7700/internal").is_err(),
            "the paths are fixed"
        );
        assert!(!Apid::off().enabled() && Apid::off().token().is_err());
    }

    /// A token file named for one test, so parallel tests do not share it.
    fn token_file(name: &str, token: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("vsockd-apid-{name}-{}", std::process::id()));
        std::fs::write(&path, format!("{token}\n")).unwrap();
        path
    }

    /// A fake apid that answers 200 to the token `accepts` holds and 403 to any other, and records
    /// the token each request carried, in arrival order.
    fn apid_that_accepts(
        accepts: &Arc<Mutex<String>>,
        seen: &Arc<Mutex<Vec<String>>>,
    ) -> apz_http::Handle {
        let (accepts, seen) = (Arc::clone(accepts), Arc::clone(seen));
        apz_http::Server::bind(0)
            .unwrap()
            .spawn(move |request| {
                let token = request.header(TOKEN_HEADER).unwrap_or("").to_string();
                seen.lock().unwrap().push(token.clone());
                if token == *accepts.lock().unwrap() {
                    apz_http::Response::new(200, "application/json", "{\"profile\":null}")
                } else {
                    apz_http::Response::new(403, "application/json", "{}")
                }
            })
            .unwrap()
    }

    #[test]
    fn the_token_is_read_once_and_a_refused_one_is_read_again() {
        let accepts = Arc::new(Mutex::new("tok-a".to_string()));
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let server = apid_that_accepts(&accepts, &seen);
        let path = token_file("read-once", "tok-a");
        let apid = Apid::new(
            &format!("http://127.0.0.1:{}", server.port()),
            Some(path.clone()),
        )
        .unwrap();

        // The file is read on the first call only: with it gone, the kept token is still sent.
        assert!(apid.hello(1, "builder", 0, &[]).unwrap().profile.is_none());
        std::fs::remove_file(&path).unwrap();
        assert!(apid.hello(1, "builder", 0, &[]).unwrap().profile.is_none());
        assert_eq!(*seen.lock().unwrap(), ["tok-a", "tok-a"]);

        // apid rotates its token and the file holds the new one: the old one is refused, so the
        // request goes out once more with the new one.
        *accepts.lock().unwrap() = "tok-b".to_string();
        std::fs::write(&path, "tok-b\n").unwrap();
        assert!(apid.hello(1, "builder", 0, &[]).unwrap().profile.is_none());
        assert_eq!(*seen.lock().unwrap(), ["tok-a", "tok-a", "tok-a", "tok-b"]);
        assert_eq!(apid.token().unwrap(), "tok-b");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_refusal_with_the_same_token_in_the_file_is_not_retried() {
        let accepts = Arc::new(Mutex::new("tok-never".to_string()));
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let server = apid_that_accepts(&accepts, &seen);
        let path = token_file("same-token", "tok-a");
        let apid = Apid::new(
            &format!("http://127.0.0.1:{}", server.port()),
            Some(path.clone()),
        )
        .unwrap();

        let error = apid.hello(1, "builder", 0, &[]).unwrap_err();
        assert_eq!(error, "apid answered 403 to /internal/v1/agents/1/hello");
        assert_eq!(*seen.lock().unwrap(), ["tok-a"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_deny_rule_is_an_address_block_and_a_bad_one_refuses_the_answer() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        let answer = egress_answer_of(&json!({
            "allow": true,
            "deny_cidrs": ["8.8.8.0/24", "2001:db8::/32", "::ffff:10.0.0.0/104"]
        }))
        .unwrap();
        assert!(answer.allow);
        assert_eq!(
            answer.deny_cidrs,
            vec![
                (ip("8.8.8.0"), 24),
                (ip("2001:db8::"), 32),
                (ip("10.0.0.0"), 8)
            ]
        );
        for bad in [
            json!({ "allow": true, "deny_cidrs": ["8.8.8.0/33"] }),
            json!({ "allow": true, "deny_cidrs": ["8.8.8.0"] }),
            json!({ "allow": true, "deny_cidrs": ["2001:db8::/129"] }),
            json!({ "allow": true, "deny_cidrs": [7] }),
            json!({ "allow": true, "deny_cidrs": "8.8.8.0/24" }),
        ] {
            let answer = egress_answer_of(&bad).unwrap();
            assert!(
                !answer.allow
                    && answer.reason == "refused: apid sent a bad deny rule"
                    && answer.deny_cidrs.is_empty(),
                "{bad} must be refused"
            );
        }
        let plain = egress_answer_of(&json!({ "allow": true })).unwrap();
        assert!(plain.allow && plain.deny_cidrs.is_empty() && !plain.intercept);
    }

    #[test]
    fn an_injection_answer_is_a_plain_header_or_nothing() {
        let rule = json!({ "header": "x-api-key", "value": "sk-test", "rule": "r1" });
        let injection = injection_of(&rule).unwrap().expect("a header");
        assert_eq!(
            (injection.header.as_str(), injection.value.as_str()),
            ("x-api-key", "sk-test")
        );
        assert!(
            injection_of(&json!({})).unwrap().is_none(),
            "no rule, no header"
        );
        for bad in [
            json!({ "header": "x:y", "value": "v" }),
            json!({ "header": "", "value": "v" }),
            json!({ "header": "x-a", "value": "v\r\nX-Evil: 1" }),
            json!({ "header": "x-a" }),
        ] {
            assert!(injection_of(&bad).is_err(), "{bad} must be refused");
        }
    }
}
