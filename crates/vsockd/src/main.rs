//! `vsockd` — the node-side relay: the host end of the guest protocol, on each node.
//!
//! Listens on one Unix socket per guest CID (`{uds-dir}/vm{cid}_{port}`, the
//! path the VMM's vsock muxer dials when its guest connects to CID 2), serves
//! a session per connection ([`serve`]), keeps the registry of keepalive agents
//! ([`hub`]), and serves the HTTP surface on loopback ([`http`]). The agent
//! loop's v3 lines and hellos go to `apid` ([`apid`]) when `--internal-token-file`
//! names its token. Runs until `SIGTERM`/`SIGINT`.
//!
//! The muxer backend runs on Linux (AF_VSOCK) and Windows (AF_UNIX via `apz-uds`);
//! macOS refuses to run.

// Safe Rust except `sys`, the `poll(2)`/`sigaction(2)` boundary.
#![deny(unsafe_code)]

#[cfg(any(target_os = "linux", windows))]
mod apid;
#[cfg(any(target_os = "linux", windows))]
mod e2b;
#[cfg(any(target_os = "linux", windows))]
mod egress;
#[cfg(any(target_os = "linux", windows))]
mod http;
#[cfg(any(target_os = "linux", windows))]
mod hub;
#[cfg(any(target_os = "linux", windows))]
mod metrics;
#[cfg(any(target_os = "linux", windows))]
mod serve;
#[cfg(any(target_os = "linux", windows))]
mod sys;
#[cfg(any(target_os = "linux", windows))]
mod waker;

use std::process::ExitCode;

#[cfg(any(target_os = "linux", windows))]
use apz_uds::{UnixListener, UnixStream};
#[cfg(any(target_os = "linux", windows))]
use std::collections::BTreeMap;
#[cfg(any(target_os = "linux", windows))]
use std::net::TcpListener;
#[cfg(any(target_os = "linux", windows))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", windows))]
use std::sync::{
    Arc, Mutex, MutexGuard, PoisonError,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[cfg(any(target_os = "linux", windows))]
use std::thread::{self, JoinHandle};
#[cfg(any(target_os = "linux", windows))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", windows))]
use apz_http::auth::InternalToken;
#[cfg(any(target_os = "linux", windows))]
use apz_proto::line::Lines;
#[cfg(any(target_os = "linux", windows))]
use hub::Hub;
#[cfg(any(target_os = "linux", windows))]
use serve::{HostError, SessionLimits, StdoutLog, SystemClock, handle_session};

/// The guest protocol's vsock port.
#[cfg(any(target_os = "linux", windows))]
const DEFAULT_PORT: u32 = 5000;
#[cfg(any(target_os = "linux", windows))]
const DEFAULT_HTTP_PORT: u16 = 19080;
/// How often the daemon's main thread checks for a stop request.
#[cfg(any(target_os = "linux", windows))]
const ACCEPT_POLL_MS: i32 = 200;

/// The shutdown waker, created by `shutdown_waker`.
#[cfg(any(target_os = "linux", windows))]
static SHUTDOWN_WAKER: std::sync::OnceLock<crate::waker::Waker> = std::sync::OnceLock::new();

/// Woken once when the daemon is asked to stop, so every accept thread returns at once. Never drained.
#[cfg(any(target_os = "linux", windows))]
fn shutdown_waker() -> &'static crate::waker::Waker {
    SHUTDOWN_WAKER.get_or_init(|| {
        let waker = crate::waker::Waker::new().expect("vsockd: cannot create the shutdown waker");
        sys::set_shutdown_wake(waker.wake_handle());
        waker
    })
}
/// Guest sessions served at once; the next connection is refused.
#[cfg(any(target_os = "linux", windows))]
const MAX_SESSIONS: usize = 64;
/// Probe/remove/bind rounds before `bind_listener` gives up on a racing path.
#[cfg(any(target_os = "linux", windows))]
const MAX_BIND_ATTEMPTS: u32 = 8;
/// A v1 session's read timeout (`SO_RCVTIMEO`), so a silent guest cannot pin
/// its thread.
#[cfg(any(target_os = "linux", windows))]
const RECV_TIMEOUT: Duration = Duration::from_secs(60);
/// A v1 session's whole life.
#[cfg(any(target_os = "linux", windows))]
const SESSION_TIMEOUT: Duration = Duration::from_secs(300);
/// A keepalive session without a heartbeat for this long is reaped.
#[cfg(any(target_os = "linux", windows))]
const KEEPALIVE_IDLE_SECS: u64 = 300;
/// The read timeout on a keepalive session's guest socket, a safety net only: a keepalive session sleeps in
/// `wait_ready` until its guest, a hub command or an apid answer wakes it.
#[cfg(any(target_os = "linux", windows))]
const KEEPALIVE_READ_SLICE: Duration = Duration::from_millis(20);
/// The longest a keepalive session sleeps before it rechecks its deadlines.
#[cfg(any(target_os = "linux", windows))]
const KEEPALIVE_MAX_WAIT: Duration = Duration::from_secs(1);
#[cfg(any(target_os = "linux", windows))]
const MAX_TOOL_CALLS: u64 = 1024;

