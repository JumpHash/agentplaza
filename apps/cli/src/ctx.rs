//! What every command shares: the hub it talks to (flags, environment, the saved hubs), the HTTP
//! client, and output (`--json` prints the hub's JSON as is; otherwise tables and lines).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The local hub the desktop app and `agentplaza hub start` run.
pub const LOCAL_HUB: &str = "http://127.0.0.1:7700";

/// How to reach the hub; resolved from `--hub/--token/--pin`, `APZ_HUB/APZ_TOKEN/APZ_PIN`, the
/// saved default hub (`agentplaza login`), else the local hub with its owner token.
pub struct Ctx {
    pub client: Client,
    /// `--json`: print the hub's JSON instead of tables.
    pub json: bool,
}

impl Ctx {
    /// Prints `value` as JSON (pretty) when `--json`, else calls `human`.
    pub fn out(&self, value: &Value, human: impl FnOnce(&Value)) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
        } else {
            human(value);
        }
    }
}

/// The hub client: JSON over `apz_http::client` (plain HTTP on loopback, pinned TLS to a remote
/// hub, CA-verified TLS to a public host), with the owner token.
#[derive(Debug, Clone)]
pub struct Client {
    pub target: apz_http::client::Target,
    pub timeout: Duration,
}

impl Client {
    pub fn new(base: &str, pin: Option<[u8; 32]>, bearer: Option<String>) -> Client {
        Client {
            target: apz_http::client::Target { base: base.trim_end_matches('/').to_string(), pin, bearer, headers: Vec::new() },
            timeout: Duration::from_secs(30),
        }
    }

    pub fn base(&self) -> &str {
        &self.target.base
    }

    /// One call; a non-2xx answer becomes `Err` with the hub's `error` text.
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let (status, value) = apz_http::client::request_json(&self.target, method, path, body, self.timeout)
            .map_err(|e| format!("{} {}: {e}", self.base(), path))?;
        if (200..300).contains(&status) {
            Ok(value)
        } else {
            Err(value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("{method} {path}: HTTP {status}")))
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        self.call("GET", path, None)
    }
    pub fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.call("POST", path, Some(body))
    }
    pub fn put(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.call("PUT", path, Some(body))
    }
    pub fn patch(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.call("PATCH", path, Some(body))
    }
    pub fn delete(&self, path: &str) -> Result<Value, String> {
        self.call("DELETE", path, None)
    }
}

/// A saved hub (`agentplaza login`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedHub {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The hub certificate's SHA-256 pin, hex (remote hubs use their own certificate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
}

/// `cli.json` in the platform's config directory (private: it holds owner tokens).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub hubs: BTreeMap<String, SavedHub>,
    #[serde(default)]
    pub default: Option<String>,
}

impl Config {
    pub fn path() -> PathBuf {
        apz_os::dirs::config().join("cli.json")
    }

    pub fn load() -> Config {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            apz_os::private::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        apz_os::private::write(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Parses a 64-hex-digit SHA-256 pin.
pub fn parse_pin(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim().trim_start_matches("sha256:").replace(':', "");
    if hex.len() != 64 {
        return Err("a pin is 64 hex digits (the hub certificate's SHA-256)".into());
    }
    let mut pin = [0u8; 32];
    for (i, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| "a pin is hex digits".to_string())?;
    }
    Ok(pin)
}

/// The local hub's owner token, written by apid in its state directory.
pub fn local_owner_token() -> Option<String> {
    let text = std::fs::read_to_string(apz_os::dirs::state().join("owner-token")).ok()?;
    let token = text.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Resolves the hub from the flags (already merged with the environment by clap), then the saved
/// default, then the local hub.
pub fn resolve(hub: Option<String>, token: Option<String>, pin: Option<String>, json: bool) -> Result<Ctx, String> {
    let config = Config::load();
    let saved = match &hub {
        Some(name_or_url) => config.hubs.get(name_or_url).cloned(),
        None => config.default.as_ref().and_then(|name| config.hubs.get(name).cloned()),
    };
    let url = match (&hub, &saved) {
        (_, Some(saved)) => saved.url.clone(),
        (Some(url), None) => url.clone(),
        (None, None) => LOCAL_HUB.to_string(),
    };
    let token = token.or_else(|| saved.as_ref().and_then(|s| s.token.clone())).or_else(|| {
        (url == LOCAL_HUB).then(local_owner_token).flatten()
    });
    let pin = match pin.or_else(|| saved.as_ref().and_then(|s| s.pin.clone())) {
        Some(hex) => Some(parse_pin(&hex)?),
        None => None,
    };
    Ok(Ctx { client: Client::new(&url, pin, token), json })
}

// ---- Output helpers ---------------------------------------------------------------------------

/// A string field (or "").
pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Any field as display text ("" when missing; numbers and bools as they are).
pub fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Prints rows as an aligned table under a header. Cells are cut at 60 characters.
pub fn table(header: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("(none)");
        return;
    }
    let cut = |c: &str| -> String {
        let one_line = c.replace('\n', " ");
        if one_line.chars().count() > 60 { one_line.chars().take(59).collect::<String>() + "…" } else { one_line }
    };
    let rows: Vec<Vec<String>> = rows.iter().map(|r| r.iter().map(|c| cut(c)).collect()).collect();
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let line = |cells: Vec<&str>| {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths.get(i).copied().unwrap_or(0).saturating_sub(cell.chars().count());
            out.push_str(cell);
            if i + 1 < cells.len() {
                out.push_str(&" ".repeat(pad + 2));
            }
        }
        println!("{}", out.trim_end());
    };
    line(header.iter().map(|h| h.to_uppercase()).collect::<Vec<_>>().iter().map(String::as_str).collect());
    for row in &rows {
        line(row.iter().map(String::as_str).collect());
    }
}

/// Prints `key: value` lines, aligned.
pub fn fields(pairs: &[(&str, String)]) {
    let width = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (k, v) in pairs {
        println!("{k:>width$}  {v}");
    }
}

/// Percent-encodes a path segment (ids and names in URLs).
pub fn seg(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_parse_in_both_spellings() {
        let hex = "ab".repeat(32);
        assert_eq!(parse_pin(&hex).unwrap(), [0xab; 32]);
        assert_eq!(parse_pin(&format!("sha256:{hex}")).unwrap(), [0xab; 32]);
        assert!(parse_pin("abc").is_err());
    }

    #[test]
    fn segments_are_encoded() {
        assert_eq!(seg("a b/c"), "a%20b%2Fc");
        assert_eq!(seg("glm-5.3"), "glm-5.3");
    }
}
