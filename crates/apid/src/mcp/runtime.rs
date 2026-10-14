//! The runtime: one instance per (server, node). An instance starts on first use, stops after the server's
//! `idle_stop_min` without use, restarts with backoff (1 s, 2 s, 4 s … 60 s) and gives up after five
//! failures in ten minutes. Every output, error and log line is scrubbed of the secret values the
//! instance was started with.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::client::{Client, Error, ServerInfo, ToolInfo};
use super::config::{McpServer, Setting, Transport as Wire, volume_name};
use super::egress::{Egress, Lease, proxy_env};
use super::http::{HttpTransport, Policy, check_endpoint};
use super::stdio::{LaunchSpec, Launcher, StdioTransport, push_capped};
use super::{McpSecrets, OnBehalf};

/// Failures in this window count toward giving up.
pub const FAILURE_WINDOW_MS: u64 = 10 * 60_000;
/// Failures in the window that make the runtime give up.
pub const MAX_FAILURES: usize = 5;
/// The longest a restart waits.
pub const MAX_BACKOFF_MS: u64 = 60_000;
/// How long a handshake may take.
const HANDSHAKE: Duration = Duration::from_secs(30);
/// Stderr lines kept per instance.
const LOG_LINES: usize = 200;
/// The text that stands in for a secret.
pub const REDACTED: &str = "[redacted]";

/// Where an instance stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Stopped,
    Running,
    Error,
}

/// One stderr line, scrubbed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogLine {
    pub ts_ms: u64,
    pub stream: &'static str,
    pub line: String,
}

/// What a slot knows. Readable while a call runs.
#[derive(Default)]
struct State {
    status: Status,
    last_used_ms: u64,
    idle_ms: u64,
    last_error: Option<String>,
    failures: VecDeque<u64>,
    next_attempt_ms: u64,
    gave_up: bool,
    server_info: Option<ServerInfo>,
    secrets: Vec<String>,
    logs: VecDeque<LogLine>,
}

/// One instance: its client (held for the length of a call) and its state.
pub struct Slot {
    client: Mutex<Option<Client>>,
    state: Mutex<State>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Who asks, and when: what the secrets resolve for, and the clock.
pub struct Ctx<'a> {
    pub secrets: &'a dyn McpSecrets,
    pub behalf: OnBehalf,
    /// The node the instance runs for (`hub` for the hub's own work).
    pub node: String,
    /// The fetch blocklist, checked for http endpoints.
    pub blocklist: Vec<String>,
    pub now: u64,
}

/// The shaped reply to a tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub text: String,
    pub is_error: bool,
    /// Image and audio content, decoded.
    pub media: Vec<Media>,
    /// The fresh tool listing, when this call started the instance.
    pub tools: Option<Vec<ToolInfo>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A test of a server: what it answered.
#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub server_info: ServerInfo,
    pub tools: Vec<ToolInfo>,
    pub resources: Vec<Value>,
    pub prompts: Vec<Value>,
}

/// The status a server shows, over its instances.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub status: Status,
    pub last_used_ms: u64,
    pub last_error: Option<String>,
    pub server_info: Option<ServerInfo>,
}

/// Every instance, by server id and node.
pub struct Runtime {
    launcher: Arc<dyn Launcher>,
    slots: Mutex<BTreeMap<(String, String), Arc<Slot>>>,
    /// The egress proxy stdio containers reach the network through. None runs containers without one (tests).
    egress: Option<Arc<Egress>>,
}

/// The backoff before restart number `failures` (1 s, 2 s, 4 s … capped at 60 s).
pub fn backoff_ms(failures: usize) -> u64 {
    if failures == 0 {
        return 0;
    }
    let shift = (failures - 1).min(16) as u32;
    (1000u64 << shift).min(MAX_BACKOFF_MS)
}

/// Whether an instance idle since `last_used_ms` should stop now.
pub fn idle_expired(last_used_ms: u64, idle_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_used_ms) >= idle_ms
}

