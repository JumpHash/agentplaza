//! The node daemon's JSON API, spoken over a tiny blocking HTTP/1.1 client.
//!
//! The dashboard reaches `noded` on loopback only, so this is deliberately not
//! a general HTTP client: one request per connection, no redirects, no
//! chunked bodies, no TLS, no async runtime, no client dependency — just a
//! `TcpStream` per call, the same shape `crates/noded/tests/common/mod.rs`
//! uses to test the daemon.
//!
//! The types here mirror `noded`'s views (`registry::RunView`, `VmView`,
//! `RunsView`) and its create request (`registry::CreateRun`); the states are
//! enums so an unknown state from a newer daemon cannot fail the parse, it
//! renders as [`RunState::Unknown`].

use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// The port `noded` listens on unless `--url` says otherwise.
pub const DEFAULT_PORT: u16 = 8899;

/// How long one request may take, end to end.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The most a status line and its headers may occupy, together.
const MAX_HEAD: usize = 64 * 1024;

/// The most a response body may occupy. A run's serial transcript is the
/// largest thing the daemon serves, and it is bounded in memory there, so this
/// is a ceiling, not a budget.
const MAX_BODY: usize = 16 * 1024 * 1024;

/// A run's state, as `core.runs.state` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    /// The run exists and its VMs are being booted.
    Starting,
    /// Every VM of the run is up.
    Running,
    /// The run ended because its VMs stopped by themselves.
    Done,
    /// The run ended because its VMs could not be booted.
    Failed,
    /// The run ended because the host killed its VMs.
    Killed,
    /// A state this dashboard does not know; rendered as-is, never a parse error.
    #[serde(other)]
    Unknown,
}

impl RunState {
    /// The name the API uses.
    pub fn label(self) -> &'static str {
        match self {
            RunState::Starting => "starting",
            RunState::Running => "running",
            RunState::Done => "done",
            RunState::Failed => "failed",
            RunState::Killed => "killed",
            RunState::Unknown => "unknown",
        }
    }

    /// Whether the run has not ended, so killing it is still an action.
    pub fn is_live(self) -> bool {
        matches!(self, RunState::Starting | RunState::Running)
    }

    /// The word a stamp presses into the register: the name, set in caps.
    pub fn stamp_label(self) -> &'static str {
        match self {
            RunState::Starting => "STARTING",
            RunState::Running => "RUNNING",
            RunState::Done => "DONE",
            RunState::Failed => "FAILED",
            RunState::Killed => "KILLED",
            RunState::Unknown => "UNKNOWN",
        }
    }
}

/// A VM's state, as the API names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    /// The VM is up.
    Running,
    /// The VM stopped by itself.
    Exited,
    /// The VM was stopped by the host.
    Killed,
    /// A state this dashboard does not know.
    #[serde(other)]
    Unknown,
}

impl VmState {
    /// The name the API uses.
    pub fn label(self) -> &'static str {
        match self {
            VmState::Running => "running",
            VmState::Exited => "exited",
            VmState::Killed => "killed",
            VmState::Unknown => "unknown",
        }
    }

    /// The word a stamp presses into the register.
    pub fn stamp_label(self) -> &'static str {
        match self {
            VmState::Running => "UP",
            VmState::Exited => "EXITED",
            VmState::Killed => "KILLED",
            VmState::Unknown => "UNKNOWN",
        }
    }
}

/// One VM of a run, as `GET /api/v0/runs/{id}` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Vm {
    /// The VM's index among the run's VMs.
    pub id: u32,
    /// The VM's vsock guest CID; absent when the run has no vsock devices.
    #[serde(default)]
    pub guest_cid: Option<u64>,
    /// The VM's state.
    pub state: VmState,
    /// The VM's serial capture; present only in a single-run GET.
    #[serde(default)]
    pub serial: Option<String>,
}

/// A run, as the API reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Run {
    /// The run's id (`r<seq>`).
    pub id: String,
    /// The run's state.
    pub state: RunState,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// Heartbeats before the guest reboots itself, or `null`.
    #[serde(default)]
    pub beats: Option<u32>,
    /// When the run was created, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// When the run ended, in milliseconds since the Unix epoch, or `null`.
    #[serde(default)]
    pub ended_at: Option<u64>,
    /// The run's VMs.
    pub vms: Vec<Vm>,
}

impl Run {
    /// How many of the run's VMs are still up.
    pub fn live_vms(&self) -> usize {
        self.vms
            .iter()
            .filter(|vm| vm.state == VmState::Running)
            .count()
    }

