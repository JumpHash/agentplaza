//! The built-in DAST scan of a topology's routes (PLAN 22.8): a small scanner for the common injection classes, run with
//! GET requests against each route's parameters. It probes SQL injection (an error, a true and a false condition, a
//! delay), reflected cross-site scripting, path traversal, command injection and open redirects, and reports a class only
//! when the answer shows it: the payload's effect on the page, never the payload alone.
//!
//! The requests go through [`Fetch`], so the checks are tested against a fake, and the scan against a seeded server.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::{Hit, Outcome};
use crate::spec::Route;

/// One answer: its status, its `Location` header, its body (the first megabyte) and the time it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub location: Option<String>,
    pub body: String,
    pub millis: u64,
}

/// Sends a GET and returns its answer. Redirects are not followed: an open redirect is the answer's `Location`.
pub trait Fetch {
    fn get(&self, url: &str) -> Result<Reply, String>;
}

/// The HTTP client of the scan: no redirects, a timeout per request.
pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new() -> Http {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        Http {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for Http {
    fn default() -> Self {
        Http::new()
    }
}

impl Fetch for Http {
    fn get(&self, url: &str) -> Result<Reply, String> {
        let started = Instant::now();
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = response
            .body_mut()
            .with_config()
            .limit(1 << 20)
            .read_to_string()
            .unwrap_or_default();
        Ok(Reply {
            status,
            location,
            body,
            millis: started.elapsed().as_millis() as u64,
        })
    }
}

/// The query string's encoding of a value: every byte but the unreserved characters as `%XX`.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The URL of `path` on `base` with `param` set to `value`.
pub fn url(base: &str, path: &str, param: &str, value: &str) -> String {
    format!(
        "{}{}?{}={}",
        base.trim_end_matches('/'),
        path,
        encode(param),
        encode(value)
    )
}

/// The DAST stages of a topology's routes: one outcome per route (`dast:<service><path>`), skipped where its service has
/// no URL in the run.
pub fn stages(routes: &[Route], services: &BTreeMap<String, String>) -> Vec<Outcome> {
    routes
        .iter()
        .map(|route| {
            let stage = format!("dast:{}{}", route.service, route.path);
            let Some(base) = services.get(&route.service) else {
                return Outcome::skipped(
                    &stage,
                    format!("no URL was given for service {}", route.service),
                );
            };
            match scan_route(base, &route.path, &route.params, &Http::new()) {
                Ok(hits) => Outcome::ran(
                    &stage,
                    hits,
                    format!("{} parameters probed", route.params.len()),
                ),
                Err(why) => Outcome::skipped(&stage, format!("the route did not answer: {why}")),
            }
        })
        .collect()
}

/// The classes the scan probes for on each parameter of a route, over `base` (a URL with no path). `params` maps each
/// parameter to a sample value that the route answers normally. Errors are the route's answer failing to come at all.
pub fn scan_route(
    base: &str,
    path: &str,
    params: &BTreeMap<String, String>,
    fetch: &dyn Fetch,
) -> Result<Vec<Hit>, String> {
    let mut hits = Vec::new();
    for (param, sample) in params {
        let baseline = fetch.get(&url(base, path, param, sample))?;
        let probe = Probe {
            fetch,
            base,
            path,
            param,
            sample,
            baseline: &baseline,
        };
        hits.extend(sql_injection(&probe)?);
        hits.extend(reflected_xss(&probe)?);
        hits.extend(path_traversal(&probe)?);
        hits.extend(command_injection(&probe)?);
        hits.extend(open_redirect(&probe)?);
    }
    Ok(hits)
}

/// One parameter of one route, with its normal answer.
struct Probe<'a> {
    fetch: &'a dyn Fetch,
    base: &'a str,
    path: &'a str,
    param: &'a str,
    sample: &'a str,
    baseline: &'a Reply,
}

impl Probe<'_> {
    fn send(&self, value: &str) -> Result<Reply, String> {
        self.fetch
            .get(&url(self.base, self.path, self.param, value))
    }

    fn hit(&self, class: &str, severity: &str, detail: String) -> Hit {
        Hit {
            id: class.into(),
            severity: severity.into(),
            subject: format!("GET {} parameter {}", self.path, self.param),
            detail,
        }
    }
}