/// The forms a secret takes in text: the value itself, its base64, and its URL-encoded form.
pub fn forms(secret: &str) -> Vec<String> {
    let mut out = vec![secret.to_string()];
    let b64 = crate::connectors::base64(secret.as_bytes());
    if b64.len() >= 4 {
        out.push(b64);
    }
    let encoded = crate::connectors::encode(secret);
    if encoded != secret {
        out.push(encoded);
    }
    out.retain(|f| !f.is_empty());
    out
}

/// Every secret form replaced by [`REDACTED`]. Longest forms go first, so a value inside a longer one is
/// still caught.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut all: Vec<String> = secrets.iter().flat_map(|s| forms(s)).collect();
    all.sort_by_key(|f| std::cmp::Reverse(f.len()));
    all.dedup();
    let mut out = text.to_string();
    for form in all {
        if out.contains(&form) {
            out = out.replace(&form, REDACTED);
        }
    }
    out
}

/// Cuts `text` to `cap` bytes at a character boundary, saying so.
fn cap_text(text: String, cap: usize) -> String {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated at {} KiB]", &text[..end], cap / 1024)
}

/// Decodes standard base64 (padding optional, whitespace ignored). None when the input is not base64.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for byte in text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
    {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Turns a `tools/call` result into text and media, scrubbed and capped.
pub fn shape(result: &Value, secrets: &[String], cap: usize) -> (String, bool, Vec<Media>) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts = Vec::new();
    let mut media = Vec::new();
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => parts.push(
                item.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            "resource" => match item.pointer("/resource/text").and_then(Value::as_str) {
                Some(text) => parts.push(text.to_string()),
                None => parts.push(format!(
                    "[resource {}]",
                    item.pointer("/resource/uri")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                )),
            },
            kind @ ("image" | "audio") => {
                let mime = item
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream")
                    .to_string();
                match item
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(decode_base64)
                {
                    Some(bytes) => {
                        parts.push(format!(
                            "[{kind} {mime}, {} bytes, saved to the file store]",
                            bytes.len()
                        ));
                        media.push(Media { mime, bytes });
                    }
                    None => parts.push(format!("[unreadable {kind}]")),
                }
            }
            other => parts.push(format!("[{other} content]")),
        }
    }
    let text = redact(&parts.join("\n"), secrets);
    (cap_text(text, cap), is_error, media)
}

impl Runtime {
    /// A runtime whose stdio servers get no network proxy: their launches are not isolated (tests only).
    #[cfg(test)]
    pub fn new(launcher: Arc<dyn Launcher>) -> Runtime {
        Runtime {
            launcher,
            slots: Mutex::new(BTreeMap::new()),
            egress: None,
        }
    }

    /// A runtime whose stdio servers reach the network only through `egress`.
    pub fn with_egress(launcher: Arc<dyn Launcher>, egress: Arc<Egress>) -> Runtime {
        Runtime {
            launcher,
            slots: Mutex::new(BTreeMap::new()),
            egress: Some(egress),
        }
    }

    fn slot(&self, server: &str, node: &str) -> Arc<Slot> {
        let mut slots = lock(&self.slots);
        Arc::clone(
            slots
                .entry((server.to_string(), node.to_string()))
                .or_insert_with(|| {
                    Arc::new(Slot {
                        client: Mutex::new(None),
                        state: Mutex::new(State::default()),
                    })
                }),
        )
    }