    /// The base guest CID the run's vsock devices were given, when it has any.
    pub fn base_cid(&self) -> Option<u64> {
        self.vms.first().and_then(|vm| vm.guest_cid)
    }

    /// The run's serial transcript: the first VM's capture, or empty when the
    /// view carries none (the list endpoint omits captures).
    pub fn serial(&self) -> &str {
        self.vms
            .first()
            .and_then(|vm| vm.serial.as_deref())
            .unwrap_or("")
    }
}

/// `GET /api/v0/runs`: the list endpoint's body.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunsPage {
    /// Every run, oldest first.
    pub runs: Vec<Run>,
}

/// `POST /api/v0/runs`: the body sends exactly the fields `noded` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRun {
    /// How many VMs to boot.
    pub n: u32,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// Heartbeats before the guest reboots itself; `null` runs the guest
    /// long-lived.
    pub beats: Option<u32>,
    /// Base guest CID for the run's vsock devices; `null` boots without vsock.
    pub vsock_cid: Option<u64>,
}

/// Where the daemon is. Loopback only, by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The literal loopback host: always `127.0.0.1`.
    host: String,
    /// The daemon's port.
    port: u16,
}

impl Endpoint {
    /// The default endpoint: the daemon on `127.0.0.1:8899`.
    pub fn loopback() -> Self {
        Self {
            host: Ipv4Addr::LOCALHOST.to_string(),
            port: DEFAULT_PORT,
        }
    }

    /// An endpoint on a loopback host, rejecting anything else.
    ///
    /// Exact matching only: `127.0.0.1` (or `localhost`, which is normalized
    /// to it). No other name, address, or lookalike is accepted, and the host
    /// is never resolved through DNS.
    pub fn loopback_at(host: &str, port: u16) -> Result<Self, ApiError> {
        let host = match host {
            "127.0.0.1" => Ipv4Addr::LOCALHOST.to_string(),
            "localhost" => Ipv4Addr::LOCALHOST.to_string(),
            other => {
                return Err(ApiError::Config(format!(
                    "{other:?} is not a loopback host: the dashboard reaches the node \
                     daemon on 127.0.0.1 only"
                )));
            }
        };
        if port == 0 {
            return Err(ApiError::Config(
                "port 0 is not a port to reach the node daemon on".to_string(),
            ));
        }
        Ok(Self { host, port })
    }

    /// Parses `http://127.0.0.1[:port]` (a bare `host[:port]` is accepted too).
    pub fn from_url(url: &str) -> Result<Self, ApiError> {
        let url = url.trim();
        let rest = match url.strip_prefix("http://") {
            Some(rest) => rest,
            None if url.contains("://") => {
                return Err(ApiError::Config(format!(
                    "{url:?} is not an http:// URL"
                )));
            }
            None => url,
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(ApiError::Config(format!(
                "{url:?} carries a path, query or fragment; the daemon serves its API at the root"
            )));
        }
        let (host, port) = match rest.rsplit_once(':') {
            Some((host, port)) => {
                let port = port.parse::<u16>().map_err(|_| {
                    ApiError::Config(format!("{port:?} is not a port"))
                })?;
                (host, port)
            }
            None => (rest, DEFAULT_PORT),
        };
        Self::loopback_at(host, port)
    }

    /// The endpoint's host, always `127.0.0.1`.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The endpoint's port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The endpoint as a URL, for the header and error messages.
    pub fn url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

impl Default for Endpoint {
    fn default() -> Self {
        Self::loopback()
    }
}

/// Why a call did not produce a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The endpoint or a request parameter cannot be used as given.
    Config(String),
    /// The daemon could not be reached.
    Connect(String),
    /// The daemon answered, but not with something this client can read.
    Malformed(String),
    /// The daemon answered with a non-2xx status and `{"error": ...}`.
    Status {
        /// The HTTP status.
        status: u16,
        /// The daemon's message.
        message: String,
    },
}

