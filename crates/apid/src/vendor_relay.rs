//! The vendor relay: an engine whose vendor needs its own account (Auggie, through Augment's API) runs
//! in a VM that never holds that credential. The VM sends its requests to `plaza.gateway/vendor/<vendor>/…`;
//! vsockd rewrites them to `/internal/v1/agents/{cid}/gateway/vendor/<vendor>/…`, and the hub relays each
//! one to the vendor's own base URL with the credential the vault holds for the agent's binding, then
//! streams the answer back. Adding a vendor is one entry in [`VENDORS`].

use std::io::{Read, Write};
use std::time::Duration;

use apz_http::{Body, Request, Response, RouteDoc, Router};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::internal::authorized;
use crate::state::App;
use crate::vault::{Consumer, VaultError, name_for};

/// A vendor's credential, read from the secret the vault holds: the upstream base URL and the headers that
/// carry the credential. A refusal names the owner's step and never echoes the secret.
pub type Upstream = fn(&SecretString) -> Result<(String, Vec<(String, String)>), String>;

/// A vendor the hub relays to.
pub struct Vendor {
    /// The path segment after `/vendor/`.
    pub id: &'static str,
    /// The vault name of the credential.
    pub secret: &'static str,
    /// The vault binding allowed to read it: the engine that uses it.
    pub binding: &'static str,
    /// The engine (`code_engine` id) the relay serves: it must be among the owner's enabled engines,
    /// and the calling agent's role must be granted `code_engine`.
    pub engine: &'static str,
    /// The label the console shows for the binding.
    pub label: &'static str,
    /// The 403 the relay answers when the vault cannot give the credential.
    pub refusal: &'static str,
    pub upstream: Upstream,
    /// The host suffix the upstream base must end in, under https (the dot included).
    pub allowed_host_suffix: &'static str,
}

const AUGMENT_HOST: &str = ".augmentcode.com";

const VENDORS: &[Vendor] = &[Vendor {
    id: "augment",
    secret: "AUGMENT_SESSION_AUTH",
    binding: "engine:auggie",
    engine: "auggie",
    label: "Auggie (Augment session)",
    refusal: "Auggie needs your Augment session in the vault: store `auggie token print` as AUGMENT_SESSION_AUTH and bind it to engine:auggie",
    upstream: augment_upstream,
    allowed_host_suffix: AUGMENT_HOST,
}];

/// The vendor paths for the methods the vendors' APIs use: GET and POST, the two a model call and a
/// session read need. Any other method is the router's 405. The `{rest..}` capture keeps the vendor's
/// own path whole, so the relay needs no route per vendor endpoint.
pub fn routes(router: Router<App>) -> Router<App> {
    const PATH: &str = "/internal/v1/agents/{cid}/gateway/vendor/{rest..}";
    router
        .get(PATH, |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                relay(app, req, cid, p.get("rest"))
            })
        })
        .doc(RouteDoc::new("Relay a vendor read for the agent").errors(&[403, 404, 409]))
        .post(PATH, |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                relay(app, req, cid, p.get("rest"))
            })
        })
        .doc(RouteDoc::new("Relay a vendor call for the agent").errors(&[403, 404, 409]))
}

/// Registers each vendor's binding with the vault, so the console lists it and the owner can bind the
/// credential to the engine. Called once at boot.
pub fn register(vault: &crate::vault::Store) {
    for vendor in VENDORS {
        vault.register_consumer(vendor.binding, vendor.label, &name_for(vendor.secret));
    }
}

