//! The hub's client for `apid`'s internal surface (`docs/architecture.md`,
//! "Internal endpoints"): the hello that fetches an agent's profile, the
//! forward of each v3 guest line, and the `gone` at session end. Loopback
//! only. Every request carries the internal token, read from its file on each
//! call, so an apid that restarted with a new token is picked up at once.

use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use apz_http::client::send_with;
use apz_proto::wire::{AgentProfile, Decision, GuestMessage, HostMessage};
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
/// Without a token file, forwarding is off and nothing is sent.
#[derive(Debug, Clone)]
pub struct Apid {
    port: u16,
    token_file: Option<PathBuf>,
}

impl Apid {
    /// Forwarding off (the daemon's own is `new` without a token file).
    #[cfg(test)]
    pub fn off() -> Self {
        Self {
            port: 0,
            token_file: None,
        }
    }

    /// Forwarding to `url` (`http://127.0.0.1:PORT`) with the token in `token_file`.
    pub fn new(url: &str, token_file: Option<PathBuf>) -> Result<Self, String> {
        Ok(Self {
            port: port_of(url)?,
            token_file,
        })
    }

    /// Whether v3 lines and hellos go to apid at all.
    pub fn enabled(&self) -> bool {
        self.token_file.is_some()
    }

    /// The agent's profile from apid (`None` when apid has none for it).
    pub fn hello(&self, cid: u32, role: &str, vm: u32) -> Result<Option<AgentProfile>, String> {
        let body = json!({ "role": role, "vm": vm }).to_string();
        let reply = self.post(
            &format!("/internal/v1/agents/{cid}/hello"),
            &body,
            HELLO_TIMEOUT,
        )?;
        serde_json::from_value(reply.get("profile").cloned().unwrap_or_default())
            .map_err(|err| format!("apid's profile is malformed: {err}"))
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
        Ok(EgressAnswer {
            allow,
            reason,
            intercept,
        })
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
        let token = self.token()?;
        let path = format!("/internal/e2b/sandboxes/{id}");
        let reply = send_with(
            self.port,
            "GET",
            &path,
            &[(TOKEN_HEADER, &token)],
            None,
            E2B_LOOKUP_TIMEOUT,
        )
        .map_err(|err| format!("apid unreachable: {err}"))?;
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
        let token = self.token()?;
        let reply = send_with(
            self.port,
            "POST",
            path,
            &[(TOKEN_HEADER, &token)],
            Some(body.as_bytes()),
            timeout,
        )
        .map_err(|err| format!("apid unreachable: {err}"))?;
        if reply.status != 200 {
            return Err(format!("apid answered {} to {path}", reply.status));
        }
        reply.json().map_err(|err| format!("apid's answer: {err}"))
    }

    /// The token as the file holds it now (trimmed). Never echoed in an error.
    pub fn token(&self) -> Result<String, String> {
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
        }),
        GuestMessage::ToolRequest { call, .. } => Some(HostMessage::ToolDecision {
            call: call.clone(),
            decision: Decision::Deny,
            output: reason.to_string(),
            is_error: true,
            program: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