impl ApiError {
    /// The HTTP status of a refusal, when there was one.
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Whether the daemon says the thing asked about does not exist.
    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }

    /// Whether the daemon could not be reached at all.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, ApiError::Connect(_))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Config(message) => write!(formatter, "{message}"),
            ApiError::Connect(message) => write!(formatter, "cannot reach {message}"),
            ApiError::Malformed(message) => {
                write!(formatter, "the daemon's answer is malformed: {message}")
            }
            ApiError::Status { status, message } => write!(formatter, "{status}: {message}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// A client for one daemon endpoint.
#[derive(Debug, Clone)]
pub struct Client {
    endpoint: Endpoint,
    timeout: Duration,
}

impl Client {
    /// A client for `endpoint`, with the default timeout.
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The same client with a different request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The endpoint this client talks to.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// `GET /api/v0/runs`.
    pub fn runs(&self) -> Result<RunsPage, ApiError> {
        self.json("GET", "/api/v0/runs", None)
    }

    /// `GET /api/v0/runs/{id}`, with every VM's serial capture.
    pub fn run(&self, id: &str) -> Result<Run, ApiError> {
        self.json("GET", &format!("/api/v0/runs/{}", checked_id(id)?), None)
    }

    /// `DELETE /api/v0/runs/{id}`: kills the run's VMs.
    pub fn kill(&self, id: &str) -> Result<Run, ApiError> {
        self.json("DELETE", &format!("/api/v0/runs/{}", checked_id(id)?), None)
    }

    /// `POST /api/v0/runs`: boots a run's VMs.
    pub fn spawn(&self, create: &CreateRun) -> Result<Run, ApiError> {
        let body = serde_json::to_string(create)
            .map_err(|err| ApiError::Malformed(format!("cannot serialize the create request: {err}")))?;
        self.json("POST", "/api/v0/runs", Some(&body))
    }

    /// One request, parsed as JSON on success and as `{"error": ...}` otherwise.
    fn json<T: DeserializeOwned>(
        &self,
        method: &str,
        target: &str,
        body: Option<&str>,
    ) -> Result<T, ApiError> {
        let reply = self.send(method, target, body)?;
        if !(200..300).contains(&reply.status) {
            return Err(reply.into_error());
        }
        serde_json::from_str(&reply.body)
            .map_err(|err| ApiError::Malformed(format!("{method} {target}: {err}")))
    }

    /// Sends one request and reads the whole response.
    fn send(&self, method: &str, target: &str, body: Option<&str>) -> Result<Reply, ApiError> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.endpoint.port));
        let mut stream = TcpStream::connect_timeout(&address, self.timeout)
            .map_err(|err| ApiError::Connect(format!("{}: {err}", self.endpoint.url())))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|err| ApiError::Connect(format!("{}: {err}", self.endpoint.url())))?;
        let _ = stream.set_nodelay(true);

        let mut request = format!(
            "{method} {target} HTTP/1.1\r\nHost: {}:{}\r\n",
            self.endpoint.host, self.endpoint.port
        );
        if let Some(body) = body {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("Connection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .and_then(|()| match body {
                Some(body) => stream.write_all(body.as_bytes()),
                None => Ok(()),
            })
            .and_then(|()| stream.flush())
            .map_err(|err| ApiError::Connect(format!("{}: {err}", self.endpoint.url())))?;
        // The daemon reads exactly `Content-Length` bytes, so this only tells
        // it the request is over; it never waits for it either way.
        let _ = stream.shutdown(Shutdown::Write);

        read_reply(&mut stream)
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new(Endpoint::default())
    }
}

/// A run id as it may appear in a path: plain, so it cannot change the request.
fn checked_id(id: &str) -> Result<&str, ApiError> {
    if !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Ok(id)
    } else {
        Err(ApiError::Config(format!(
            "{id:?} is not a run id: ids are plain alphanumeric words"
        )))
    }
}

/// One response, read off the socket.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    /// The refusal this response carries.
    fn into_error(self) -> ApiError {
        #[derive(Deserialize)]
        struct ErrorBody {
            error: String,
        }
        let message = serde_json::from_str::<ErrorBody>(&self.body)
            .map(|body| body.error)
            .unwrap_or_else(|_| self.body.trim().to_string());
        ApiError::Status {
            status: self.status,
            message,
        }
    }
}

