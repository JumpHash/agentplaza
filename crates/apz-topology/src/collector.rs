//! The lab VM's collector (PLAN 22.7): a process in the guest that serves OTLP for the topology's services (they export
//! to it on the bridge's gateway), relays what they export to the hub's receiver for the run, and ships the lines the
//! services write to their log files (`<container>.log`, see [`crate::run`]) as OTLP logs.
//!
//! The guest runs it as `apz-topo-collect <config.json>`; the config is [`Config`]. The services export OTLP/HTTP JSON
//! (the runner sets `OTEL_EXPORTER_OTLP_PROTOCOL=http/json`): the collector relays the bodies as they are. Its routes:
//! `POST /v1/logs|traces|metrics` (relayed to the upstream), `GET /healthz`. A relay that the hub refuses answers 502, so
//! the service's exporter retries.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::observe;

/// The header that carries the run's ingest token to the hub.
pub const TOKEN_HEADER: &str = "X-Apz-Lab-Token";
/// The most log lines the collector holds while the hub refuses them (the oldest go first).
pub const MAX_PENDING: usize = 20_000;
/// The largest body the collector relays.
const MAX_BODY: usize = 8 * 1024 * 1024;
/// The largest header block it reads.
const MAX_HEAD: usize = 16 * 1024;

/// A collector's configuration, written into the guest by the hub's plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where services export OTLP: `ip:port`, the bridge's gateway in the lab VM.
    pub listen: String,
    /// The run's receiver: `<upstream>/logs`, `/traces` and `/metrics` take what is shipped.
    pub upstream: String,
    /// The run's ingest token, sent with every relay.
    pub token: String,
    /// The directory of the services' log files (`<container>.log`).
    pub logs_dir: PathBuf,
    /// How often the log files are read and shipped, in milliseconds.
    #[serde(default = "default_interval")]
    pub interval_ms: u64,
}

fn default_interval() -> u64 {
    500
}

impl Config {
    /// The config is usable: a listen address, an upstream URL, a token and a log directory.
    pub fn check(&self) -> Result<(), String> {
        self.listen
            .parse::<SocketAddr>()
            .map_err(|_| format!("listen {:?} is not an address", self.listen))?;
        if !(self.upstream.starts_with("http://") || self.upstream.starts_with("https://")) {
            return Err(format!("upstream {:?} is not an http URL", self.upstream));
        }
        if self.token.trim().is_empty() {
            return Err("the collector needs the run's ingest token".into());
        }
        if self.interval_ms == 0 {
            return Err("interval_ms is 0".into());
        }
        Ok(())
    }
}

/// Tails the log files of a directory: each call returns the lines written since the last one, as (service, read time,
/// line). A file's service is its stem (the container's name). A last line still being written waits for its newline.
#[derive(Debug, Default)]
pub struct Tail {
    offsets: BTreeMap<PathBuf, u64>,
    partial: BTreeMap<PathBuf, String>,
}