const SQL_ERRORS: &[&str] = &[
    "SQL syntax",
    "SQLITE_ERROR",
    "sqlite3",
    "syntax error",
    "unrecognized token",
    "unterminated quoted string",
    "near \"",
    "You have an error in your SQL",
    "PG::SyntaxError",
    "ORA-",
    "Unclosed quotation mark",
];
/// The delay a time-based probe asks for, in seconds, and the least extra time that counts as a delay.
const TIME_PAYLOADS: &[&str] = &["SLEEP(2)", "pg_sleep(2)"];
const SLOW_MS: u64 = 1500;

fn sql_injection(p: &Probe<'_>) -> Result<Vec<Hit>, String> {
    const CLASS: &str = "sql-injection";
    let error = p.send(&format!("{}'", p.sample))?;
    if let Some(signature) = SQL_ERRORS
        .iter()
        .find(|s| error.body.contains(**s) && !p.baseline.body.contains(**s))
    {
        return Ok(vec![p.hit(
            CLASS,
            "high",
            format!("error-based: a quote after the value makes the answer say \"{signature}\""),
        )]);
    }
    let never = p.send(&format!("{}' AND '1'='2", p.sample))?;
    let always = p.send(&format!("{}' AND '1'='1", p.sample))?;
    if !p.baseline.body.is_empty()
        && always.body == p.baseline.body
        && never.body != p.baseline.body
    {
        return Ok(vec![p.hit(
            CLASS,
            "high",
            "boolean-based: a true condition answers as the value does, a false one differs".into(),
        )]);
    }
    for payload in TIME_PAYLOADS {
        let first = p.send(&format!("{}' AND {payload} AND '1'='1", p.sample))?;
        let second = p.send(&format!("{}' AND {payload} AND '1'='1", p.sample))?;
        let slow = |r: &Reply| r.millis >= p.baseline.millis + SLOW_MS;
        if slow(&first) && slow(&second) {
            return Ok(vec![p.hit(
                CLASS,
                "high",
                format!(
                    "time-based: {payload} delays the answer ({} ms, normally {} ms), twice",
                    second.millis, p.baseline.millis
                ),
            )]);
        }
    }
    Ok(Vec::new())
}

const XSS_PAYLOAD: &str = "<apz9x>\"'";

fn reflected_xss(p: &Probe<'_>) -> Result<Vec<Hit>, String> {
    let reply = p.send(&format!("{}{XSS_PAYLOAD}", p.sample))?;
    if reply.body.contains(XSS_PAYLOAD) && !p.baseline.body.contains(XSS_PAYLOAD) {
        return Ok(vec![p.hit(
            "reflected-xss",
            "medium",
            format!("the payload {XSS_PAYLOAD} comes back in the answer unescaped"),
        )]);
    }
    Ok(Vec::new())
}

const TRAVERSAL_PAYLOADS: &[&str] = &[
    "../../../../../../../../etc/passwd",
    "....//....//....//....//etc/passwd",
];

fn is_passwd(body: &str) -> bool {
    body.contains("root:") && body.contains(":0:0:")
}

fn path_traversal(p: &Probe<'_>) -> Result<Vec<Hit>, String> {
    for payload in TRAVERSAL_PAYLOADS {
        let reply = p.send(payload)?;
        if is_passwd(&reply.body) && !is_passwd(&p.baseline.body) {
            return Ok(vec![p.hit(
                "path-traversal",
                "high",
                format!("the value {payload} reads a file outside the route's directory (a passwd file's lines came back)"),
            )]);
        }
    }
    Ok(Vec::new())
}

const COMMAND_PAYLOADS: &[&str] = &[
    ";echo apz$((7*191))x",
    "|echo apz$((7*191))x",
    "$(echo apz$((7*191))x)",
    "`echo apz$((7*191))x`",
];
/// What the marker the payloads print comes out as when the shell evaluates it.
const COMMAND_MARKER: &str = "apz1337x";