/// Reads one HTTP/1.1 response: the head, then exactly `Content-Length` bytes.
fn read_reply(stream: &mut TcpStream) -> Result<Reply, ApiError> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(position) = find(&buffer, b"\r\n\r\n") {
            break position + 4;
        }
        if buffer.len() > MAX_HEAD {
            return Err(ApiError::Malformed(
                "the response head is too large".to_string(),
            ));
        }
        let read = stream
            .read(&mut chunk)
            .map_err(|err| ApiError::Malformed(format!("reading the response: {err}")))?;
        if read == 0 {
            return Err(ApiError::Malformed(
                "the daemon closed before answering".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&buffer[..head_end - 4]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            ApiError::Malformed(format!("no status code in {status_line:?}"))
        })?;
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse::<usize>().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ApiError::Malformed(
                "chunked responses are not supported".to_string(),
            ));
        }
    }
    let content_length = content_length.ok_or_else(|| {
        ApiError::Malformed("the response has no Content-Length".to_string())
    })?;
    if content_length > MAX_BODY {
        return Err(ApiError::Malformed(format!(
            "the response body is larger than {MAX_BODY} bytes"
        )));
    }

    let mut body = buffer[head_end..].to_vec();
    while body.len() < content_length {
        let read = stream
            .read(&mut chunk)
            .map_err(|err| ApiError::Malformed(format!("reading the response: {err}")))?;
        if read == 0 {
            return Err(ApiError::Malformed(
                "the response body is shorter than Content-Length".to_string(),
            ));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Reply {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// The first position of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parses a fixture body, so the tests exercise exactly the type the client
/// deserializes into.
#[cfg(test)]
fn from_body<T: DeserializeOwned>(body: &str) -> T {
    serde_json::from_str(body).expect("the fixture should parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list body as `GET /api/v0/runs` sends it: no serial captures, and a
    /// run with vsock devices next to one without.
    const LIST: &str = r#"{"runs":[
        {"id":"r1","state":"running","vcpus":2,"memory_mib":512,"beats":null,
         "created_at":1760000000000,"ended_at":null,
         "vms":[{"id":0,"guest_cid":50,"state":"running"},
                {"id":1,"guest_cid":51,"state":"running"}]},
        {"id":"r2","state":"killed","vcpus":1,"memory_mib":128,"beats":3,
         "created_at":1760000001000,"ended_at":1760000009000,
         "vms":[{"id":0,"state":"killed"}]}
    ]}"#;

    /// A single-run body as `GET /api/v0/runs/r1` sends it: every VM's serial.
    const DETAIL: &str = "{\"id\":\"r1\",\"state\":\"running\",\"vcpus\":2,\"memory_mib\":512,\
         \"beats\":null,\"created_at\":1760000000000,\"ended_at\":null,\
         \"vms\":[{\"id\":0,\"guest_cid\":50,\"state\":\"running\",\
         \"serial\":\"login: plaza\\r\\n$ uname\\r\\nLinux\\r\\n\"}]}";

    #[test]
    fn a_list_body_parses_into_runs() {
        let page: RunsPage = from_body(LIST);
        assert_eq!(page.runs.len(), 2);

        let first = &page.runs[0];
        assert_eq!(first.id, "r1");
        assert_eq!(first.state, RunState::Running);
        assert_eq!(first.vcpus, 2);
        assert_eq!(first.memory_mib, 512);
        assert_eq!(first.beats, None);
        assert_eq!(first.created_at, 1_760_000_000_000);
        assert_eq!(first.ended_at, None);
        assert_eq!(first.live_vms(), 2);
        assert_eq!(first.base_cid(), Some(50));
        assert_eq!(first.vms[1].guest_cid, Some(51));
        assert_eq!(first.serial(), "", "a list view carries no serial");

        let second = &page.runs[1];
        assert_eq!(second.state, RunState::Killed);
        assert_eq!(second.beats, Some(3));
        assert_eq!(second.ended_at, Some(1_760_000_009_000));
        assert_eq!(second.live_vms(), 0);
        assert_eq!(second.base_cid(), None, "no vsock device, no CID");
    }

    #[test]
    fn a_detail_body_parses_into_a_run_with_its_serial() {
        let run: Run = from_body(DETAIL);
        assert_eq!(run.id, "r1");
        assert_eq!(run.vms[0].guest_cid, Some(50));
        assert_eq!(run.vms[0].serial.as_deref(), Some("login: plaza\r\n$ uname\r\nLinux\r\n"));
        assert_eq!(run.serial().lines().count(), 3);
    }

    #[test]
    fn an_unknown_state_parses_instead_of_failing() {
        let body = r#"{"runs":[{"id":"r9","state":"hibernating","vcpus":1,"memory_mib":128,
            "created_at":1,"ended_at":null,"vms":[{"id":0,"state":"paused"}]}]}"#;
        let page: RunsPage = from_body(body);
        assert_eq!(page.runs[0].state, RunState::Unknown);
        assert_eq!(page.runs[0].vms[0].state, VmState::Unknown);
        assert!(!page.runs[0].state.is_live());
    }

    #[test]
    fn a_create_body_carries_exactly_the_daemons_fields() {
        let create = CreateRun {
            n: 2,
            vcpus: 2,
            memory_mib: 512,
            beats: None,
            vsock_cid: Some(50),
        };
        let body = serde_json::to_value(create).expect("serializable");
        assert_eq!(
            body,
            serde_json::json!({
                "n": 2, "vcpus": 2, "memory_mib": 512, "beats": null, "vsock_cid": 50
            })
        );
        let parsed: CreateRun = from_body(&body.to_string());
        assert_eq!(parsed, create);
    }

    #[test]
    fn the_default_endpoint_is_the_loopbacks_daemon() {
        let endpoint = Endpoint::default();
        assert_eq!(endpoint.host(), "127.0.0.1");
        assert_eq!(endpoint.port(), DEFAULT_PORT);
        assert_eq!(endpoint.url(), "http://127.0.0.1:8899");
    }

    #[test]
    fn a_url_selects_a_loopback_port() {
        let endpoint = Endpoint::from_url("http://127.0.0.1:9000").expect("loopback");
        assert_eq!(endpoint.host(), "127.0.0.1");
        assert_eq!(endpoint.port(), 9000);

        assert_eq!(Endpoint::from_url("127.0.0.1:9000").expect("bare host").port(), 9000);
        assert_eq!(Endpoint::from_url("http://127.0.0.1/").expect("trailing slash").port(), 8899);
        assert_eq!(Endpoint::from_url(" http://localhost ").expect("localhost").host(), "127.0.0.1");
    }

    #[test]
    fn a_non_loopback_url_is_refused() {
        for url in [
            "http://0.0.0.0:8899",
            "http://127.0.0.1.evil.tld:8899",
            "https://127.0.0.1:8899",
            "http://[::1]:8899",
            "http://192.168.1.10:8899",
        ] {
            let error = Endpoint::from_url(url).expect_err("must be refused");
            assert!(
                matches!(error, ApiError::Config(_)),
                "{url} should be a configuration error, got {error:?}"
            );
        }
    }

    #[test]
    fn a_url_with_a_path_query_or_portless_garbage_is_refused() {
        assert!(Endpoint::from_url("http://127.0.0.1:8899/api/v0/runs").is_err());
        assert!(Endpoint::from_url("http://127.0.0.1:8899?a=b").is_err());
        assert!(Endpoint::from_url("http://127.0.0.1:notaport").is_err());
        assert!(Endpoint::from_url("http://127.0.0.1:0").is_err());
    }

    #[test]
    fn a_run_id_that_is_not_plain_is_refused_before_it_reaches_a_path() {
        assert_eq!(checked_id("r12").expect("a plain id"), "r12");
        for id in ["", "r1/../r2", "r1?x=1", "r1 r2", "r\u{00e9}"] {
            assert!(matches!(checked_id(id), Err(ApiError::Config(_))), "{id:?}");
        }
        let client = Client::default();
        assert!(client.run("r1/../r2").is_err());
    }

    #[test]
    fn an_error_body_becomes_the_daemons_message() {
        let reply = Reply {
            status: 400,
            body: r#"{"error":"n must be between 1 and 64"}"#.to_string(),
        };
        let error = reply.into_error();
        assert_eq!(error.status(), Some(400));
        assert_eq!(error.to_string(), "400: n must be between 1 and 64");
        assert!(!error.is_not_found());
    }

    #[test]
    fn a_404_is_recognizable() {
        let reply = Reply {
            status: 404,
            body: r#"{"error":"no run with id r9"}"#.to_string(),
        };
        let error = reply.into_error();
        assert!(error.is_not_found());
        assert_eq!(error.to_string(), "404: no run with id r9");
    }

    #[test]
    fn a_non_json_error_body_still_reports_the_status() {
        let reply = Reply {
            status: 500,
            body: "boom\n".to_string(),
        };
        let error = reply.into_error();
        assert_eq!(error.to_string(), "500: boom");
    }

    #[test]
    fn find_locates_the_head_terminator() {
        assert_eq!(find(b"a\r\n\r\nb", b"\r\n\r\n"), Some(1));
        assert_eq!(find(b"abc", b"\r\n\r\n"), None);
    }

    #[test]
    fn a_connect_error_says_so() {
        let error = ApiError::Connect("http://127.0.0.1:8899: refused".to_string());
        assert!(error.is_unreachable());
        assert_eq!(error.to_string(), "cannot reach http://127.0.0.1:8899: refused");
    }
}