    /// Calls `tool` on `server` for `ctx.node`, starting the instance if needed.
    pub fn call(
        &self,
        server: &McpServer,
        ctx: &Ctx,
        tool: &str,
        args: Value,
    ) -> Result<Output, String> {
        let slot = self.slot(&server.id, &ctx.node);
        let timeout = Duration::from_secs(server.limits.timeout_s);
        let mut fresh = None;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut guard = lock(&slot.client);
            let started = self.ensure(server, &slot, &mut guard, ctx)?;
            if started.is_some() {
                fresh = started;
            }
            let secrets = lock(&slot.state).secrets.clone();
            lock(&slot.state).idle_ms = server.limits.idle_stop_min * 60_000;
            lock(&slot.state).last_used_ms = ctx.now;
            let client = guard.as_mut().ok_or("the server is not running")?;
            match client.call_tool(tool, args.clone(), timeout) {
                Ok(result) => {
                    let (text, is_error, media) = shape(
                        &result,
                        &secrets,
                        server.limits.max_output_kb as usize * 1024,
                    );
                    return Ok(Output {
                        text,
                        is_error,
                        media,
                        tools: fresh,
                    });
                }
                Err(Error::SessionLost) if attempt == 1 => {
                    // The HTTP session expired: start a new one and send the call again, once.
                    take_client(&mut guard);
                    continue;
                }
                Err(Error::Rpc { code, message }) => {
                    return Err(redact(&format!("server error {code}: {message}"), &secrets));
                }
                Err(Error::Timeout) => {
                    take_client(&mut guard);
                    let why = format!("timed out after {} s", server.limits.timeout_s);
                    let mut st = lock(&slot.state);
                    st.status = Status::Stopped;
                    st.last_error = Some(why.clone());
                    return Err(why);
                }
                Err(other) => {
                    take_client(&mut guard);
                    let why = redact(&other.to_string(), &secrets);
                    record_failure(&mut lock(&slot.state), ctx.now, &why);
                    return Err(format!("{} failed: {why}", server.id));
                }
            }
        }
    }

    /// Starts the instance (or finds it running) and lists its tools, resources and prompts. The probe
    /// does not keep the instance if the caller stops it afterwards.
    pub fn probe(&self, server: &McpServer, ctx: &Ctx) -> Result<Probe, String> {
        let slot = self.slot(&server.id, &ctx.node);
        let timeout = Duration::from_secs(server.limits.timeout_s);
        let mut guard = lock(&slot.client);
        let started = self.ensure(server, &slot, &mut guard, ctx)?;
        let secrets = lock(&slot.state).secrets.clone();
        let info = lock(&slot.state).server_info.clone().unwrap_or(ServerInfo {
            name: String::new(),
            version: String::new(),
            protocol: String::new(),
        });
        let client = guard.as_mut().ok_or("the server is not running")?;
        let tools = match started {
            Some(tools) => tools,
            None => client
                .list_tools(timeout)
                .map_err(|e| redact(&e.to_string(), &secrets))?,
        };
        let resources = client.list_resources(timeout).unwrap_or_default();
        let prompts = client.list_prompts(timeout).unwrap_or_default();
        Ok(Probe {
            server_info: info,
            tools,
            resources,
            prompts,
        })
    }

    /// Starts the instance if it is not running. Returns the tool listing when it did start.
    fn ensure(
        &self,
        server: &McpServer,
        slot: &Arc<Slot>,
        guard: &mut Option<Client>,
        ctx: &Ctx,
    ) -> Result<Option<Vec<ToolInfo>>, String> {
        if let Some(client) = guard.as_mut() {
            if client.alive() {
                return Ok(None);
            }
            take_client(guard);
            record_failure(&mut lock(&slot.state), ctx.now, "the server exited");
        }
        {
            let st = lock(&slot.state);
            if st.gave_up {
                return Err(format!(
                    "{} gave up after {MAX_FAILURES} failures in ten minutes: {}. Start it again from the console",
                    server.id,
                    st.last_error.clone().unwrap_or_default()
                ));
            }
            if ctx.now < st.next_attempt_ms {
                let wait = st.next_attempt_ms.saturating_sub(ctx.now).div_ceil(1000);
                return Err(format!(
                    "{} restarts after a failure, in {wait} s: {}",
                    server.id,
                    st.last_error.clone().unwrap_or_default()
                ));
            }
        }
        // Secrets resolve before anything starts; a refused resolution is not a failure of the server.
        let resolved = match resolve(server, ctx) {
            Ok(resolved) => resolved,
            Err(why) => {
                record_failure(&mut lock(&slot.state), ctx.now, &why);
                return Err(format!("{} did not start: {why}", server.id));
            }
        };
        lock(&slot.state).secrets = resolved.secrets.clone();
        let transport: Box<dyn super::client::Transport> = match &server.transport {
            Wire::Stdio {
                image,
                command,
                workdir,
            } => {
                let lease: Option<Lease> = match &self.egress {
                    Some(egress) => {
                        match egress.attach(&server.id, server.allow_private, &ctx.blocklist) {
                            Ok(lease) => Some(lease),
                            Err(why) => {
                                record_failure(&mut lock(&slot.state), ctx.now, &why);
                                return Err(format!("{} did not start: {why}", server.id));
                            }
                        }
                    }
                    None => None,
                };
                let mut env = resolved.env;
                if let Some(lease) = &lease {
                    env.extend(proxy_env(lease.proxy()));
                }
                let spec = LaunchSpec {
                    server: server.id.clone(),
                    image: image.clone(),
                    command: command.clone(),
                    workdir: workdir.clone(),
                    env,
                    ip: lease.as_ref().map(Lease::ip),
                    volume: server.storage.volume.then(|| volume_name(&server.id)),
                };
                let child = match self.launcher.launch(&spec) {
                    Ok(child) => child,
                    Err(why) => {
                        record_failure(
                            &mut lock(&slot.state),
                            ctx.now,
                            &redact(&why, &resolved.secrets),
                        );
                        return Err(format!("{} did not start: {why}", server.id));
                    }
                };
                let sink_slot = Arc::clone(slot);
                let on_stderr: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line: String| {
                    let mut st = lock(&sink_slot.state);
                    let line = redact(&line, &st.secrets.clone());
                    push_capped(
                        &mut st.logs,
                        LogLine {
                            ts_ms: apz_plaza::now_ms(),
                            stream: "stderr",
                            line,
                        },
                        LOG_LINES,
                    );
                });
                match StdioTransport::new(child, on_stderr, lease) {
                    Ok(transport) => Box::new(transport),
                    Err(why) => {
                        record_failure(&mut lock(&slot.state), ctx.now, &why);
                        return Err(format!("{} did not start: {why}", server.id));
                    }
                }
            }
            Wire::Http { url } => {
                if let Err(why) = check_endpoint(url, server.allow_private, &ctx.blocklist) {
                    record_failure(&mut lock(&slot.state), ctx.now, &why);
                    return Err(format!("{}: {why}", server.id));
                }
                let policy = Policy {
                    allow_private: server.allow_private,
                    blocklist: ctx.blocklist.clone(),
                };
                Box::new(HttpTransport::new(
                    url.clone(),
                    resolved.headers,
                    resolved.secrets.clone(),
                    policy,
                ))
            }
        };
        let mut client = Client::new(transport);
        let handshake = client
            .initialize("agentplaza", env!("CARGO_PKG_VERSION"), HANDSHAKE)
            .and_then(|info| {
                let tools = client.list_tools(HANDSHAKE)?;
                Ok((info, tools))
            });
        match handshake {
            Ok((info, tools)) => {
                let mut st = lock(&slot.state);
                st.status = Status::Running;
                st.server_info = Some(info);
                st.last_error = None;
                st.next_attempt_ms = 0;
                *guard = Some(client);
                Ok(Some(tools))
            }
            Err(err) => {
                client.close();
                let why = redact(&err.to_string(), &resolved.secrets);
                let last = lock(&slot.state).logs.back().map(|l| l.line.clone());
                let detail = match last {
                    Some(line) if !line.is_empty() => format!("{why} (stderr: {line})"),
                    _ => why,
                };
                record_failure(&mut lock(&slot.state), ctx.now, &detail);
                Err(format!("{} did not start: {detail}", server.id))
            }
        }
    }

    /// Stops every instance of `server_id` (its child is killed; an http session is forgotten).
    pub fn stop(&self, server_id: &str) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
            .collect();
        for slot in slots {
            take_client(&mut lock(&slot.client));
            let mut st = lock(&slot.state);
            if st.status == Status::Running {
                st.status = Status::Stopped;
            }
        }
    }

    /// Clears the failures of `server_id`, so an owner's start can try again after giving up.
    pub fn reset(&self, server_id: &str) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
            .collect();
        for slot in slots {
            let mut st = lock(&slot.state);
            st.failures.clear();
            st.gave_up = false;
            st.next_attempt_ms = 0;
        }
    }

    /// Stops the instances idle longer than their server allows. An instance in use is left alone.
    pub fn reap_idle(&self, now_ms: u64) {
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().cloned().collect();
        for slot in slots {
            let Ok(mut guard) = slot.client.try_lock() else {
                continue;
            };
            if guard.is_none() {
                continue;
            }
            let mut st = lock(&slot.state);
            if idle_expired(st.last_used_ms, st.idle_ms, now_ms) {
                *guard = None;
                st.status = Status::Stopped;
            }
        }
    }

    /// The status of `server_id` over its instances.
    pub fn summary(&self, server_id: &str) -> Summary {
        let mut out = Summary::default();
        let (mut running, mut failed) = (false, false);
        for slot in lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
        {
            let st = lock(&slot.state);
            running |= st.status == Status::Running;
            failed |= st.status == Status::Error || st.gave_up;
            out.last_used_ms = out.last_used_ms.max(st.last_used_ms);
            if out.last_error.is_none() {
                out.last_error = st.last_error.clone();
            }
            if out.server_info.is_none() {
                out.server_info = st.server_info.clone();
            }
        }
        out.status = if running {
            Status::Running
        } else if failed {
            Status::Error
        } else {
            Status::Stopped
        };
        out
    }

    /// The last stderr lines of `server_id`, oldest first, at most `limit`.
    pub fn logs(&self, server_id: &str, limit: usize) -> Vec<LogLine> {
        let mut lines: Vec<LogLine> = Vec::new();
        for slot in lock(&self.slots)
            .iter()
            .filter(|((id, _), _)| id == server_id)
            .map(|(_, s)| Arc::clone(s))
        {
            lines.extend(lock(&slot.state).logs.iter().cloned());
        }
        lines.sort_by_key(|l| l.ts_ms);
        let skip = lines.len().saturating_sub(limit);
        lines.split_off(skip)
    }
}

