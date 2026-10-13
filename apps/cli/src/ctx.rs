//! What every command shares: the hub it talks to (flags, environment, the saved hubs), the HTTP
//! client, and output (`--json` prints the hub's JSON as is; otherwise tables and lines).

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// The MCP token  uses (kept apart from the owner token above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_token: Option<String>,
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
        self.save_to(&Self::path())
    }

    /// Writes cli.json to `path`, keeping each saved hub's `pinned` entry. The desktop writes `pinned`
    /// (the identity first seen on a connection) and `SavedHub` does not model it, so a plain
    /// serialize would drop it: the pins are read from the file first and written back by name.
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        let pins: Vec<(String, serde_json::Value)> = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|doc| {
                doc.get("hubs").and_then(serde_json::Value::as_object).map(|hubs| {
                    hubs.iter().filter_map(|(name, hub)| Some((name.clone(), hub.get("pinned")?.clone()))).collect()
                })
            })
            .unwrap_or_default();
        let mut doc = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let Some(hubs) = doc.get_mut("hubs").and_then(serde_json::Value::as_object_mut) {
            for (name, pinned) in pins {
                if let Some(hub) = hubs.get_mut(&name).and_then(serde_json::Value::as_object_mut) {
                    hub.insert("pinned".into(), pinned);
                }
            }
        }
        if let Some(dir) = path.parent() {
            apz_os::private::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        apz_os::private::write(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
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

// ---- Times, prompts and text input ------------------------------------------------------------

/// Asks `question [y/N]` on a terminal unless `yes` says to go ahead. Off a terminal it refuses
/// (naming `--yes`), so a destructive action is never assumed. `Ok(false)` is a "no".
pub fn confirm(yes: bool, question: &str) -> Result<bool, String> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!("{question} Not on a terminal: pass --yes to go ahead"));
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// A text flag's value, or stdin for "-". Blank text is refused.
pub fn text_arg(value: &str) -> Result<String, String> {
    let body = if value == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(|e| format!("reading stdin: {e}"))?;
        buf
    } else {
        value.to_string()
    };
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err("the text is empty".into());
    }
    Ok(trimmed.to_string())
}

/// Epoch milliseconds from a hub value, when it is a number.
fn millis(ms: &Value) -> Option<i64> {
    ms.as_i64().or_else(|| ms.as_f64().map(|f| f as i64))
}

/// Seconds east of UTC on this machine, read once; None when it cannot be read.
fn local_offset() -> Option<i64> {
    static OFFSET: OnceLock<Option<i64>> = OnceLock::new();
    *OFFSET.get_or_init(read_offset)
}

#[cfg(unix)]
fn read_offset() -> Option<i64> {
    let out = std::process::Command::new("date").arg("+%z").output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    let sign = match text.as_bytes().first().copied()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours: i64 = text.get(1..3)?.parse().ok()?;
    let minutes: i64 = text.get(3..5)?.parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

#[cfg(not(unix))]
fn read_offset() -> Option<i64> {
    None
}

/// (year, month, day) for a count of days since 1970-01-01 (proleptic Gregorian calendar).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `YYYY-MM-DD HH:MM` for epoch milliseconds at `offset` seconds east of UTC. With no offset the
/// time is UTC and marked with a trailing `Z`.
fn format_when(ms: i64, offset: Option<i64>) -> String {
    let local = ms.div_euclid(1000) + offset.unwrap_or(0);
    let (year, month, day) = civil(local.div_euclid(86_400));
    let minutes = local.rem_euclid(86_400) / 60;
    let zone = if offset.is_none() { "Z" } else { "" };
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}{zone}", minutes / 60, minutes % 60)
}

/// Local time as `YYYY-MM-DD HH:MM` for a hub time in epoch milliseconds. Text passes through as
/// it is, and null is "—".
pub fn when(ms: &Value) -> String {
    if let Some(text) = ms.as_str() {
        return text.to_string();
    }
    match millis(ms) {
        Some(ms) => format_when(ms, local_offset()),
        None => "—".to_string(),
    }
}

/// "just now", "5 min ago", "3 h ago" or "2 d ago", for `ms` seen from `now_ms`.
fn ago_at(ms: i64, now_ms: i64) -> String {
    let secs = (now_ms - ms).max(0) / 1000;
    match secs {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86_400),
    }
}

/// How long ago a hub time (epoch milliseconds) was; "—" for null.
pub fn ago(ms: &Value) -> String {
    match millis(ms) {
        Some(ms) => ago_at(ms, now_ms()),
        None => "—".to_string(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_save_keeps_the_pinned_identity() {
        let dir = std::env::temp_dir().join(format!("apz-cli-pins-{}", std::process::id()));
        let path = dir.join("cli.json");
        let before = r#"{"default":"home","hubs":{"home":{"url":"https://home.example:7700","token":"tok","pinned":{"fingerprint":"ab","since":"2026-10-13"}}}}"#;
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, before).unwrap();
        let mut config: Config = serde_json::from_str(before).unwrap();
        config.hubs.insert("work".into(), SavedHub { url: "https://work.example:7700".into(), token: None, pin: None, mcp_token: None });
        config.save_to(&path).unwrap();
        let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["hubs"]["home"]["pinned"]["fingerprint"], "ab");
        assert_eq!(after["hubs"]["work"]["url"], "https://work.example:7700");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn times_read_as_utc_with_a_z_without_a_local_offset() {
        assert_eq!(format_when(0, None), "1970-01-01 00:00Z");
        assert_eq!(format_when(1_791_811_353_718, None), "2026-10-12 13:22Z");
        assert_eq!(format_when(0, Some(7200)), "1970-01-01 02:00");
        assert_eq!(format_when(0, Some(-18_000)), "1969-12-31 19:00");
    }

    #[test]
    fn ago_reads_in_minutes_hours_and_days() {
        let now = 10 * 86_400_000;
        assert_eq!(ago_at(now, now), "just now");
        assert_eq!(ago_at(now + 1000, now), "just now");
        assert_eq!(ago_at(now - 5 * 60_000, now), "5 min ago");
        assert_eq!(ago_at(now - 3 * 3_600_000, now), "3 h ago");
        assert_eq!(ago_at(now - 2 * 86_400_000, now), "2 d ago");
        assert_eq!(when(&Value::Null), "—");
        assert_eq!(ago(&Value::Null), "—");
    }

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
