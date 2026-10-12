//! `vsockd` — the plaza's hub: the host end of the guest protocol.
//!
//! Listens on one Unix socket per guest CID (`{uds-dir}/vm{cid}_{port}`, the
//! path the VMM's vsock muxer dials when its guest connects to CID 2), serves
//! a session per connection ([`serve`]), keeps the registry of keepalive agents
//! ([`hub`]), and serves the HTTP surface on loopback ([`http`]). The agent
//! loop's v3 lines and hellos go to `apid` ([`apid`]) when `--internal-token-file`
//! names its token. Runs until `SIGTERM`/`SIGINT`.
//!
//! AF_VSOCK's muxer backend is Linux-only; elsewhere the binary refuses to run.

// Safe Rust except `sys`, the `poll(2)`/`sigaction(2)` boundary.
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
mod apid;
#[cfg(target_os = "linux")]
mod egress;
#[cfg(target_os = "linux")]
mod http;
#[cfg(target_os = "linux")]
mod hub;
#[cfg(target_os = "linux")]
mod serve;
#[cfg(target_os = "linux")]
mod sys;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use apz_proto::line::Lines;
#[cfg(target_os = "linux")]
use hub::Hub;
#[cfg(target_os = "linux")]
use serve::{HostError, SessionLimits, StdoutLog, SystemClock, handle_session};

/// The guest protocol's vsock port.
#[cfg(target_os = "linux")]
const DEFAULT_PORT: u32 = 5000;
#[cfg(target_os = "linux")]
const DEFAULT_UDS_DIR: &str = "/tmp/plaza-vsock";
#[cfg(target_os = "linux")]
const DEFAULT_HTTP_PORT: u16 = 19080;
/// How often an accept loop checks for a stop request.
#[cfg(target_os = "linux")]
const ACCEPT_POLL_MS: i32 = 200;
/// Guest sessions served at once; the next connection is refused.
#[cfg(target_os = "linux")]
const MAX_SESSIONS: usize = 64;
/// Probe/remove/bind rounds before `bind_listener` gives up on a racing path.
#[cfg(target_os = "linux")]
const MAX_BIND_ATTEMPTS: u32 = 8;
/// A v1 session's read timeout (`SO_RCVTIMEO`), so a silent guest cannot pin
/// its thread.
#[cfg(target_os = "linux")]
const RECV_TIMEOUT: Duration = Duration::from_secs(60);
/// A v1 session's whole life.
#[cfg(target_os = "linux")]
const SESSION_TIMEOUT: Duration = Duration::from_secs(300);
/// A keepalive session without a heartbeat for this long is reaped.
#[cfg(target_os = "linux")]
const KEEPALIVE_IDLE_SECS: u64 = 300;
/// How often a keepalive session wakes to take the hub's commands.
#[cfg(target_os = "linux")]
const KEEPALIVE_READ_SLICE: Duration = Duration::from_millis(250);
#[cfg(target_os = "linux")]
const MAX_TOOL_CALLS: u64 = 1024;

/// Caps concurrent guest sessions: a slot is reserved before a worker is
/// spawned and released when its guard drops.
#[cfg(target_os = "linux")]
struct SessionLimiter {
    active: AtomicUsize,
    max: usize,
}

#[cfg(target_os = "linux")]
impl SessionLimiter {
    fn new(max: usize) -> Self {
        Self { active: AtomicUsize::new(0), max }
    }

    fn try_acquire(self: &Arc<Self>) -> Option<SessionGuard> {
        if self.active.fetch_add(1, Ordering::SeqCst) >= self.max {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(SessionGuard(Arc::clone(self)))
    }
}

#[cfg(target_os = "linux")]
struct SessionGuard(Arc<SessionLimiter>);

#[cfg(target_os = "linux")]
impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(target_os = "linux")]
const USAGE: &str = "\
usage: vsockd [--uds-dir DIR] [--cids LIST] [--port P] [--http-port P] [--apid URL]
              [--internal-token-file PATH] [--egress on|off] [--egress-ports LIST]

  --uds-dir DIR     directory for the per-VM vsock Unix sockets (default /tmp/plaza-vsock)
  --cids LIST       comma-separated guest CIDs to serve, one listener each (default 3)
  --port P          vsock port each CID's listener answers (default 5000)
  --egress on|off   the egress proxy on vsock port 5001 per CID (default on); off serves none
  --egress-ports LIST
                    extra destination ports the proxy may reach beyond 80 and 443
                    (default none); private and loopback addresses are always refused
  --http-port P     the HTTP surface on 127.0.0.1 (default 19080): GET /v1/status,
                    GET /healthz, POST /v1/agents/{cid}/{send,tell,stop}
  --apid URL        apid's internal surface, http://127.0.0.1:PORT (default
                    http://127.0.0.1:7700)
  --internal-token-file PATH
                    the token apid's internal surface requires (re-read on every
                    call); without it the v3 agent loop is not forwarded
  -h, --help        print this help

