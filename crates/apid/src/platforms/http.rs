//! The platforms' one HTTP path: [`Http`] is what every client calls, [`Ureq`] the hub's, [`Fake`] the tests'.

use std::io::Read;
#[cfg(test)]
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

/// How long one call to a platform may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A request body.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Json(Value),
    /// `application/x-www-form-urlencoded` (OAuth token endpoints).
    Form(Vec<(String, String)>),
}

/// A platform's answer: its status and its body (JSON, or the text as a JSON string, or null when empty).
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    /// The body of a 2xx answer, else the platform's own message.
    pub fn ok(self, what: &str) -> Result<Value, String> {
        if (200..300).contains(&self.status) {
            return Ok(self.body);
        }
        // GitLab sends some messages as a map of fields to their errors: those read as their JSON.
        let message = match &self.body["message"] {
            Value::String(m) => m.clone(),
            Value::Null => self.body["error_description"]
                .as_str()
                .or_else(|| self.body["error"].as_str())
                .or_else(|| self.body.as_str())
                .unwrap_or("")
                .to_string(),
            other => other.to_string(),
        };
        // An error page (a proxy's, a server's own) says nothing a person needs; a long message is cut.
        let message = if message.trim_start().starts_with('<') {
            String::new()
        } else if message.chars().count() > 300 {
            format!("{}…", message.chars().take(300).collect::<String>())
        } else {
            message
        };
        Err(
            format!("{what}: the platform answered {} {message}", self.status)
                .trim_end()
                .to_string(),
        )
    }
}

/// One call to a platform.
pub trait Http: Send + Sync {
    fn call(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: Option<Body>,
    ) -> Result<Reply, String>;
}

/// The hub's client.
pub struct Ureq;

impl Http for Ureq {
    fn call(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: Option<Body>,
    ) -> Result<Reply, String> {
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(url)
            .header("User-Agent", "agentplaza")
            .header("Accept", "application/json");
        for (key, value) in headers {
            builder = builder.header(*key, value.as_str());
        }
        let bytes = match body {
            Some(Body::Json(v)) => {
                builder = builder.header("Content-Type", "application/json");
                serde_json::to_vec(&v).map_err(|e| e.to_string())?
            }
            Some(Body::Form(fields)) => {
                builder = builder.header("Content-Type", "application/x-www-form-urlencoded");
                form(&fields).into_bytes()
            }
            None => Vec::new(),
        };
        let request = builder
            .body(bytes)
            .map_err(|e| format!("bad request: {e}"))?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build();
        let response = ureq::Agent::new_with_config(config)
            .run(request)
            .map_err(|e| format!("{url}: no answer: {e}"))?;
        let status = response.status().as_u16();
        let mut raw = Vec::new();
        response
            .into_body()
            .into_reader()
            .read_to_end(&mut raw)
            .map_err(|e| format!("{url}: reading the answer: {e}"))?;
        Ok(Reply {
            status,
            body: parse(&raw),
        })
    }
}

/// A body as JSON: null when empty, the text as a JSON string when it is not JSON (a form-encoded OAuth answer is
/// read into an object).
pub fn parse(raw: &[u8]) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    if let Ok(v) = serde_json::from_slice(raw) {
        return v;
    }
    let text = String::from_utf8_lossy(raw).into_owned();
    if text.contains('=') && !text.contains(char::is_whitespace) {
        let map: serde_json::Map<String, Value> = text
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (unescape(k), Value::String(unescape(v))))
            .collect();
        return Value::Object(map);
    }
    Value::String(text)
}

/// Fields as `application/x-www-form-urlencoded`.
pub fn form(fields: &[(String, String)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", escape(k), escape(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encodes everything but the unreserved characters.
pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unescape(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        match (
            b[i],
            b.get(i + 1).and_then(|c| hex(*c)),
            b.get(i + 2).and_then(|c| hex(*c)),
        ) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
            (b'+', _, _) => out.push(b' '),
            (c, _, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One call a [`Fake`] was asked: method, URL, the `Authorization` header and the body.
#[cfg(test)]
pub type Seen = (String, String, Option<String>, Option<Body>);

/// How a [`Fake`] answers: `(method, url, body)` → `(status, body)`.
#[cfg(test)]
type Answer = Box<dyn Fn(&str, &str, Option<&Body>) -> (u16, Value) + Send + Sync>;

/// A stand-in platform for the tests: each call is answered by `answer(method, url, body)`, and every call is kept.
#[cfg(test)]
pub struct Fake {
    answer: Answer,
    seen: Mutex<Vec<Seen>>,
}

#[cfg(test)]
impl Fake {
    pub fn new(
        answer: impl Fn(&str, &str, Option<&Body>) -> (u16, Value) + Send + Sync + 'static,
    ) -> Fake {
        Fake {
            answer: Box::new(answer),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// The calls so far, oldest first.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The URLs called so far, oldest first.
    pub fn urls(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .map(|(m, u, _, _)| format!("{m} {u}"))
            .collect()
    }
}

#[cfg(test)]
impl Http for Fake {
    fn call(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: Option<Body>,
    ) -> Result<Reply, String> {
        let (status, value) = (self.answer)(method, url, body.as_ref());
        let auth = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.clone());
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).push((
            method.to_string(),
            url.to_string(),
            auth,
            body,
        ));
        Ok(Reply {
            status,
            body: value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_form_answer_reads_as_an_object_and_errors_carry_the_platforms_words() {
        assert_eq!(
            parse(b"access_token=gho_1&scope=repo%2Cuser&token_type=bearer"),
            json!({ "access_token": "gho_1", "scope": "repo,user", "token_type": "bearer" })
        );
        assert_eq!(parse(b""), Value::Null);
        assert_eq!(parse(b"plain words"), json!("plain words"));
        let refused = Reply {
            status: 401,
            body: json!({ "message": "Bad credentials" }),
        };
        assert_eq!(
            refused.ok("reading the user").unwrap_err(),
            "reading the user: the platform answered 401 Bad credentials"
        );
        let page = Reply {
            status: 502,
            body: json!("<!DOCTYPE html><html><body>Bad gateway</body></html>"),
        };
        assert_eq!(
            page.ok("adding the key").unwrap_err(),
            "adding the key: the platform answered 502"
        );
        assert_eq!(form(&[("a b".into(), "x/y".into())]), "a%20b=x%2Fy");
    }
}