/// Drops the client: its child is killed when its transport drops.
fn take_client(guard: &mut Option<Client>) {
    if let Some(mut client) = guard.take() {
        client.close();
    }
}

/// Counts a failure at `now`, and sets the backoff, or gives up at the limit.
fn record_failure(st: &mut State, now: u64, why: &str) {
    st.failures
        .retain(|t| now.saturating_sub(*t) < FAILURE_WINDOW_MS);
    st.failures.push_back(now);
    st.status = Status::Error;
    st.last_error = Some(why.to_string());
    if st.failures.len() >= MAX_FAILURES {
        st.gave_up = true;
    }
    st.next_attempt_ms = now + backoff_ms(st.failures.len());
}

/// The env and headers a server gets, with secrets resolved for `ctx`.
struct Resolved {
    env: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    /// The secret values (for scrubbing), never literals.
    secrets: Vec<String>,
}

fn resolve(server: &McpServer, ctx: &Ctx) -> Result<Resolved, String> {
    let mut out = Resolved {
        env: Vec::new(),
        headers: Vec::new(),
        secrets: Vec::new(),
    };
    for (name, setting) in &server.env {
        let value = value_of(setting, server, ctx, &mut out.secrets)?;
        out.env.push((name.clone(), value));
    }
    for (name, setting) in &server.headers {
        let value = value_of(setting, server, ctx, &mut out.secrets)?;
        out.headers.push((name.clone(), value));
    }
    Ok(out)
}