fn command_injection(p: &Probe<'_>) -> Result<Vec<Hit>, String> {
    for payload in COMMAND_PAYLOADS {
        let reply = p.send(&format!("{}{payload}", p.sample))?;
        if reply.body.contains(COMMAND_MARKER) && !p.baseline.body.contains(COMMAND_MARKER) {
            return Ok(vec![p.hit(
                "command-injection",
                "high",
                format!(
                    "the payload {payload} runs a command: its marker {COMMAND_MARKER} comes back"
                ),
            )]);
        }
    }
    Ok(Vec::new())
}

const CANARY: &str = "https://apz-canary.invalid/";

fn open_redirect(p: &Probe<'_>) -> Result<Vec<Hit>, String> {
    let reply = p.send(CANARY)?;
    let to_canary = reply
        .location
        .as_deref()
        .is_some_and(|l| l.starts_with(CANARY) || l.starts_with("//apz-canary.invalid"));
    if (300..400).contains(&reply.status) && to_canary {
        return Ok(vec![p.hit(
            "open-redirect",
            "medium",
            format!(
                "the answer redirects ({}) to the value's own host",
                reply.status
            ),
        )]);
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fetcher that answers each URL by `answer`.
    struct Fake<F: Fn(&str) -> Reply>(F);

    impl<F: Fn(&str) -> Reply> Fetch for Fake<F> {
        fn get(&self, url: &str) -> Result<Reply, String> {
            Ok((self.0)(url))
        }
    }

    fn page(body: &str) -> Reply {
        Reply {
            status: 200,
            location: None,
            body: body.into(),
            millis: 5,
        }
    }

    fn params(name: &str, sample: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(name.to_string(), sample.to_string())])
    }

    fn classes(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.id.as_str()).collect()
    }

    #[test]
    fn the_encoding_keeps_unreserved_characters_and_escapes_the_rest() {
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode("x' OR 1=1"), "x%27%20OR%201%3D1");
        assert_eq!(url("http://h:1/", "/p", "q", "a b"), "http://h:1/p?q=a%20b");
    }

    #[test]
    fn a_time_based_delay_is_found_from_the_answers_times() {
        let fake = Fake(|url: &str| {
            let millis = if url.contains("SLEEP") { 2100 } else { 5 };
            Reply {
                millis,
                ..page("Alice\n")
            }
        });
        let hits = scan_route("http://h", "/users", &params("name", "Alice"), &fake).unwrap();
        assert_eq!(classes(&hits), ["sql-injection"], "{hits:?}");
        assert!(
            hits[0].detail.starts_with("time-based"),
            "{}",
            hits[0].detail
        );
    }

    #[test]
    fn a_single_slow_answer_is_not_a_delay() {
        // Only the first of the two identical requests is slow: noise, not a delay.
        let slow_once = std::cell::Cell::new(0);
        let fake = Fake(|url: &str| {
            let millis = if url.contains("SLEEP") {
                slow_once.set(slow_once.get() + 1);
                if slow_once.get() == 1 { 2100 } else { 5 }
            } else {
                5
            };
            Reply {
                millis,
                ..page("Alice\n")
            }
        });
        let hits = scan_route("http://h", "/users", &params("name", "Alice"), &fake).unwrap();
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn a_route_that_answers_nothing_is_an_error_for_its_outcome() {
        struct Down;
        impl Fetch for Down {
            fn get(&self, url: &str) -> Result<Reply, String> {
                Err(format!("{url}: connection refused"))
            }
        }
        let error = scan_route("http://h", "/users", &params("name", "Alice"), &Down).unwrap_err();
        assert!(error.contains("connection refused"));
    }

    #[test]
    fn a_route_whose_service_has_no_url_is_a_skip_with_its_reason() {
        let route = Route {
            service: "api".into(),
            path: "/users".into(),
            params: params("name", "Alice"),
        };
        let outcomes = stages(&[route], &BTreeMap::new());
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].ran);
        assert!(
            outcomes[0].reason.contains("no URL"),
            "{}",
            outcomes[0].reason
        );
    }
}