/// Relays one vendor call for agent `cid`. `rest` is `<vendor>/<the vendor's path>`.
fn relay(app: &App, req: &Request, cid: u64, rest: &str) -> Result<Response, Response> {
    let (id, tail) = split_vendor(rest).ok_or_else(|| Response::error(404, "no such endpoint"))?;
    let vendor = vendor_named(id).ok_or_else(|| Response::error(404, "unknown vendor"))?;
    if !safe_tail(tail) {
        return Err(Response::error(400, "the vendor path is not a plain path"));
    }
    // Held by STOP ALL like every other model call (see `llm_proxy::relay`), and so is an agent whose project is
    // paused or over (see `projects::held`).
    if app.control.paused() {
        return Err(Response::error(
            409,
            "the plaza is held (STOP ALL); waiting for the owner to resume",
        ));
    }
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| Response::error(404, "unknown agent"))?;
    if let Some(why) = crate::projects::held(app, &agent) {
        return Err(Response::error(409, &why));
    }
    // Only an agent that may run this vendor's engine reaches the vendor with the owner's account.
    let enabled = app.settings.get().engines_enabled;
    let granted = crate::tools::specs_for_agent(app, &agent)
        .iter()
        .any(|spec| spec.name == "code_engine");
    if let Some(refused) = engine_refusal(vendor.engine, &enabled, granted) {
        return Err(Response::error(403, &refused));
    }
    let consumer = Consumer::agent(&app.nodes.list(), cid, vendor.binding);
    let secret = app
        .vault
        .resolve(&name_for(vendor.secret), &consumer, apz_plaza::now_ms())
        .map_err(|err| refusal(vendor, &err))?;
    let (base, mut headers) =
        (vendor.upstream)(&secret).map_err(|message| Response::error(403, &message))?;
    // The builder checked the base already; the relay checks it again against the table's suffix, so a
    // vendor added later cannot relay anywhere its entry does not allow.
    let base = checked_base(&base, vendor.allowed_host_suffix)
        .map_err(|message| Response::error(403, &message))?;
    headers.extend(forwarded_headers(&req.headers));

    let url = target_url(&base, tail, &req.query);
    let mut builder = ureq::http::Request::builder()
        .method(req.method.as_str())
        .uri(url.as_str());
    for (name, value) in &headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let request = builder
        .body(req.body.clone())
        .map_err(|err| Response::error(400, &format!("the request cannot be sent: {err}")))?;
    let resp = http_agent()
        .run(request)
        .map_err(|err| Response::error(502, &format!("upstream: {err}")))?;

    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let mut reader = resp.into_body().into_reader();
    let mut out = Response::new(status, &content_type, Vec::new());
    // The answer goes back as it arrives: an event stream is relayed event by event, not buffered.
    out.body = Body::Stream(Box::new(move |sink: &mut dyn Write| {
        copy_through(&mut reader, sink)
    }));
    Ok(out)
}

/// Copies the upstream answer to the client chunk by chunk, flushing each one.
fn copy_through(reader: &mut impl Read, sink: &mut dyn Write) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        sink.write_all(&buf[..n])?;
        sink.flush()?;
    }
}

/// How long the vendor's host may take to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the vendor may take to answer, and then to finish its answer. ureq 3 has no per-read limit, so this
/// is the budget for the whole response body, counted from its headers: a stalled vendor cannot hold a hub
/// thread forever. A stream that runs longer than this is cut.
const READ_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The upstream agent's config: a status is an answer, not an error (as `llm_proxy::http_agent`), and the
/// connect and read limits above.
fn upstream_config() -> ureq::config::Config {
    ureq::config::Config::builder()
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(READ_TIMEOUT))
        .timeout_recv_body(Some(READ_TIMEOUT))
        .build()
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(upstream_config())
}

/// The 403 for a credential the vault would not give: a sealed vault says so, a tampered one says why,
/// and a missing or unbound secret gets the owner's step.
fn refusal(vendor: &Vendor, err: &VaultError) -> Response {
    let message = match err {
        VaultError::Sealed | VaultError::Uninitialized => {
            "the vault is sealed; unseal it so the engine can reach its vendor account".to_string()
        }
        VaultError::Tampered(_) => err.message(),
        _ => vendor.refusal.to_string(),
    };
    Response::error(403, &message)
}

fn vendor_named(id: &str) -> Option<&'static Vendor> {
    VENDORS.iter().find(|vendor| vendor.id == id)
}

/// Splits `augment/chat/stream` into the vendor and the rest of its path; `None` without a rest.
fn split_vendor(path: &str) -> Option<(&str, &str)> {
    let (id, tail) = path.split_once('/')?;
    (!id.is_empty() && !tail.is_empty()).then_some((id, tail))
}

/// A vendor's path goes upstream as written, unless a segment is empty or would climb out of the base, or
/// would change the query or fragment once joined.
fn safe_tail(tail: &str) -> bool {
    tail.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment.contains(['\\', '?', '#'])
    })
}