impl Tail {
    /// The new lines of every `*.log` file in `dir`, read at `now_ms`.
    pub fn read(&mut self, dir: &Path, now_ms: u64) -> Vec<(String, u64, String)> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
            .collect();
        paths.sort();
        let mut out = Vec::new();
        for path in paths {
            let Some(service) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            let Ok(mut file) = File::open(&path) else {
                continue;
            };
            let len = file.metadata().map_or(0, |m| m.len());
            let mut offset = self.offsets.get(&path).copied().unwrap_or(0);
            if len < offset {
                // Truncated (or replaced): the file is read from its start again.
                offset = 0;
                self.partial.remove(&path);
            }
            if file.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let mut bytes = Vec::new();
            if file.read_to_end(&mut bytes).is_err() {
                continue;
            }
            self.offsets
                .insert(path.clone(), offset + bytes.len() as u64);
            let mut text = self.partial.remove(&path).unwrap_or_default();
            text.push_str(&String::from_utf8_lossy(&bytes));
            let mut pieces: Vec<&str> = text.split('\n').collect();
            let rest = pieces.pop().unwrap_or_default().to_string();
            for piece in pieces {
                out.push((
                    service.clone(),
                    now_ms,
                    piece.trim_end_matches('\r').to_string(),
                ));
            }
            if !rest.is_empty() {
                self.partial.insert(path, rest);
            }
        }
        out
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The relay to the run's receiver: a POST per signal, with the token.
#[derive(Clone)]
pub struct Relay {
    agent: ureq::Agent,
    upstream: String,
    token: String,
}

impl Relay {
    /// A relay to `upstream` for `token`, with a five-second timeout per request.
    pub fn new(upstream: &str, token: &str) -> Relay {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .build();
        Relay {
            agent: ureq::Agent::new_with_config(config),
            upstream: upstream.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    /// Posts `body` (JSON) to the receiver's `signal` (`logs`, `traces` or `metrics`); Ok when the hub answers 2xx.
    pub fn post(&self, signal: &str, body: &[u8]) -> Result<(), String> {
        let url = format!("{}/{signal}", self.upstream);
        let response = self
            .agent
            .post(&url)
            .header(TOKEN_HEADER, &self.token)
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| format!("{url}: {e}"))?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(format!("{url} answered {status}"))
        }
    }
}

/// The lines the collector holds for the hub, by service, until a shipment takes them (`count` is all of them).
#[derive(Default)]
struct Pending {
    by_service: BTreeMap<String, Vec<(u64, String)>>,
    count: usize,
}

impl Pending {
    fn add(&mut self, lines: Vec<(String, u64, String)>) {
        for (service, t_ms, line) in lines {
            self.by_service
                .entry(service)
                .or_default()
                .push((t_ms, line));
            self.count += 1;
        }
        while self.count > MAX_PENDING {
            let Some(service) = self.by_service.keys().next().cloned() else {
                break;
            };
            if let Some(lines) = self.by_service.get_mut(&service) {
                let drop = lines.len().min(self.count - MAX_PENDING);
                lines.drain(..drop);
                self.count -= drop;
            }
            self.by_service.retain(|_, lines| !lines.is_empty());
        }
    }

    /// Ships what is held: one OTLP logs request per service. A refused request keeps its lines for the next round.
    fn ship(&mut self, relay: &Relay) {
        let held = std::mem::take(&mut self.by_service);
        for (service, lines) in held {
            let body = observe::logs_otlp(&service, &lines);
            let bytes = serde_json::to_vec(&body).unwrap_or_default();
            if relay.post("logs", &bytes).is_err() {
                // The lines wait ahead of any that arrived since.
                let mut older = lines;
                older.extend(self.by_service.remove(&service).unwrap_or_default());
                self.by_service.insert(service, older);
            } else {
                self.count -= lines.len();
            }
        }
    }
}

/// Runs the collector until the process ends: it listens for OTLP, and ships the log files on its interval.
pub fn run(config: &Config) -> Result<(), String> {
    config.check()?;
    let listener =
        TcpListener::bind(&config.listen).map_err(|e| format!("listen {}: {e}", config.listen))?;
    let relay = Relay::new(&config.upstream, &config.token);
    let pending = Arc::new(Mutex::new(Pending::default()));
    {
        let pending = Arc::clone(&pending);
        let relay = relay.clone();
        let logs_dir = config.logs_dir.clone();
        let interval = Duration::from_millis(config.interval_ms);
        thread::spawn(move || {
            let mut tail = Tail::default();
            loop {
                thread::sleep(interval);
                let read = tail.read(&logs_dir, now_ms());
                let mut held = lock(&pending);
                held.add(read);
                held.ship(&relay);
            }
        });
    }
    for stream in listener.incoming().flatten() {
        let relay = relay.clone();
        thread::spawn(move || {
            let _ = serve(stream, &relay);
        });
    }
    Ok(())
}

fn lock(pending: &Mutex<Pending>) -> std::sync::MutexGuard<'_, Pending> {
    pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One HTTP request the collector reads: its method, its path, the ingest token it carries (empty if none) and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub token: String,
    pub body: Vec<u8>,
}