Guest CID c is served at {DIR}/vm{c}_{port}; its hello must name c. A hello with
\"keepalive\": true opens a long-lived agent session (heartbeats every 5 s, reaped
after 300 s without one); otherwise the one-shot v1 session. SIGTERM or SIGINT
stops the daemon.";

#[cfg(all(unix, not(target_os = "linux")))]
fn main() -> ExitCode {
    eprintln!("vsockd: the UDS muxer backend is Linux-only; this host cannot accept guests");
    ExitCode::FAILURE
}

#[cfg(not(unix))]
fn main() -> ExitCode {
    eprintln!(
        "vsockd needs Unix sockets; on Windows the node runs without a guest socket bridge (planned)"
    );
    ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
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

#[cfg(target_os = "linux")]
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
}

#[cfg(target_os = "linux")]
impl Default for Config {
    fn default() -> Self {
        Self {
            uds_dir: PathBuf::from(DEFAULT_UDS_DIR),
            cids: vec![3],
            port: DEFAULT_PORT,
            http_port: DEFAULT_HTTP_PORT,
            apid: apid::DEFAULT_APID.to_string(),
            internal_token_file: None,
            egress: true,
            egress_ports: Vec::new(),
        }
    }
}

#[cfg(target_os = "linux")]
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("missing value for {flag}"));
        match flag.as_str() {
            "--uds-dir" => config.uds_dir = PathBuf::from(value?),
            "--apid" => config.apid = value?,
            "--internal-token-file" => config.internal_token_file = Some(PathBuf::from(value?)),
            "--cids" => {
                config.cids = value?
                    .split(',')
                    .filter(|part| !part.is_empty())
                    .map(|part| part.parse::<u32>().map_err(|err| format!("--cids: invalid CID {part}: {err}")))
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
                    .map(|part| part.parse::<u16>().map_err(|err| format!("--egress-ports: invalid port {part}: {err}")))
                    .collect::<Result<_, _>>()?;
            }
            "--port" => config.port = value?.parse().map_err(|err| format!("--port: {err}"))?,
            "--http-port" => config.http_port = value?.parse().map_err(|err| format!("--http-port: {err}"))?,
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
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Serves until a signal asks the daemon to stop.
#[cfg(target_os = "linux")]
fn run(config: Config) -> Result<(), String> {
    // Checked before anything is bound: a daemon with an unusable apid address does not serve.
    let apid = apid::Apid::new(&config.apid, config.internal_token_file.clone()).map_err(|err| format!("--apid: {err}"))?;
    if apid.enabled() {
        println!("vsockd: v3 agent loop forwarded to {}", config.apid);
    }
    sys::install_signal_handlers();
    create_uds_dir(&config.uds_dir)
        .map_err(|err| format!("cannot create the socket directory {}: {err}", config.uds_dir.display()))?;
    let egress_apid = apid.clone();
    let hub = Arc::new(Hub::new(Arc::new(StdoutLog), apid));
    let facts = http::NodeFacts {
        cids: config.cids.clone(),
        port: config.port,
        started: Instant::now(),
        max_sessions: MAX_SESSIONS,
        keepalive_idle_s: KEEPALIVE_IDLE_SECS,
    };
    let http = http::serve(config.http_port, Arc::clone(&hub), facts)
        .map_err(|err| format!("cannot listen on 127.0.0.1:{} for HTTP: {err}", config.http_port))?;
    println!("vsockd: http on http://127.0.0.1:{}", http.port());

    // Every listener is bound before any is served: a taken path fails the
    // daemon rather than half-serving.
    let mut listeners = Vec::new();
    let mut egress_listeners = Vec::new();
    for &cid in &config.cids {
        let path = config.uds_dir.join(format!("vm{cid}_{}", config.port));
        let listener = bind_listener(&path, cid)?;
        listener
            .set_nonblocking(true)
            .map_err(|err| format!("cannot make {} non-blocking: {err}", path.display()))?;
        println!("vsockd: listening on {} for guest CID {cid}", path.display());
        listeners.push((cid, path, listener));
        if config.egress {
            let path = config.uds_dir.join(format!("vm{cid}_{}", egress::EGRESS_PORT));
            let listener = bind_listener(&path, cid)?;
            listener
                .set_nonblocking(true)
                .map_err(|err| format!("cannot make {} non-blocking: {err}", path.display()))?;
            println!("vsockd: egress proxy listening on {} for guest CID {cid}", path.display());
            egress_listeners.push((cid, path, listener));
        }
    }
    let limiter = Arc::new(SessionLimiter::new(MAX_SESSIONS));
    let mut paths = Vec::new();
    let mut loops = Vec::new();
    for (cid, path, listener) in listeners {
        paths.push(path);
        let (limiter, hub) = (Arc::clone(&limiter), Arc::clone(&hub));
        loops.push(thread::spawn(move || accept_loop(cid, listener, limiter, hub)));
    }
    let policy = Arc::new(egress::Policy::new(config.egress_ports.clone()));
    for (cid, path, listener) in egress_listeners {
        paths.push(path);
        let (policy, egress_apid) = (Arc::clone(&policy), egress_apid.clone());
        loops.push(thread::spawn(move || egress::accept_loop(cid, listener, policy, egress_apid)));
    }
    for accept in loops {
        let _ = accept.join();
    }
    println!("vsockd: stop requested, closing the listeners");
    drop(http);
    // The socket files outlive their listeners; a stale one would fail the
    // next start's bind.
    for path in &paths {
        if let Err(err) = std::fs::remove_file(path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            println!("vsockd: warning: cannot remove {}: {err}", path.display());
        }
    }
    Ok(())
}

/// Whether a failed probe of an existing socket path means nobody listens:
/// refused (a dead socket) or not found (a leftover file). Anything else — a
/// full backlog (`EAGAIN`), a permission problem — is treated as live.
#[cfg(target_os = "linux")]
fn probe_is_stale(err: &std::io::Error) -> bool {
    matches!(err.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound)
}

/// Binds `path`, clearing a stale socket file but never taking over a live
/// listener. Two restarts racing one stale path settle within
/// [`MAX_BIND_ATTEMPTS`] rounds: the loser's bind fails and its next probe
/// sees the winner.
#[cfg(target_os = "linux")]
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
                return Err(format!("cannot remove the stale socket {}: {err}", path.display()));
            }
        }
        match UnixListener::bind(path) {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(err) => return Err(format!("cannot listen on {} for guest CID {cid}: {err}", path.display())),
        }
    }
    Err(format!(
        "cannot bind {} for guest CID {cid} after {MAX_BIND_ATTEMPTS} attempts — another restart is racing this path",
        path.display()
    ))
}