/// The upstream URL: the base, the vendor's path and the query the client sent.
fn target_url(base: &str, tail: &str, query: &str) -> String {
    if query.is_empty() {
        format!("{base}/{tail}")
    } else {
        format!("{base}/{tail}?{query}")
    }
}

/// Headers the client never sends on: the placeholder credential it holds, the hop-by-hop headers, and the
/// framing and encoding the relay sets itself (it does not decode what the vendor compresses).
const DROPPED: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "cookie",
    "host",
    "content-length",
    "accept-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn forwarded_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| {
            !DROPPED
                .iter()
                .any(|dropped| name.eq_ignore_ascii_case(dropped))
        })
        .cloned()
        .collect()
}

/// The vendor's base URL when it is `https://` on a host ending in `host_suffix`, the dot included, so the
/// bare domain is refused. Anything else would make the hub an open proxy for whatever the owner stored.
fn checked_base(url: &str, host_suffix: &str) -> Result<String, String> {
    let refuse = || format!("the stored base URL must be an https:// URL on a {host_suffix} host");
    let rest = url.strip_prefix("https://").ok_or_else(refuse)?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let host = authority.to_ascii_lowercase();
    let named = host.ends_with(host_suffix) && host.len() > host_suffix.len();
    if !named || authority.contains(['@', ':', ' ']) || path.contains(['?', '#', ' ']) {
        return Err(refuse());
    }
    Ok(format!("https://{authority}{}", path.trim_end_matches('/')))
}

/// Augment's session, as `auggie token print` writes it: the bearer token and the tenant's URL. The tenant's
/// API is the base, and the token goes out as a bearer.
fn augment_upstream(secret: &SecretString) -> Result<(String, Vec<(String, String)>), String> {
    // The stored value is the owner's credential: no message below quotes it.
    let session: Value = serde_json::from_str(secret.expose_secret()).map_err(|_| {
        "the stored Augment session is not JSON; store the output of `auggie token print`"
            .to_string()
    })?;
    let token = session
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "the stored Augment session has no accessToken".to_string())?;
    let tenant = session
        .get("tenantURL")
        .and_then(Value::as_str)
        .ok_or_else(|| "the stored Augment session has no tenantURL".to_string())?;
    let base = checked_base(tenant, AUGMENT_HOST)?;
    Ok((
        base,
        vec![("Authorization".to_string(), format!("Bearer {token}"))],
    ))
}