/// One setting's value: the literal, or the vault's value for the server (with a header's `format` applied).
fn value_of(
    setting: &Setting,
    server: &McpServer,
    ctx: &Ctx,
    secrets: &mut Vec<String>,
) -> Result<String, String> {
    match setting {
        Setting::Value { value } => Ok(value.clone()),
        Setting::Secret { secret, format } => {
            let raw = ctx.secrets.resolve(secret, &server.id, &ctx.behalf)?;
            secrets.push(raw.expose().to_string());
            Ok(match format {
                Some(format) => format.replace("{}", raw.expose()),
                None => raw.expose().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_one_second_and_stops_at_a_minute() {
        let steps: Vec<u64> = (1..=8).map(backoff_ms).collect();
        assert_eq!(
            steps,
            vec![1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000]
        );
    }

    #[test]
    fn an_instance_stops_after_its_idle_minutes() {
        assert!(!idle_expired(1_000, 600_000, 600_999));
        assert!(idle_expired(1_000, 600_000, 601_000));
    }

    #[test]
    fn redaction_covers_the_value_and_its_base64_and_url_forms() {
        let secret = "sk-SENTINEL/+&=value".to_string();
        let b64 = crate::connectors::base64(secret.as_bytes());
        let text = format!(
            "raw {secret}; b64 {b64}; url {}",
            crate::connectors::encode(&secret)
        );
        let clean = redact(&text, std::slice::from_ref(&secret));
        assert!(!clean.contains("SENTINEL"), "{clean}");
        assert_eq!(clean.matches(REDACTED).count(), 3);
    }

    #[test]
    fn a_secret_inside_a_longer_secret_is_still_caught() {
        let secrets = vec!["abcdef".to_string(), "bcd".to_string()];
        assert!(!redact("xxabcdefyy", &secrets).contains("abc"));
    }

    #[test]
    fn output_is_redacted_before_it_is_capped() {
        let secret = "SENTINEL-SECRET-1234".to_string();
        let result = serde_json::json!({"content": [{"type": "text", "text": format!("prefix {secret} tail")}], "isError": false});
        let (text, is_error, media) = shape(&result, std::slice::from_ref(&secret), 1024);
        assert!(!text.contains("SENTINEL") && text.contains(REDACTED));
        assert!(!is_error && media.is_empty());
    }

    #[test]
    fn text_is_joined_capped_and_marked() {
        let big = "x".repeat(3000);
        let result = serde_json::json!({"content": [{"type": "text", "text": "a"}, {"type": "text", "text": big}], "isError": true});
        let (text, is_error, _) = shape(&result, &[], 1024);
        assert!(is_error);
        assert!(text.starts_with("a\nxxx"));
        assert!(text.ends_with("[truncated at 1 KiB]"));
    }

    #[test]
    fn images_are_decoded_for_the_file_store() {
        let result = serde_json::json!({"content": [{"type": "image", "mimeType": "image/png", "data": "iVBORw0K"}]});
        let (text, _, media) = shape(&result, &[], 1024);
        assert_eq!(media.len(), 1);
        assert_eq!(&media[0].bytes[..4], &[0x89, b'P', b'N', b'G']);
        assert!(text.contains("image/png"));
    }

    #[test]
    fn base64_decodes_and_refuses_garbage() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello");
        assert!(decode_base64("!!").is_none());
    }

    #[test]
    fn a_failure_window_gives_up_at_five() {
        let mut st = State::default();
        for i in 0..4 {
            record_failure(&mut st, i * 1000, "boom");
        }
        assert!(!st.gave_up);
        record_failure(&mut st, 5000, "boom");
        assert!(st.gave_up);
    }

    #[test]
    fn old_failures_fall_out_of_the_window() {
        let mut st = State::default();
        for i in 0..4 {
            record_failure(&mut st, i, "boom");
        }
        record_failure(&mut st, FAILURE_WINDOW_MS + 10, "boom");
        assert!(!st.gave_up);
        assert_eq!(st.failures.len(), 1);
    }
}