/// Caps concurrent guest sessions: a slot is reserved before a worker is
/// spawned and released when its guard drops.
#[cfg(any(target_os = "linux", windows))]
struct SessionLimiter {
    active: AtomicUsize,
    max: usize,
}

#[cfg(any(target_os = "linux", windows))]
impl SessionLimiter {
    fn new(max: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            max,
        }
    }

    fn try_acquire(self: &Arc<Self>) -> Option<SessionGuard> {
        if self.active.fetch_add(1, Ordering::SeqCst) >= self.max {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(SessionGuard(Arc::clone(self)))
    }
}

#[cfg(any(target_os = "linux", windows))]
struct SessionGuard(Arc<SessionLimiter>);

#[cfg(any(target_os = "linux", windows))]
impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(any(target_os = "linux", windows))]
const USAGE: &str = "\
usage: vsockd [--uds-dir DIR] [--cids LIST] [--port P] [--http-port P] [--apid URL]
              [--internal-token-file PATH] [--egress on|off] [--egress-ports LIST]
              [--e2b-port P]

  --uds-dir DIR     directory for the per-VM vsock Unix sockets (default /tmp/plaza-vsock;
                    %LOCALAPPDATA%\\AgentPlaza\\state\\vsock on Windows)
  --cids LIST       comma-separated guest CIDs to serve, one listener each (default 3)
  --port P          vsock port each CID's listener answers (default 5000)
  --egress on|off   the egress proxy on vsock port 5001 per CID (default on); off serves none
  --egress-ports LIST
                    extra destination ports the proxy may reach beyond 80 and 443
                    (default none); private and loopback addresses are always refused
  --http-port P     the HTTP surface on 127.0.0.1 (default 19080): GET /v1/status,
                    GET /healthz, POST and DELETE /v1/cids/{cid} (serve or stop serving
                    a guest CID at run time), POST /v1/agents/{cid}/{send,tell,stop}
  --apid URL        apid's internal surface, http://127.0.0.1:PORT (default
                    http://127.0.0.1:7700)
  --internal-token-file PATH
                    the token apid's internal surface requires (re-read on every
                    call), and the token the HTTP API requires but GET /healthz;
                    without it the v3 agent loop is not forwarded and the HTTP API
                    takes no token
  --e2b-port P      the E2B gateway on 127.0.0.1:P (default off): the SDKs' sandbox
                    requests, relayed to the sandbox's port 49983 through its VM's
                    muxer; needs --internal-token-file, since apid names the sandbox
  -h, --help        print this help

Guest CID c is served at {DIR}/vm{c}_{port}; its hello must name c. A hello with
\"keepalive\": true opens a long-lived agent session (heartbeats every 5 s, reaped
after 300 s without one); otherwise the one-shot v1 session. SIGTERM or SIGINT
stops the daemon.";

#[cfg(all(unix, not(target_os = "linux")))]
fn main() -> ExitCode {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("vsockd {}", apz_os::version());
        return ExitCode::SUCCESS;
    }
    eprintln!("vsockd: the UDS muxer backend is Linux-only; this host cannot accept guests");
    ExitCode::FAILURE
}

#[cfg(any(target_os = "linux", windows))]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("vsockd {}", apz_os::version());
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match parse_args(args).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("vsockd: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    uds_dir: PathBuf,
    cids: Vec<u32>,
    port: u32,
    http_port: u16,
    apid: String,
    internal_token_file: Option<PathBuf>,
    egress: bool,
    egress_ports: Vec<u16>,
    e2b_port: Option<u16>,
}