/// Why an agent may not use a vendor's relay: its engine is off, or the agent may not run engines.
fn engine_refusal(engine: &str, enabled: &[String], code_engine_granted: bool) -> Option<String> {
    if !enabled.iter().any(|id| id == engine) {
        return Some(format!(
            "the owner has not enabled the {engine} engine, so its vendor relay is closed"
        ));
    }
    if !code_engine_granted {
        return Some("this agent's role may not run coding engines".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_relay_serves_only_an_enabled_engine_and_an_agent_that_may_run_engines() {
        let on = vec!["claude".to_string(), "auggie".to_string()];
        assert_eq!(engine_refusal("auggie", &on, true), None);
        assert!(engine_refusal("auggie", &on[..1], true).is_some());
        assert!(engine_refusal("auggie", &on, false).is_some());
    }

    use super::*;

    fn secret(json: &str) -> SecretString {
        SecretString::from(json.to_string())
    }

    #[test]
    fn a_vendor_path_splits_into_the_vendor_and_its_rest() {
        assert_eq!(
            split_vendor("augment/chat/stream"),
            Some(("augment", "chat/stream"))
        );
        assert_eq!(split_vendor("augment/x"), Some(("augment", "x")));
        assert_eq!(split_vendor("augment"), None);
        assert_eq!(split_vendor("augment/"), None);
        assert_eq!(split_vendor("/chat"), None);
        assert_eq!(
            vendor_named("augment").map(|v| v.binding),
            Some("engine:auggie")
        );
        assert!(vendor_named("openai").is_none());
    }

    #[test]
    fn a_path_that_climbs_or_changes_the_query_is_not_relayed() {
        assert!(safe_tail("chat/stream"));
        assert!(!safe_tail("a/../b"));
        assert!(!safe_tail("./x"));
        assert!(!safe_tail("a//b"));
        assert!(!safe_tail("a?b"));
        assert!(!safe_tail("a#b"));
        assert!(!safe_tail(""));
    }

    #[test]
    fn the_upstream_url_keeps_the_query_the_client_sent() {
        let base = "https://acme.api.augmentcode.com";
        assert_eq!(
            target_url(base, "chat/stream", ""),
            format!("{base}/chat/stream")
        );
        assert_eq!(
            target_url(base, "chat", "alt=sse"),
            format!("{base}/chat?alt=sse")
        );
    }

    #[test]
    fn only_https_on_the_vendors_host_is_relayed() {
        assert_eq!(
            checked_base("https://acme.api.augmentcode.com/", AUGMENT_HOST).as_deref(),
            Ok("https://acme.api.augmentcode.com")
        );
        assert_eq!(
            checked_base("https://acme.api.augmentcode.com/v2/", AUGMENT_HOST).as_deref(),
            Ok("https://acme.api.augmentcode.com/v2")
        );
        for refused in [
            "http://acme.api.augmentcode.com/",
            "https://augmentcode.com/",
            "https://evil.example/",
            "https://acme.api.augmentcode.com.evil.example/",
            "https://user@acme.api.augmentcode.com/",
            "https://acme.api.augmentcode.com:8443/",
            "https://acme.api.augmentcode.com/?x=1",
            "acme.api.augmentcode.com",
            "https://",
        ] {
            assert!(
                checked_base(refused, AUGMENT_HOST).is_err(),
                "{refused} relayed"
            );
        }
    }

    #[test]
    fn augment_upstream_reads_the_session_and_refuses_what_it_cannot_read() {
        // the tenant is the base and the token is sent as a bearer
        {
            let json = r#"{"accessToken":"tok-123","tenantURL":"https://acme.api.augmentcode.com/","scopes":["read"]}"#;
            let (base, headers) = augment_upstream(&secret(json)).unwrap();
            assert_eq!(base, "https://acme.api.augmentcode.com");
            assert_eq!(
                headers,
                vec![("Authorization".to_string(), "Bearer tok-123".to_string())]
            );
        }
        // a session it cannot read is refused without echoing the token
        {
            let truncated = r#"{"accessToken":"tok-123","tenantURL":"https://acme.api.augmentcode"#;
            let err = augment_upstream(&secret(truncated)).unwrap_err();
            assert!(
                !err.contains("tok-123"),
                "the message echoed the token: {err}"
            );

            let foreign = r#"{"accessToken":"tok-123","tenantURL":"https://evil.example/"}"#;
            assert!(augment_upstream(&secret(foreign)).is_err());

            let no_token = r#"{"tenantURL":"https://acme.api.augmentcode.com/"}"#;
            assert!(
                augment_upstream(&secret(no_token))
                    .unwrap_err()
                    .contains("accessToken")
            );

            let empty = r#"{"accessToken":"","tenantURL":"https://acme.api.augmentcode.com/"}"#;
            assert!(augment_upstream(&secret(empty)).is_err());
        }
    }

    #[test]
    fn the_upstream_agent_bounds_its_connect_and_its_read() {
        let timeouts = upstream_config().timeouts();
        assert_eq!(timeouts.connect, Some(Duration::from_secs(15)));
        assert_eq!(timeouts.recv_response, Some(Duration::from_secs(600)));
        assert_eq!(timeouts.recv_body, Some(Duration::from_secs(600)));
        assert!(!upstream_config().http_status_as_error());
    }

    #[test]
    fn the_placeholder_and_hop_by_hop_headers_never_go_upstream() {
        let headers = [
            ("authorization", "Bearer placeholder"),
            ("x-api-key", "placeholder"),
            ("Content-Type", "application/json"),
            ("accept", "text/event-stream"),
            ("connection", "keep-alive"),
            ("transfer-encoding", "chunked"),
            ("accept-encoding", "gzip"),
            ("host", "plaza.gateway"),
            ("x-request-id", "abc"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .to_vec();
        let kept: Vec<String> = forwarded_headers(&headers)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(kept, ["Content-Type", "accept", "x-request-id"]);
    }
}