/// Accepts sessions for one CID, each on its own thread, until a stop request.
#[cfg(target_os = "linux")]
fn accept_loop(cid: u32, listener: UnixListener, limiter: Arc<SessionLimiter>, hub: Arc<Hub>) {
    while !sys::shutdown_requested() {
        if let Err(err) = sys::poll_readable(listener.as_raw_fd(), ACCEPT_POLL_MS) {
            eprintln!("vsockd: poll failed: {err}");
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
            Err(err) => eprintln!("vsockd: accept failed: {err}"),
        }
    }
}

/// Serves one guest session and logs how it ended.
#[cfg(target_os = "linux")]
fn serve_guest(stream: UnixStream, cid: u32, hub: &Hub) {
    if let Err(err) = stream.set_nonblocking(false).and_then(|()| stream.set_read_timeout(Some(RECV_TIMEOUT))) {
        println!("vsockd: session with cid {cid} dropped: cannot set a read timeout ({err})");
        return;
    }
    let limits = SessionLimits {
        max_calls: MAX_TOOL_CALLS,
        deadline: Instant::now().checked_add(SESSION_TIMEOUT),
        keepalive_idle: Some(Duration::from_secs(KEEPALIVE_IDLE_SECS)),
        keepalive_read_slice: KEEPALIVE_READ_SLICE,
    };
    match handle_session(&mut Lines::new(stream), cid, limits, &StdoutLog, hub, &SystemClock) {
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

#[cfg(all(target_os = "linux", test))]
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
            "--uds-dir", "/tmp/x", "--cids", "3,4,5", "--port", "6000", "--http-port", "0",
            "--apid", "http://127.0.0.1:9", "--internal-token-file", "t.token",
            "--egress", "off", "--egress-ports", "8080,8443",
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
        };
        assert_eq!(config, expected);
        assert!(Config::default().egress, "the egress proxy is on by default");
        for bad in [
            &["--port"][..],
            &["--port", "x"],
            &["--cids", ""],
            &["--http-port", "70000"],
            &["--roles", "r.json"],
            &["--egress", "maybe"],
            &["--egress-ports", "70000"],
        ] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_apid_address_that_is_not_loopback_stops_the_daemon_before_it_binds() {
        let never = scratch("apid").join("never-created");
        let config = Config { uds_dir: never.clone(), apid: "http://10.0.0.5:7700".into(), http_port: 0, ..Config::default() };
        let err = run(config).expect_err("a routable apid stops the daemon");
        assert!(err.starts_with("--apid:") && err.contains("loopback"), "{err}");
        assert!(!never.exists(), "nothing was created: {err}");
    }

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
        assert!(refusal.contains("refusing to take it over") && live.exists(), "{refusal}");

        let stale = |kind: std::io::ErrorKind| probe_is_stale(&kind.into());
        assert!(stale(std::io::ErrorKind::ConnectionRefused) && stale(std::io::ErrorKind::NotFound));
        assert!(!stale(std::io::ErrorKind::WouldBlock) && !stale(std::io::ErrorKind::PermissionDenied));
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
}