#[cfg(any(target_os = "linux", windows))]
impl Default for Config {
    fn default() -> Self {
        Self {
            uds_dir: apz_os::dirs::vsock_sockets(),
            cids: vec![3],
            port: DEFAULT_PORT,
            http_port: DEFAULT_HTTP_PORT,
            apid: apid::DEFAULT_APID.to_string(),
            internal_token_file: None,
            egress: true,
            egress_ports: Vec::new(),
            e2b_port: None,
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"));
        match flag.as_str() {
            "--uds-dir" => config.uds_dir = PathBuf::from(value?),
            "--apid" => config.apid = value?,
            "--internal-token-file" => config.internal_token_file = Some(PathBuf::from(value?)),
            "--cids" => {
                config.cids = value?
                    .split(',')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.parse::<u32>()
                            .map_err(|err| format!("--cids: invalid CID {part}: {err}"))
                    })
                    .collect::<Result<_, _>>()?;
                if config.cids.is_empty() {
                    return Err("--cids: at least one CID is required".to_string());
                }
            }
            "--egress" => {
                config.egress = match value?.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("--egress: expected on or off, not {other}")),
                }
            }
            "--egress-ports" => {
                config.egress_ports = value?
                    .split(',')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.parse::<u16>()
                            .map_err(|err| format!("--egress-ports: invalid port {part}: {err}"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--e2b-port" => {
                let port: u16 = value?.parse().map_err(|err| format!("--e2b-port: {err}"))?;
                if port == 0 {
                    return Err("--e2b-port: port 0 is not a port to listen on".to_string());
                }
                config.e2b_port = Some(port);
            }
            "--port" => config.port = value?.parse().map_err(|err| format!("--port: {err}"))?,
            "--http-port" => {
                config.http_port = value?
                    .parse()
                    .map_err(|err| format!("--http-port: {err}"))?
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(config)
}

/// Creates the socket directory `0700` (and tightens an existing one): the
/// listener names are predictable, so another local user must not be able to
/// squat or impersonate them. Mirrors `vmm::spawner::create_uds_dir`.
#[cfg(target_os = "linux")]
fn create_uds_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// On Windows the directory lives in the per-user profile, whose ACL already
/// keeps other users out, so it is only created.
#[cfg(windows)]
fn create_uds_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Serves until a signal asks the daemon to stop.
#[cfg(any(target_os = "linux", windows))]
fn run(config: Config) -> Result<(), String> {
    // Checked before anything is bound: a daemon with an unusable apid address does not serve.
    let apid = apid::Apid::new(&config.apid, config.internal_token_file.clone())
        .map_err(|err| format!("--apid: {err}"))?;
    if apid.enabled() {
        println!("vsockd: v3 agent loop forwarded to {}", config.apid);
    }
    sys::install_signal_handlers();
    shutdown_waker();
    create_uds_dir(&config.uds_dir).map_err(|err| {
        format!(
            "cannot create the socket directory {}: {err}",
            config.uds_dir.display()
        )
    })?;
    // The E2B gateway looks sandboxes up in apid, which needs the internal token: without it the
    // gateway would refuse every request, so it is not started.
    if config.e2b_port.is_some() && !apid.enabled() {
        return Err("--e2b-port needs --internal-token-file: apid names the sandboxes".to_string());
    }
    let e2b_gateway = config
        .e2b_port
        .map(|_| Arc::new(e2b::Gateway::new(apid.clone(), config.uds_dir.clone())));
    let egress_apid = apid.clone();
    let hub = Arc::new(Hub::new(Arc::new(StdoutLog), apid));
    let egress_setup = config.egress.then(|| {
        (
            Arc::new(egress::Policy::new(config.egress_ports.clone())),
            egress_apid,
        )
    });
    let registry = Arc::new(Registry::new(
        config.uds_dir.clone(),
        config.port,
        egress_setup,
        Arc::clone(&hub),
    ));
    let facts = http::NodeFacts {
        port: config.port,
        started: Instant::now(),
        max_sessions: MAX_SESSIONS,
        keepalive_idle_s: KEEPALIVE_IDLE_SECS,
    };
    let internal_token = match &config.internal_token_file {
        Some(path) => InternalToken::file(path),
        None => {
            println!("vsockd: no --internal-token-file: the HTTP API takes no token");
            InternalToken::none()
        }
    };
    let http = http::serve(
        config.http_port,
        Arc::clone(&hub),
        Arc::clone(&registry),
        facts,
        internal_token,
    )
    .map_err(|err| {
        format!(
            "cannot listen on 127.0.0.1:{} for HTTP: {err}",
            config.http_port
        )
    })?;
    println!("vsockd: http on http://127.0.0.1:{}", http.port());

    // The E2B listener is bound with the others, so a taken port fails the daemon before anything
    // is served.
    let e2b_listener = match config.e2b_port {
        Some(port) => {
            let listener = TcpListener::bind(("127.0.0.1", port))
                .map_err(|err| format!("cannot listen on 127.0.0.1:{port} for E2B: {err}"))?;
            println!("vsockd: e2b gateway on 127.0.0.1:{port}");
            Some(listener)
        }
        None => None,
    };

    // Every listener is bound before any is served: a taken path fails the
    // daemon rather than half-serving.
    let mut bound = Vec::new();
    for &cid in &config.cids {
        bound.push(registry.bind(cid)?);
    }
    for listeners in bound {
        registry.start(listeners);
    }
    let e2b_thread = e2b_listener
        .zip(e2b_gateway)
        .map(|(listener, gateway)| thread::spawn(move || e2b::serve(listener, gateway)));

    // The accept loops serve until a stop request; run-time CIDs stop with them.
    while !sys::shutdown_requested() {
        thread::sleep(Duration::from_millis(ACCEPT_POLL_MS as u64));
    }
    println!("vsockd: stop requested, closing the listeners");
    drop(http);
    registry.close();
    if let Some(handle) = e2b_thread {
        // The E2B accept loop checks for the stop request within one poll.
        let _ = handle.join();
    }
    Ok(())
}

/// The guest CIDs this daemon serves. Built at start from `--cids`, then changed at run time by
/// `POST`/`DELETE /v1/cids/{cid}`. A served CID has the same listeners however it was added: its
/// session socket and, with egress on, its egress socket.
#[cfg(any(target_os = "linux", windows))]
pub(crate) struct Registry {
    uds_dir: PathBuf,
    port: u32,
    /// The egress proxy's policy and apid; `None` when egress is off.
    egress: Option<(Arc<egress::Policy>, apid::Apid)>,
    hub: Arc<Hub>,
    limiter: Arc<SessionLimiter>,
    served: Mutex<BTreeMap<u32, Served>>,
}

/// One served CID and the threads that accept for it.
#[cfg(any(target_os = "linux", windows))]
struct Served {
    /// Stops the session accept loop; checked on every poll.
    stop: Arc<AtomicBool>,
    session: JoinHandle<()>,
    /// Present only when egress is on; it stops on the same request as the session loop.
    egress: Option<JoinHandle<()>>,
    paths: Vec<PathBuf>,
    /// Wakes the accept loops out of their wait, so they see `stop` at once.
    stop_waker: Arc<crate::waker::Waker>,
}

/// Listeners bound for one CID and not yet accepting.
#[cfg(any(target_os = "linux", windows))]
pub(crate) struct Bound {
    cid: u32,
    session: (PathBuf, UnixListener),
    egress: Option<(PathBuf, UnixListener)>,
}

/// Why a CID cannot be removed at run time.
#[cfg(any(target_os = "linux", windows))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveError {
    /// No listener serves the CID.
    NotServed,
}

#[cfg(any(target_os = "linux", windows))]
impl Registry {
    pub(crate) fn new(
        uds_dir: PathBuf,
        port: u32,
        egress: Option<(Arc<egress::Policy>, apid::Apid)>,
        hub: Arc<Hub>,
    ) -> Self {
        Self {
            uds_dir,
            port,
            egress,
            hub,
            limiter: Arc::new(SessionLimiter::new(MAX_SESSIONS)),
            served: Mutex::new(BTreeMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<u32, Served>> {
        self.served.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The served CIDs, ascending.
    pub(crate) fn served_cids(&self) -> Vec<u32> {
        self.lock().keys().copied().collect()
    }

    pub(crate) fn is_served(&self, cid: u32) -> bool {
        self.lock().contains_key(&cid)
    }

    /// Serves `cid` unless it already is; `Ok(true)` when serving starts now.
    pub(crate) fn serve(&self, cid: u32) -> Result<bool, String> {
        let mut served = self.lock();
        if served.contains_key(&cid) {
            return Ok(false);
        }
        let bound = self.bind(cid)?;
        served.insert(cid, self.spawn(bound));
        Ok(true)
    }

    /// Binds the session listener of `cid` and, with egress on, its egress listener.
    pub(crate) fn bind(&self, cid: u32) -> Result<Bound, String> {
        let session = bind_nonblocking(self.uds_dir.join(format!("vm{cid}_{}", self.port)), cid)?;
        println!(
            "vsockd: listening on {} for guest CID {cid}",
            session.0.display()
        );
        let egress = match self.egress {
            Some(_) => {
                let listener = bind_nonblocking(
                    self.uds_dir
                        .join(format!("vm{cid}_{}", egress::EGRESS_PORT)),
                    cid,
                )?;
                println!(
                    "vsockd: egress proxy listening on {} for guest CID {cid}",
                    listener.0.display()
                );
                Some(listener)
            }
            None => None,
        };
        Ok(Bound {
            cid,
            session,
            egress,
        })
    }

    /// Starts accepting for listeners that [`Registry::bind`] returned.
    pub(crate) fn start(&self, bound: Bound) {
        let cid = bound.cid;
        let served = self.spawn(bound);
        self.lock().insert(cid, served);
    }

    fn spawn(&self, bound: Bound) -> Served {
        let Bound {
            cid,
            session,
            egress,
        } = bound;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_waker =
            Arc::new(crate::waker::Waker::new().expect("vsockd: cannot create a CID's stop waker"));
        let (session_path, session_listener) = session;
        let mut paths = vec![session_path];
        let session = {
            let (stop, limiter, hub, stop_waker) = (
                Arc::clone(&stop),
                Arc::clone(&self.limiter),
                Arc::clone(&self.hub),
                Arc::clone(&stop_waker),
            );
            thread::spawn(move || {
                accept_loop(cid, session_listener, stop, limiter, hub, stop_waker)
            })
        };
        let mut egress_loop = None;
        if let (Some((path, listener)), Some((policy, apid))) = (egress, &self.egress) {
            paths.push(path);
            let (policy, apid, stop, stop_waker) = (
                Arc::clone(policy),
                apid.clone(),
                Arc::clone(&stop),
                Arc::clone(&stop_waker),
            );
            egress_loop = Some(thread::spawn(move || {
                egress::accept_loop(cid, listener, policy, apid, stop, stop_waker)
            }));
        }
        Served {
            stop,
            session,
            egress: egress_loop,
            paths,
            stop_waker,
        }
    }

    /// Stops serving `cid`: the session accept loop stops, and its socket files go. Sessions the
    /// CID already has run on to their own end. Refused, with nothing changed, when the CID has
    /// an egress listener.
    pub(crate) fn remove(&self, cid: u32) -> Result<(), RemoveError> {
        let mut served = self.lock();
        match served.remove(&cid) {
            None => Err(RemoveError::NotServed),
            Some(entry) => {
                entry.stop_and_join();
                Ok(())
            }
        }
    }

    /// Stops every served CID and removes its socket files, at shutdown.
    pub(crate) fn close(&self) {
        let entries = std::mem::take(&mut *self.lock());
        for entry in entries.into_values() {
            entry.stop_and_join();
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
impl Served {
    /// Stops the accept loops and joins them: the stop flag is set and the stop waker woken, so both
    /// return at once. Then removes the socket files.
    fn stop_and_join(self) {
        self.stop.store(true, Ordering::SeqCst);
        self.stop_waker.wake();
        let _ = self.session.join();
        if let Some(loop_) = self.egress {
            let _ = loop_.join();
        }
        remove_socket_files(&self.paths);
    }
}

/// Binds `path` for `cid` and makes it non-blocking, as the accept loops poll it.
#[cfg(any(target_os = "linux", windows))]
fn bind_nonblocking(path: PathBuf, cid: u32) -> Result<(PathBuf, UnixListener), String> {
    let listener = bind_listener(&path, cid)?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("cannot make {} non-blocking: {err}", path.display()))?;
    Ok((path, listener))
}

/// The socket files outlive their listeners; a stale one would fail the next start's bind.
#[cfg(any(target_os = "linux", windows))]
fn remove_socket_files(paths: &[PathBuf]) {
    for path in paths {
        if let Err(err) = std::fs::remove_file(path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            println!("vsockd: warning: cannot remove {}: {err}", path.display());
        }
    }
}

/// Whether a failed probe of an existing socket path means nobody listens:
/// refused (a dead socket) or not found (a leftover file). Anything else — a
/// full backlog (`EAGAIN`), a permission problem — is treated as live.
#[cfg(any(target_os = "linux", windows))]
fn probe_is_stale(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
    )
}

/// Binds `path`, clearing a stale socket file but never taking over a live
/// listener. Two restarts racing one stale path settle within
/// [`MAX_BIND_ATTEMPTS`] rounds: the loser's bind fails and its next probe
/// sees the winner.
#[cfg(any(target_os = "linux", windows))]
fn bind_listener(path: &Path, cid: u32) -> Result<UnixListener, String> {
    for _ in 0..MAX_BIND_ATTEMPTS {
        if path.exists() {
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(format!(
                        "another listener is already serving {} for guest CID {cid} — refusing to take it over",
                        path.display()
                    ));
                }
                Err(err) if !probe_is_stale(&err) => {
                    return Err(format!(
                        "the listener at {} for guest CID {cid} does not answer ({err}) — refusing to take it over",
                        path.display()
                    ));
                }
                Err(_) => {}
            }
            if let Err(err) = std::fs::remove_file(path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!(
                    "cannot remove the stale socket {}: {err}",
                    path.display()
                ));
            }
        }
        match UnixListener::bind(path) {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(err) => {
                return Err(format!(
                    "cannot listen on {} for guest CID {cid}: {err}",
                    path.display()
                ));
            }
        }
    }
    Err(format!(
        "cannot bind {} for guest CID {cid} after {MAX_BIND_ATTEMPTS} attempts — another restart is racing this path",
        path.display()
    ))
}

/// Accepts sessions for one CID, each on its own thread, until its stop flag or the daemon's stop
/// request. Sessions already running are not touched by the stop flag.
#[cfg(any(target_os = "linux", windows))]
fn accept_loop(
    cid: u32,
    listener: UnixListener,
    stop: Arc<AtomicBool>,
    limiter: Arc<SessionLimiter>,
    hub: Arc<Hub>,
    stop_waker: Arc<crate::waker::Waker>,
) {
    #[cfg(unix)]
    let listener_handle = {
        use std::os::fd::AsRawFd;
        listener.as_raw_fd()
    };
    #[cfg(windows)]
    let listener_handle = {
        use std::os::windows::io::AsRawSocket;
        listener.as_raw_socket()
    };
    while !stop.load(Ordering::SeqCst) && !sys::shutdown_requested() {
        let handles = [
            listener_handle,
            stop_waker.handle(),
            shutdown_waker().handle(),
        ];
        if let Err(err) = sys::wait_readable(&handles, None) {
            eprintln!("vsockd: poll failed: {err}");
            thread::sleep(Duration::from_millis(100));
        }
        match listener.accept() {
            Ok((stream, _)) => match limiter.try_acquire() {
                None => println!(
                    "vsockd: refusing a connection from cid {cid}: {MAX_SESSIONS} sessions are already running"
                ),
                Some(guard) => {
                    println!("vsockd: accepted a connection from cid {cid}");
                    let hub = Arc::clone(&hub);
                    thread::spawn(move || {
                        let _guard = guard;
                        serve_guest(stream, cid, &hub);
                    });
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => {
                eprintln!("vsockd: accept failed: {err}");
                // A broken listener or fd exhaustion must not spin the loop.
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Counts a guest session in the open-sessions gauge until it ends, however it ends.
#[cfg(any(target_os = "linux", windows))]
struct OpenSession;

#[cfg(any(target_os = "linux", windows))]
impl Drop for OpenSession {
    fn drop(&mut self) {
        crate::metrics::SESSIONS.add(-1);
    }
}

/// Serves one guest session and logs how it ended.
#[cfg(any(target_os = "linux", windows))]
fn serve_guest(stream: UnixStream, cid: u32, hub: &Hub) {
    crate::metrics::SESSIONS.add(1);
    let _open = OpenSession;
    if let Err(err) = stream
        .set_nonblocking(false)
        .and_then(|()| stream.set_read_timeout(Some(RECV_TIMEOUT)))
    {
        println!("vsockd: session with cid {cid} dropped: cannot set a read timeout ({err})");
        return;
    }
    let limits = SessionLimits {
        max_calls: MAX_TOOL_CALLS,
        deadline: Instant::now().checked_add(SESSION_TIMEOUT),
        keepalive_idle: Some(Duration::from_secs(KEEPALIVE_IDLE_SECS)),
        keepalive_read_slice: KEEPALIVE_READ_SLICE,
        keepalive_max_wait: KEEPALIVE_MAX_WAIT,
    };
    match handle_session(
        &mut Lines::new(stream),
        cid,
        limits,
        &StdoutLog,
        hub,
        &SystemClock,
    ) {
        // A keepalive session and a reap log their own ends.
        Ok(summary) if summary.keepalive => {}
        Err(HostError::Reaped) => {}
        Ok(summary) => println!(
            "vsockd: session with guest {} vm {} ended cleanly after {} tool calls",
            summary.guest_cid, summary.vm, summary.calls
        ),
        Err(err) => println!("vsockd: session with cid {cid} dropped: {err}"),
    }
}

/// Test helper: everything `stream` sends until it closes, read with short polled timeouts up to
/// `deadline`. On Windows a read already blocked on an AF_UNIX socket is not reliably woken when
/// the peer closes, but a fresh one sees the close; so no read here waits longer than a poll.
#[cfg(all(any(target_os = "linux", windows), test))]
pub(crate) fn read_until_closed(stream: &UnixStream, deadline: Duration) -> Vec<u8> {
    use std::io::{ErrorKind, Read};
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let end = Instant::now() + deadline;
    let mut reader = stream;
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < end {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            // A reset after the peer closed counts as the close.
            Err(_) => break,
        }
    }
    bytes
}

#[cfg(all(any(target_os = "linux", windows), test))]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Result<Config, String> {
        parse_args(args.iter().map(|arg| arg.to_string()).collect())
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plaza-vsockd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn flags_override_the_defaults_and_bad_ones_are_refused() {
        assert_eq!(args(&[]).unwrap(), Config::default());
        assert_eq!(Config::default().http_port, 19080);
        let config = args(&[
            "--uds-dir",
            "/tmp/x",
            "--cids",
            "3,4,5",
            "--port",
            "6000",
            "--http-port",
            "0",
            "--apid",
            "http://127.0.0.1:9",
            "--internal-token-file",
            "t.token",
            "--egress",
            "off",
            "--egress-ports",
            "8080,8443",
            "--e2b-port",
            "49990",
        ])
        .unwrap();
        let expected = Config {
            uds_dir: "/tmp/x".into(),
            cids: vec![3, 4, 5],
            port: 6000,
            http_port: 0,
            apid: "http://127.0.0.1:9".into(),
            internal_token_file: Some("t.token".into()),
            egress: false,
            egress_ports: vec![8080, 8443],
            e2b_port: Some(49990),
        };
        assert_eq!(config, expected);
        assert!(
            Config::default().egress,
            "the egress proxy is on by default"
        );
        for bad in [
            &["--port"][..],
            &["--port", "x"],
            &["--cids", ""],
            &["--http-port", "70000"],
            &["--roles", "r.json"],
            &["--egress", "maybe"],
            &["--egress-ports", "70000"],
            &["--e2b-port", "0"],
            &["--e2b-port", "70000"],
        ] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_apid_address_that_is_not_loopback_stops_the_daemon_before_it_binds() {
        let never = scratch("apid").join("never-created");
        let config = Config {
            uds_dir: never.clone(),
            apid: "http://10.0.0.5:7700".into(),
            http_port: 0,
            ..Config::default()
        };
        let err = run(config).expect_err("a routable apid stops the daemon");
        assert!(
            err.starts_with("--apid:") && err.contains("loopback"),
            "{err}"
        );
        assert!(!never.exists(), "nothing was created: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn the_uds_dir_is_created_and_tightened_to_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("uds");
        let mode = |dir: &Path| std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        create_uds_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_uds_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_socket_files_are_cleared_but_live_listeners_are_kept() {
        let dir = scratch("stale");
        create_uds_dir(&dir).unwrap();
        let stale = dir.join("vm31_5000");
        std::fs::write(&stale, b"").unwrap();
        drop(bind_listener(&stale, 31).expect("a stale file is cleared and bound over"));

        let live = dir.join("vm32_5000");
        let _listener = UnixListener::bind(&live).unwrap();
        let refusal = bind_listener(&live, 32).expect_err("a live listener is never taken over");
        assert!(
            refusal.contains("refusing to take it over") && live.exists(),
            "{refusal}"
        );

        let stale = |kind: std::io::ErrorKind| probe_is_stale(&kind.into());
        assert!(
            stale(std::io::ErrorKind::ConnectionRefused) && stale(std::io::ErrorKind::NotFound)
        );
        assert!(
            !stale(std::io::ErrorKind::WouldBlock) && !stale(std::io::ErrorKind::PermissionDenied)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_session_cap_refuses_beyond_its_limit_and_reuses_a_freed_slot() {
        let limiter = Arc::new(SessionLimiter::new(2));
        let first = limiter.try_acquire().unwrap();
        let _second = limiter.try_acquire().unwrap();
        assert!(limiter.try_acquire().is_none());
        drop(first);
        assert!(limiter.try_acquire().is_some());
    }

    #[test]
    fn a_cid_added_at_run_time_serves_a_guest_hello_and_its_removal_takes_the_socket_away() {
        use std::io::Write;
        use std::net::Shutdown;
        let dir = scratch("runtime-cid");
        create_uds_dir(&dir).unwrap();
        let registry = Registry::new(
            dir.clone(),
            DEFAULT_PORT,
            None,
            Arc::new(crate::hub::tests::hub_with(None).1),
        );
        assert_eq!(registry.serve(40), Ok(true));
        let path = dir.join(format!("vm40_{DEFAULT_PORT}"));
        assert!(path.exists(), "the run-time CID has its socket file");

        let mut guest = UnixStream::connect(&path).expect("the socket accepts");
        writeln!(guest, r#"{{"type":"hello","vm":0,"guest_cid":40}}"#).unwrap();
        guest.shutdown(Shutdown::Write).unwrap();
        let answer = String::from_utf8(read_until_closed(&guest, Duration::from_secs(10))).unwrap();
        assert!(answer.contains("welcome"), "a hello is answered: {answer}");
        drop(guest);

        assert_eq!(registry.remove(40), Ok(()));
        assert!(
            !path.exists(),
            "removing the CID takes its socket file away"
        );
        let refused = UnixStream::connect(&path).expect_err("a removed CID refuses a new connect");
        // Windows AF_UNIX reports a missing socket file as `WSAECONNREFUSED`, not `ENOENT`.
        #[cfg(not(windows))]
        let gone = std::io::ErrorKind::NotFound;
        #[cfg(windows)]
        let gone = std::io::ErrorKind::ConnectionRefused;
        assert_eq!(refused.kind(), gone, "{refused}");
        assert!(!registry.is_served(40));
        assert_eq!(registry.remove(40), Err(RemoveError::NotServed));
        registry.close();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_registry_starts_the_start_up_cids_and_serving_a_served_one_again_changes_nothing() {
        let dir = scratch("registry");
        create_uds_dir(&dir).unwrap();
        let registry = Registry::new(
            dir.clone(),
            DEFAULT_PORT,
            None,
            Arc::new(crate::hub::tests::hub_with(None).1),
        );
        let bound = registry.bind(3).expect("binds");
        registry.start(bound);
        assert_eq!(registry.served_cids(), vec![3]);
        assert_eq!(registry.serve(41), Ok(true));
        assert_eq!(registry.serve(41), Ok(false), "idempotent");
        assert_eq!(registry.served_cids(), vec![3, 41]);
        assert_eq!(registry.remove(41), Ok(()));
        assert_eq!(registry.served_cids(), vec![3]);
        assert_eq!(registry.remove(41), Err(RemoveError::NotServed));
        assert!(
            registry.bind(3).is_err(),
            "a served start-up CID's path is taken by a live listener"
        );
        registry.close();
        assert!(
            !dir.join(format!("vm3_{DEFAULT_PORT}")).exists(),
            "closing removes the start-up socket files too"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