/// Reads one request from a stream: the head up to its blank line, and a `Content-Length` body.
pub fn read_request(reader: &mut impl BufRead) -> Result<Request, String> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the request ends before its head".into());
        }
        head.push_str(&line);
        if head.len() > MAX_HEAD {
            return Err("the request head is too long".into());
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut lines = head.lines();
    let request_line = lines.next().ok_or("no request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("no method")?.to_string();
    let path = parts.next().ok_or("no path")?.to_string();
    let mut length = 0usize;
    let mut token = String::new();
    for header in lines {
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().map_err(|_| "a bad Content-Length")?;
            } else if name.eq_ignore_ascii_case(TOKEN_HEADER) {
                token = value.trim().to_string();
            }
        }
    }
    if length > MAX_BODY {
        return Err("the body is too large".into());
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Request {
        method,
        path,
        token,
        body,
    })
}

/// The answer to a request: its status, and its body.
fn answer(request: &Request, relay: &Relay) -> (u16, &'static str, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/healthz") => (200, "OK", "ok".into()),
        ("POST", path) => {
            let Some(signal) = path
                .strip_prefix("/v1/")
                .filter(|s| matches!(*s, "logs" | "traces" | "metrics"))
            else {
                return (404, "Not Found", "no such signal".into());
            };
            match relay.post(signal, &request.body) {
                Ok(()) => (200, "OK", "{}".into()),
                Err(why) => (502, "Bad Gateway", why),
            }
        }
        _ => (404, "Not Found", "no such route".into()),
    }
}

fn serve(mut stream: TcpStream, relay: &Relay) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let (status, reason, body) = match read_request(&mut reader) {
        Ok(request) => answer(&request, relay),
        Err(why) => (400, "Bad Request", why),
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|e| e.to_string())
}

/// Starts the collector binary with its config file, and waits until it serves `/healthz` (a TCP connect is enough
/// to know it listens). The child ends when it is dropped.
pub fn spawn(binary: &Path, config: &Path, listen: &str, wait: Duration) -> Result<Child, String> {
    let child = Command::new(binary)
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", binary.display()))?;
    let addr: SocketAddr = listen
        .parse()
        .map_err(|_| format!("listen {listen:?} is not an address"))?;
    let deadline = Instant::now() + wait;
    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
            return Ok(child);
        }
        if Instant::now() >= deadline {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the collector is not listening on {listen}"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_reads_new_lines_once_and_waits_for_a_partial_one() {
        let dir =
            std::env::temp_dir().join(format!("apz-tail-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("api-1.log");
        std::fs::write(&log, "first\nsecond\npar").unwrap();
        let mut tail = Tail::default();
        let lines = tail.read(&dir, 5);
        assert_eq!(
            lines,
            [
                ("api-1".to_string(), 5, "first".to_string()),
                ("api-1".to_string(), 5, "second".to_string())
            ]
        );
        assert!(tail.read(&dir, 6).is_empty(), "nothing new");
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        file.write_all(b"tial\r\n").unwrap();
        let more = tail.read(&dir, 7);
        assert_eq!(more, [("api-1".to_string(), 7, "partial".to_string())]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_request_is_read_with_its_content_length_body() {
        let raw = b"POST /v1/traces HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{}";
        let request = read_request(&mut BufReader::new(&raw[..])).unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/traces");
        assert!(request.token.is_empty());
        assert_eq!(request.body, b"{}");
        let tokened =
            b"POST /v1/logs HTTP/1.1\r\nX-Apz-Lab-Token: tok\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            read_request(&mut BufReader::new(&tokened[..]))
                .unwrap()
                .token,
            "tok"
        );
        assert!(read_request(&mut BufReader::new(&b"GET"[..])).is_err());
    }

    #[test]
    fn a_config_is_checked_before_the_collector_listens() {
        let good = Config {
            listen: "127.0.0.1:4318".into(),
            upstream: "http://hub/v1/test-lab/runs/q1/otlp".into(),
            token: "t".into(),
            logs_dir: PathBuf::from("/var/log/apz-topo"),
            interval_ms: 500,
        };
        assert!(good.check().is_ok());
        assert!(
            Config {
                listen: "gateway".into(),
                ..good.clone()
            }
            .check()
            .is_err()
        );
        assert!(
            Config {
                upstream: "ftp://x".into(),
                ..good.clone()
            }
            .check()
            .is_err()
        );
        assert!(
            Config {
                token: " ".into(),
                ..good.clone()
            }
            .check()
            .is_err()
        );
        assert!(
            Config {
                interval_ms: 0,
                ..good
            }
            .check()
            .is_err()
        );
    }
}
