//! `vsockd` — the plaza's hub: the host end of the agent protocol.
//!
//! ```text
//! vsockd [--uds-dir DIR] [--cids LIST] [--port P] [--status-port P]
//! ```
//!
//! Listens on one Unix socket per guest CID (`{DIR}/vm{cid}_{port}`, the path
//! the VMM's vsock muxer dials when its guest connects to CID 2 port 5000),
//! accepts a session per connection, and serves the plaza's agent protocol on
//! it: the frozen one-shot session (`welcome`, `tool_call`, `bye`) and the
//! keepalive agent session a `hello` with `"keepalive": true` opens —
//! heartbeats, the hub's own tool calls, and the `stop` that ends it. Every
//! line in both directions is logged, along with the keepalive registry's
//! transitions (`vsockd: agent <cid> ...`). It runs until `SIGTERM` or
//! `SIGINT`; a guest that sends something this side cannot answer is logged
//! and dropped, never guessed at.
//!
//! `--status-port` adds a second, read-only surface on `127.0.0.1`: the
//! registry as JSON (`GET /status`), so the fleet's console can ask who is
//! connected without scraping the log, and a liveness probe (`GET /healthz`).
//! It is off unless asked for and mutates nothing — see [`status`].
//!
//! `--admin-port` adds the control plane's surface on `127.0.0.1`:
//! `POST /tell` sends one `tool_call` into a connected agent's session and
//! answers with that agent's own `tool_result`, bounded by a wait the caller
//! asks for; `POST /stop` sends the session the hub's `stop` and answers
//! whether the session ended (the guest's `bye` count, or an honest
//! `stopped: false` when it did not). It is off unless asked for, and it is
//! how the dispatcher (`dispatchd`) drives the fleet — see [`admin`].
//!
//! `--roles <manifest>` adds the codex's tool grants ([`roles`]): the manifest
//! (`docs/agents/initial-roles.json`) names, per role, the tools that role may
//! use, and the hub refuses — `403`, before anything is dispatched — any
//! `POST /tell` for a tool the session's role does not grant. A manifest the
//! daemon cannot read is a startup error, never a fleet served under a policy
//! nobody can name. It is off unless asked for: with no manifest the surface is
//! exactly what it was.
//!
//! This is a stand-in for the real orchestrator: it exists so the plaza's core
//! loop can be exercised end to end against a real VM. It holds no policy
//! beyond the two tools the frozen slice contract defines and, when `--roles`
//! names a manifest, the codex's per-role tool grants — and no secrets.
//!
//! AF_VSOCK exists on Linux only, so the crate's code is Linux-gated: the
//! workspace is also type-checked for `aarch64-apple-darwin`, where this
//! binary must still build, and there it refuses to run rather than pretend to
//! have listened.

// This crate is safe Rust only: `deny(unsafe_code)` makes any `unsafe` outside
// the one enumerated module below a compile error. The single entry is
//   * `sys` — the raw-syscall boundary (`poll(2)` and `sigaction(2)`), which
//     safe Rust cannot call.
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
mod admin;
#[cfg(target_os = "linux")]
mod hub;
#[cfg(target_os = "linux")]
mod line;
#[cfg(target_os = "linux")]
mod proto;
#[cfg(target_os = "linux")]
mod roles;
#[cfg(target_os = "linux")]
mod serve;
#[cfg(target_os = "linux")]
mod status;
#[cfg(target_os = "linux")]
mod sys;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
use std::net::TcpListener;
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
use hub::Hub;
#[cfg(target_os = "linux")]
use line::StreamLines;
#[cfg(target_os = "linux")]
use serve::{HostError, SessionLimits, StdoutLog, SystemClock, handle_session};
#[cfg(target_os = "linux")]
use status::NodeFacts;

/// The port the orchestrator listens on (the contract fixes 5000).
#[cfg(target_os = "linux")]
const DEFAULT_PORT: u32 = 5000;

/// Where the per-VM vsock Unix sockets live unless `--uds-dir` says otherwise;
/// guest CID `c` reaches the daemon at `{DEFAULT_UDS_DIR}/vm{c}_{port}`.
#[cfg(target_os = "linux")]
const DEFAULT_UDS_DIR: &str = "/tmp/plaza-vsock";

/// How long the accept loop waits for a connection before checking whether it
/// was asked to stop, in milliseconds.
#[cfg(target_os = "linux")]
const ACCEPT_POLL_MS: i32 = 200;

/// The most guest sessions the daemon serves at once. A connection above this
/// cap is refused and logged, so a fleet of hostile guests cannot exhaust the
/// daemon's threads and file descriptors.
#[cfg(target_os = "linux")]
const MAX_SESSIONS: usize = 64;

/// How many probe/remove/bind rounds `bind_listener` runs before giving up
/// with the honest error (two restarts racing the same stale path resolve in
/// one or two rounds; the bound keeps a hostile race from looping forever).
const MAX_BIND_ATTEMPTS: u32 = 8;

/// How long one read may block before the session is reconsidered. Set as
/// `SO_RCVTIMEO` on every accepted socket, so a guest that connects and then
/// sends nothing cannot pin its worker thread forever.
#[cfg(target_os = "linux")]
const RECV_TIMEOUT: Duration = Duration::from_secs(60);

/// The most time one guest session may live, from accept to close.
///
/// This bounds the frozen v1 session; a keepalive session is long-lived by
/// design and is bounded by [`KEEPALIVE_IDLE_SECS`] instead.
#[cfg(target_os = "linux")]
const SESSION_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a keepalive agent may go without a heartbeat before the hub reaps
/// its session. Staying up is the point of a keepalive session, so this — not
/// [`SESSION_TIMEOUT`] — is its liveness bound: an agent that keeps beating
/// may live forever, and an agent that stops is reaped rather than leaked.
#[cfg(target_os = "linux")]
const KEEPALIVE_IDLE_SECS: u64 = 300;

/// How long one read on a keepalive session waits before the session re-checks
/// its idle deadline and its outbound work. A keepalive session has to wake up
/// on its own account; a v1 session keeps [`RECV_TIMEOUT`].
#[cfg(target_os = "linux")]
const KEEPALIVE_READ_SLICE: Duration = Duration::from_secs(1);

/// The most tool calls one guest session may make before it is dropped.
#[cfg(target_os = "linux")]
const MAX_TOOL_CALLS: u64 = 1024;

/// A cap on how many guest sessions may run at once.
///
/// The accept loop reserves a slot before spawning each worker and the guard
/// releases it when the session ends, so the daemon's threads and file
/// descriptors stay bounded no matter how many connections a guest (or a fleet
/// of guests) opens.
#[cfg(target_os = "linux")]
struct SessionLimiter {
    active: AtomicUsize,
    max: usize,
}

#[cfg(target_os = "linux")]
impl SessionLimiter {
    fn new(max: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            max,
        }
    }

    /// Reserves a session slot, returning the guard that releases it on drop,
    /// or `None` when the cap is reached.
    fn try_acquire(self: &Arc<Self>) -> Option<SessionGuard> {
        let previous = self.active.fetch_add(1, Ordering::SeqCst);
        if previous >= self.max {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(SessionGuard(Arc::clone(self)))
    }
}

/// Releases a reserved session slot when a worker finishes.
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
usage: vsockd [--uds-dir DIR] [--cids LIST] [--port P] [--status-port P]
              [--admin-port P] [--roles MANIFEST]

  --uds-dir DIR  directory for the per-VM vsock Unix sockets
                 (default /tmp/plaza-vsock)
  --cids LIST    comma-separated guest CIDs to serve, one listener per CID
                 (default 3)
  --port P       vsock port to listen on for each CID (default 5000)
  --status-port P  also serve the read-only status surface on 127.0.0.1:P
                 (GET /status — the agent registry and the host facts as
                 JSON; GET /healthz — a liveness probe; default: off)
  --admin-port P   also serve the control plane's admin surface on 127.0.0.1:P
                 (POST /tell — send one tool_call into a connected agent's
                 session and answer with its tool_result; POST /stop — send a
                 session the hub's stop and answer whether it ended;
                 GET /healthz; default: off)
  --roles MANIFEST  enforce the codex's per-role tool grants from MANIFEST
                 (docs/agents/initial-roles.json): a POST /tell for a tool the
                 agent's role does not grant answers 403 and never reaches the
                 guest. The manifest is read and checked at startup — one that
                 is malformed (a role that declares no tools, or none; a role
                 that does not call itself by its key) stops the daemon rather
                 than serving under a policy nobody can name. Default: off,
                 and then every tool the surface accepts is driven.
  -h, --help     print this help

For each guest CID `c` the daemon listens on the Unix socket
`{DIR}/vm{c}_{port}` (the path the VMM's vsock muxer connects to when guest
`c` dials CID 2 port `port`). The guest's `hello.guest_cid` is checked against
the CID parsed from that socket path — identity by configuration, since the
muxer carries no peer address. Serves the plaza's frozen guest protocol:
hello -> welcome, tool_call (echo, uptime) -> tool_result, bye -> close, and
the keepalive agent protocol a hello with \"keepalive\": true opts into: the
hub dispatches tool_calls at will, the agent answers tool_results and beats
with a heartbeat every 5 s, and the session ends on the hub's stop (or on the
agent's bye) or is reaped after 300 s without a heartbeat. With --roles the
hub drives only the tools the agent's role grants. Every line in both
directions is logged; SIGTERM or SIGINT stops the daemon.";

/// On a host without Unix sockets there is nothing to listen on: say so and
/// fail, rather than starting a daemon that cannot accept a guest.
#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("vsockd: the UDS muxer backend is Linux-only; this host cannot accept guests");
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

/// Everything `vsockd` accepts on its command line.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct Config {
    /// Directory holding the per-VM vsock Unix sockets.
    uds_dir: PathBuf,
    /// The guest CIDs to serve, one listener per CID.
    cids: Vec<u32>,
    /// The vsock port each listener answers on.
    port: u32,
    /// The loopback TCP port the read-only status surface answers on, when
    /// `--status-port` asks for it.
    status_port: Option<u16>,
    /// The loopback TCP port the admin surface (`POST /tell`, `POST /stop`)
    /// answers on, when `--admin-port` asks for it.
    admin_port: Option<u16>,
    /// The roles manifest the admin surface's tool grants are enforced from,
    /// when `--roles` names one. `None` is the daemon with no policy: every
    /// tool the surface accepts is driven.
    roles: Option<PathBuf>,
}

#[cfg(target_os = "linux")]
impl Default for Config {
    fn default() -> Self {
        Self {
            uds_dir: PathBuf::from(DEFAULT_UDS_DIR),
            cids: vec![3],
            port: DEFAULT_PORT,
            status_port: None,
            admin_port: None,
            roles: None,
        }
    }
}

/// Reads `--uds-dir`, `--cids`, `--port`, `--status-port`, `--admin-port` and
/// `--roles`, leaving the rest at its default.
#[cfg(target_os = "linux")]
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match argument.as_str() {
            "--uds-dir" => config.uds_dir = PathBuf::from(value("--uds-dir")?),
            "--roles" => config.roles = Some(PathBuf::from(value("--roles")?)),
            "--cids" => {
                config.cids = value("--cids")?
                    .split(',')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.parse::<u32>()
                            .map_err(|err| format!("--cids: invalid CID {part}: {err}"))
                    })
                    .collect::<Result<Vec<u32>, String>>()?;
                if config.cids.is_empty() {
                    return Err("--cids: at least one CID is required".to_string());
                }
            }
            "--port" => {
                config.port = value("--port")?
                    .parse()
                    .map_err(|err| format!("--port: {err}"))?;
            }
            "--status-port" => {
                config.status_port = Some(
                    value("--status-port")?
                        .parse()
                        .map_err(|err| format!("--status-port: {err}"))?,
                );
            }
            "--admin-port" => {
                config.admin_port = Some(
                    value("--admin-port")?
                        .parse()
                        .map_err(|err| format!("--admin-port: {err}"))?,
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(config)
}

/// The socket path the muxer connects to when guest `cid` dials CID 2 `port`.
#[cfg(target_os = "linux")]
fn uds_path(uds_dir: &PathBuf, cid: u32, port: u32) -> PathBuf {
    uds_dir.join(format!("vm{cid}_{port}"))
}

/// Creates the directory holding the per-CID listeners, with mode `0700`.
///
/// The listener names are deterministic (`{dir}/vm{cid}_{port}`, the fixed
/// contract port) and the default directory is under world-writable `/tmp`, so
/// without this another local user could squat a listener path (the daemon
/// would fail to bind) or bind `{dir}/vm{cid}_{port}` first and answer as the
/// orchestrator. The directory is created (with parents) `0700`, and an
/// existing one is re-tightened to `0700`. (`vsockd` cannot depend on the
/// `vmm` crate, so this is a local copy of `vmm::spawner::create_uds_dir`; the
/// two must stay in step.)
#[cfg(target_os = "linux")]
fn create_uds_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    // `mode()` is masked by the process umask, so set the mode explicitly too
    // (this also tightens a directory that already existed).
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Listens, serves every guest session on its own thread, and returns when a
/// signal asks it to stop.
#[cfg(target_os = "linux")]
fn run(config: Config) -> Result<(), String> {
    let started = Instant::now();
    // The codex's tool grants, when a manifest was named: read and checked
    // *first*, before a socket is bound or a directory created. A daemon that
    // cannot enforce the policy it was given must not serve at all — a
    // malformed manifest, or one that is not there, fails the startup with what
    // it read (the same rule `noded --agents` follows for the fleet it boots).
    let roles = config
        .roles
        .as_ref()
        .map(|path| roles::Manifest::load(path))
        .transpose()
        .map_err(|err| format!("--roles: {err}"))?
        .map(Arc::new);
    sys::install_signal_handlers();
    // The listeners live under `uds_dir` with deterministic names; create it
    // 0700 first, so another local user cannot squat a path or impersonate
    // the orchestrator (F4).
    create_uds_dir(&config.uds_dir).map_err(|err| {
        format!(
            "cannot create the socket directory {}: {err}",
            config.uds_dir.display()
        )
    })?;
    // Bind every listener first, so a taken path fails the daemon before it
    // serves any guest (rather than one CID up and one down).
    let limiter = Arc::new(SessionLimiter::new(MAX_SESSIONS));
    // One hub for the whole daemon: the registry of connected keepalive
    // agents, shared by every session thread — and, when `--roles` named a
    // manifest, the tool-grant ceiling every dispatch passes (Hub::admit).
    let hub = match &roles {
        Some(manifest) => Hub::with_roles(Arc::new(StdoutLog), Arc::clone(manifest)),
        None => Hub::new(Arc::new(StdoutLog)),
    };
    let hub = Arc::new(hub);
    if let (Some(path), Some(manifest)) = (&config.roles, &roles) {
        println!(
            "vsockd: role tool grants enforced from {} ({} roles)",
            path.display(),
            manifest.len()
        );
    }
    let mut handles = Vec::new();
    // The host facts both loopback surfaces report are the daemon's own, so
    // they are built once and shared.
    let facts = Arc::new(NodeFacts::new(config.cids.clone(), config.port, started));
    // The status surface, when asked for, binds with the guest listeners: a
    // port that is already taken fails the daemon rather than half-serving.
    if let Some(port) = config.status_port {
        let listener = bind_status_listener(port)?;
        let bound = listener.local_addr().map_or(port, |addr| addr.port());
        listener
            .set_nonblocking(true)
            .map_err(|err| format!("cannot make the status listener non-blocking: {err}"))?;
        println!(
            "vsockd: status on http://127.0.0.1:{bound} (GET /status, GET /healthz, read-only)"
        );
        let hub = Arc::clone(&hub);
        let facts = Arc::clone(&facts);
        handles.push(thread::spawn(move || status::serve(listener, hub, facts)));
    }
    // The admin surface, the control plane's way in: `POST /tell` drives a
    // connected agent. Off unless `--admin-port` asks for it.
    if let Some(port) = config.admin_port {
        let listener = bind_admin_listener(port)?;
        let bound = listener.local_addr().map_or(port, |addr| addr.port());
        listener
            .set_nonblocking(true)
            .map_err(|err| format!("cannot make the admin listener non-blocking: {err}"))?;
        println!(
            "vsockd: admin on http://127.0.0.1:{bound} (POST /tell, POST /stop, GET /healthz)"
        );
        let hub = Arc::clone(&hub);
        let facts = Arc::clone(&facts);
        handles.push(thread::spawn(move || admin::serve(listener, hub, facts)));
    }
    let mut listener_paths = Vec::new();
    for cid in &config.cids {
        let cid = *cid;
        let path = uds_path(&config.uds_dir, cid, config.port);
        let listener = bind_listener(&path, cid)?;
        listener
            .set_nonblocking(true)
            .map_err(|err| format!("cannot make {0} non-blocking: {err}", path.display()))?;
        println!(
            "vsockd: listening on {} for guest CID {}",
            path.display(),
            cid
        );
        listener_paths.push(path);
        let limiter = Arc::clone(&limiter);
        let hub = Arc::clone(&hub);
        handles.push(thread::spawn(move || {
            accept_loop(cid, listener, limiter, hub)
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }
    println!("vsockd: stop requested, closing the listeners");
    // The listener sockets are closed with their threads; the socket FILES
    // outlive them, so remove each one or the next start would find a path
    // whose socket is dead and fail to bind. Best effort: a file that is
    // already gone (taken care of elsewhere) is not a failure.
    for path in &listener_paths {
        if let Err(err) = std::fs::remove_file(path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                println!("vsockd: warning: cannot remove {}: {err}", path.display());
            }
        }
    }
    Ok(())
}

/// What a failed probe connection says about the path: a REFUSED connection
/// (`ECONNREFUSED`) is a dead socket, a path NOT FOUND (`ENOENT`) is a plain
/// leftover file in the daemon's own 0700 directory — both stale, removable.
/// Anything else — a connection that would block because the listener's
/// accept backlog is full (`EAGAIN`), a permission problem, an unexplained
/// failure — is treated as live: the path is kept and the takeover refused.
///
/// The probe exists for the UDS listener path, which is Linux-only, so the
/// verdict and its classifier are gated with it (and so is the test that pins
/// them).
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum ProbeVerdict {
    Stale,
    Live,
}

#[cfg(target_os = "linux")]
fn probe_verdict(err: &std::io::Error) -> ProbeVerdict {
    match err.kind() {
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound => ProbeVerdict::Stale,
        _ => ProbeVerdict::Live,
    }
}

/// Binds the listener for `cid` at `path`, clearing stale socket files.
///
/// A socket file left by an earlier `vsockd` (a crash, or a stop that could
/// not clean up) makes `bind` fail although nothing listens on it. The probe
/// distinguishes exactly: a connection that is REFUSED (`ECONNREFUSED`) is
/// a dead socket, a path that is NOT FOUND (`ENOENT`) is a plain leftover
/// file in the daemon's own 0700 directory — both are stale and removed.
/// A connection that is ANSWERED, one that fails because the listener's
/// accept backlog is FULL (`EAGAIN`), or any other unexplained failure is
/// treated as live: the takeover is refused, and never by removing the file
/// a live listener may own. Two restarts may race the same stale path (both
/// see it dead, both remove): the loser's `bind` then fails because the
/// winner is already bound, and the next round's probe refuses — a live
/// listener's path is never harvested, whoever won. A `NotFound` on the
/// remove means another restart won that race (fall through to the bind,
/// whose `EADDRINUSE` retry re-probes), and the whole dance is bounded:
/// after [`MAX_BIND_ATTEMPTS`] rounds the daemon gives up with the honest
/// error rather than looping forever.
#[cfg(target_os = "linux")]
fn bind_listener(path: &Path, cid: u32) -> Result<UnixListener, String> {
    for _attempt in 1..=MAX_BIND_ATTEMPTS {
        if path.exists() {
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(format!(
                        "another listener is already serving {} for guest CID {cid} — refusing to take it over",
                        path.display()
                    ));
                }
                Err(err) => match probe_verdict(&err) {
                    // A dead socket (`ECONNREFUSED`) or a leftover
                    // non-socket file in the daemon's own 0700 directory
                    // (`ENOENT` on Linux when the path is no socket):
                    // nothing answers, the path is stale.
                    ProbeVerdict::Stale => {}
                    // A listener is there but its accept backlog is full
                    // (`EAGAIN`), or the probe failed in a way this daemon
                    // does not understand: treat the path as live and
                    // refuse — never remove a file a live listener may own.
                    ProbeVerdict::Live => {
                        return Err(format!(
                            "the listener at {} for guest CID {cid} does not answer ({err}) — refusing to take it over",
                            path.display()
                        ));
                    }
                },
            }
            if path.exists() {
                if let Err(err) = std::fs::remove_file(path) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        return Err(format!(
                            "cannot remove the stale socket {}: {err}",
                            path.display()
                        ));
                    }
                    // `NotFound`: another restart removed the same stale
                    // file first. Fall through to the bind; its
                    // `EADDRINUSE` retry re-probes whoever bound it.
                }
            }
        }
        match UnixListener::bind(path) {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                // Someone bound this path between the probe and the bind. If
                // it is a live listener the next round's probe refuses; if it
                // is already dead again the next round clears it.
                continue;
            }
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

/// Binds the status surface's listener.
///
/// It is loopback-only by construction: the console runs on the same host, and
/// the registry is not the network's to read.
#[cfg(target_os = "linux")]
fn bind_status_listener(port: u16) -> Result<TcpListener, String> {
    TcpListener::bind(("127.0.0.1", port))
        .map_err(|err| format!("cannot listen on 127.0.0.1:{port} for the status surface: {err}"))
}

/// Binds the admin surface's listener.
///
/// Loopback-only, like the status surface: the dispatcher runs on the same
/// host, and a surface that can make an agent act is not the network's to
/// reach.
#[cfg(target_os = "linux")]
fn bind_admin_listener(port: u16) -> Result<TcpListener, String> {
    TcpListener::bind(("127.0.0.1", port))
        .map_err(|err| format!("cannot listen on 127.0.0.1:{port} for the admin surface: {err}"))
}

/// Accepts sessions for one guest CID and serves each on its own thread.
#[cfg(target_os = "linux")]
fn accept_loop(cid: u32, listener: UnixListener, limiter: Arc<SessionLimiter>, hub: Arc<Hub>) {
    while !sys::shutdown_requested() {
        match sys::poll_readable(listener.as_raw_fd(), ACCEPT_POLL_MS) {
            Ok(false) => continue,
            // A connection, a shutdown request or a spurious wakeup: in every
            // case the loop re-checks the flag and tries to accept.
            Ok(true) => {}
            Err(err) => eprintln!("vsockd: poll failed: {err}"),
        }
        match listener.accept() {
            Ok((stream, _addr)) => match limiter.try_acquire() {
                None => {
                    println!(
                        "vsockd: refusing a connection from cid {cid}: {MAX_SESSIONS} \
                         sessions are already running"
                    );
                    // `stream` is dropped here, closing the connection.
                }
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

/// Serves one guest session and reports how it ended.
#[cfg(target_os = "linux")]
fn serve_guest(stream: UnixStream, peer_cid: u32, hub: &Hub) {
    if let Err(err) = stream.set_read_timeout(Some(RECV_TIMEOUT)) {
        println!("vsockd: session with cid {peer_cid} dropped: cannot set a read timeout ({err})");
        return;
    }
    let deadline = Instant::now().checked_add(SESSION_TIMEOUT);
    let mut transport = StreamLines::new(stream);
    let limits = SessionLimits {
        max_calls: MAX_TOOL_CALLS,
        deadline,
        keepalive_idle: Some(Duration::from_secs(KEEPALIVE_IDLE_SECS)),
        keepalive_read_slice: KEEPALIVE_READ_SLICE,
    };
    match handle_session(
        &mut transport,
        peer_cid,
        limits,
        &StdoutLog,
        hub,
        &SystemClock,
    ) {
        // A keepalive session logs its own transitions and its own end
        // (`vsockd: agent <cid> ...`), so there is nothing to add here.
        Ok(summary) if summary.keepalive => {}
        Ok(summary) => println!(
            "vsockd: session with guest {} vm {} ended cleanly after {} tool calls",
            summary.guest_cid, summary.vm, summary.calls
        ),
        // The hub logged the reap with its reason.
        Err(HostError::Reaped) => {}
        Err(err) => println!(
            "vsockd: session with cid {peer_cid} dropped: {}",
            describe(&err)
        ),
    }
}

/// A one-line classification of a session error.
#[cfg(target_os = "linux")]
fn describe(err: &HostError) -> String {
    match err {
        HostError::Io(io) => format!("connection failed ({io})"),
        HostError::Protocol(detail) => format!("protocol violation ({detail})"),
        HostError::Dropped(detail) => format!("connection lost ({detail})"),
        HostError::Reaped => "reaped: the agent stopped heartbeating".to_string(),
    }
}

#[cfg(all(target_os = "linux", test))]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_contracts_defaults() {
        let config = parse_args(Vec::new()).expect("no arguments is valid");
        assert_eq!(config.uds_dir, PathBuf::from(DEFAULT_UDS_DIR));
        assert_eq!(config.cids, vec![3]);
        assert_eq!(config.port, 5000);
        assert_eq!(
            config.status_port, None,
            "the status surface is off unless asked for"
        );
        assert_eq!(
            config.admin_port, None,
            "the admin surface is off unless asked for"
        );
        assert_eq!(config.roles, None, "no manifest, no policy");
    }

    #[test]
    fn explicit_flags_override_their_defaults() {
        let config = parse_args(vec![
            "--uds-dir".to_string(),
            "/tmp/x".to_string(),
            "--cids".to_string(),
            "3,4,5".to_string(),
            "--port".to_string(),
            "6000".to_string(),
            "--status-port".to_string(),
            "9101".to_string(),
            "--admin-port".to_string(),
            "9102".to_string(),
            "--roles".to_string(),
            "docs/agents/initial-roles.json".to_string(),
        ])
        .expect("explicit flags parse");
        assert_eq!(config.uds_dir, PathBuf::from("/tmp/x"));
        assert_eq!(config.cids, vec![3, 4, 5]);
        assert_eq!(config.port, 6000);
        assert_eq!(config.status_port, Some(9101));
        assert_eq!(config.admin_port, Some(9102));
        assert_eq!(
            config.roles,
            Some(PathBuf::from("docs/agents/initial-roles.json"))
        );
    }

    #[test]
    fn bad_arguments_are_reported() {
        assert!(parse_args(vec!["--port".to_string()]).is_err());
        assert!(parse_args(vec!["--port".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(vec!["--cids".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(vec!["--cids".to_string(), "".to_string()]).is_err());
        assert!(parse_args(vec!["--status-port".to_string()]).is_err());
        assert!(parse_args(vec!["--status-port".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(vec!["--status-port".to_string(), "70000".to_string()]).is_err());
        assert!(parse_args(vec!["--admin-port".to_string()]).is_err());
        assert!(parse_args(vec!["--admin-port".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(vec!["--admin-port".to_string(), "70000".to_string()]).is_err());
        assert!(parse_args(vec!["--roles".to_string()]).is_err());
        assert!(parse_args(vec!["--nope".to_string()]).is_err());
    }

    /// A manifest the daemon cannot read stops it *before* anything is bound or
    /// created: a daemon that cannot enforce the policy it was given must not
    /// serve (the rule `noded --agents` follows for its fleet).
    #[test]
    fn a_manifest_that_cannot_be_read_stops_the_daemon_before_it_binds() {
        let dir = std::env::temp_dir().join(format!(
            "plaza-vsockd-roles-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let never = dir.join("never-created");
        let manifest = dir.join("roles.json");
        std::fs::create_dir_all(&dir).expect("a directory for the test's manifest");

        // Not there at all.
        let config = Config {
            uds_dir: never.clone(),
            roles: Some(dir.join("absent.json")),
            ..Config::default()
        };
        let err = run(config).expect_err("a missing manifest stops the daemon");
        assert!(err.starts_with("--roles: cannot read"), "{err}");
        assert!(err.contains("absent.json"), "{err}");
        assert!(!never.exists(), "nothing was bound or created: {err}");

        // There, and not a policy: a role that declares no tools.
        std::fs::write(&manifest, r#"{"roles": {"builder": {"role": "builder"}}}"#)
            .expect("the test writes a malformed manifest");
        let config = Config {
            uds_dir: never.clone(),
            roles: Some(manifest.clone()),
            ..Config::default()
        };
        let err = run(config).expect_err("a malformed manifest stops the daemon");
        assert!(err.starts_with("--roles: role \"builder\""), "{err}");
        assert!(err.contains("declares no tools"), "{err}");
        assert!(!never.exists(), "nothing was bound or created: {err}");

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    /// The status surface binds loopback only, and a port that is taken is an
    /// honest error rather than a silent second binding.
    #[test]
    fn the_status_listener_binds_loopback_and_refuses_a_taken_port() {
        let listener = bind_status_listener(0).expect("an ephemeral port binds");
        let addr = listener.local_addr().expect("a bound address");
        assert!(
            addr.ip().is_loopback(),
            "the status surface is loopback only"
        );
        let taken = bind_status_listener(addr.port());
        assert!(
            taken
                .expect_err("a taken port is an error")
                .contains("status surface"),
            "the error names the surface"
        );
    }

    /// The admin surface binds loopback only too — it can make an agent act,
    /// so it is even less the network's to reach — and a taken port is an
    /// honest error.
    #[test]
    fn the_admin_listener_binds_loopback_and_refuses_a_taken_port() {
        let listener = bind_admin_listener(0).expect("an ephemeral port binds");
        let addr = listener.local_addr().expect("a bound address");
        assert!(
            addr.ip().is_loopback(),
            "the admin surface is loopback only"
        );
        let taken = bind_admin_listener(addr.port());
        assert!(
            taken
                .expect_err("a taken port is an error")
                .contains("admin surface"),
            "the error names the surface"
        );
    }

    /// F4: the socket directory is created `0700`, and an existing wider one is
    /// re-tightened - `0700` is what keeps another local user from squatting a
    /// listener path or impersonating the orchestrator.
    #[test]
    fn the_uds_dir_is_created_and_tightened_to_0700() {
        use std::os::unix::fs::PermissionsExt;

        let dir =
            std::env::temp_dir().join(format!("plaza-vsockd-uds-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        create_uds_dir(&dir).expect("the socket directory is created");
        let mode = std::fs::metadata(&dir)
            .expect("the directory exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "a fresh socket directory must be 0700");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("widen the directory for the test");
        create_uds_dir(&dir).expect("an existing directory is accepted");
        let mode = std::fs::metadata(&dir)
            .expect("the directory exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "a pre-existing socket directory is tightened"
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    /// `bind_listener`: an absent path binds as-is; a leftover file nothing
    /// answers is cleared and the listener binds over it; a live listener
    /// keeps its path and the takeover is refused; and a live listener whose
    /// backlog is full (whose probes fail, so it looks stale) is still never
    /// taken over — the bind retry refuses to harvest it, and the daemon
    /// gives up with the honest race error instead.
    #[test]
    fn stale_listener_files_are_cleared_but_live_listeners_are_kept() {
        let dir =
            std::env::temp_dir().join(format!("plaza-vsockd-stale-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_uds_dir(&dir).expect("the socket directory is created");

        // Absent: bindable without a word.
        let fresh = dir.join("vm30_5000");
        let fresh_listener = bind_listener(&fresh, 30).expect("an absent path is bindable");
        assert!(fresh.exists(), "the new listener's socket file exists");
        drop(fresh_listener);
        let _ = std::fs::remove_file(&fresh);

        // Stale: a leftover file nothing answers is removed and bound over.
        let stale = dir.join("vm31_5000");
        std::fs::write(&stale, b"").expect("a leftover socket file");
        let stale_listener = bind_listener(&stale, 31).expect("a stale file is cleared and bound");
        assert!(stale.exists(), "the new listener's socket file exists");
        drop(stale_listener);
        let _ = std::fs::remove_file(&stale);

        // Live: a listener that answers keeps its path; the takeover is
        // refused (the probe connection is closed immediately).
        let live = dir.join("vm32_5000");
        let listener = UnixListener::bind(&live).expect("a live listener binds");
        // The thread answers exactly one probe and ends; nothing joins it,
        // because the refusal this test asserts does not wait on it.
        let _probe = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the probe connects");
            drop(stream);
        });
        let refusal = bind_listener(&live, 32).expect_err("a live listener must not be taken over");
        assert!(
            refusal.contains("refusing to take it over"),
            "the refusal says why: {refusal}"
        );
        // A listener with parked (un-accepted) connections is still live: the
        // takeover is refused and its path file is never removed. (A truly
        // FULL backlog would make the probe itself fail with EAGAIN — that
        // policy is `probe_verdict`'s, unit-tested below, because a blocking
        // std connect cannot produce EAGAIN from a client.)
        let busy = dir.join("vm33_5000");
        let busy_listener = UnixListener::bind(&busy).expect("the busy listener binds");
        let parked: Vec<UnixStream> = (0..64)
            .map(|_| UnixStream::connect(&busy).expect("the parked connection connects"))
            .collect();
        let race = bind_listener(&busy, 33).expect_err("a busy listener is never taken over");
        assert!(
            race.contains("refusing to take it over"),
            "the refusal says why: {race}"
        );
        assert!(
            busy.exists(),
            "the busy listener's path file is never removed"
        );
        drop(parked);
        drop(busy_listener);

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    /// The probe's error-kind policy: only a refused connection and a path
    /// not found are stale; everything else — a would-block (a full accept
    /// backlog), a permission problem, anything unexplained — is live, and a
    /// live path is never removed.
    #[test]
    fn only_refused_and_missing_probes_are_stale() {
        let kind = |k: std::io::ErrorKind| probe_verdict(&std::io::Error::from(k));
        assert_eq!(
            kind(std::io::ErrorKind::ConnectionRefused),
            ProbeVerdict::Stale
        );
        assert_eq!(kind(std::io::ErrorKind::NotFound), ProbeVerdict::Stale);
        assert_eq!(kind(std::io::ErrorKind::WouldBlock), ProbeVerdict::Live);
        assert_eq!(
            kind(std::io::ErrorKind::PermissionDenied),
            ProbeVerdict::Live
        );
        assert_eq!(kind(std::io::ErrorKind::TimedOut), ProbeVerdict::Live);
    }

    /// The session cap reserves at most `max` slots and refuses the next
    /// connection, but reuses a slot once its session ends.
    #[test]
    fn the_session_cap_refuses_beyond_its_limit() {
        let limiter = Arc::new(SessionLimiter::new(2));
        let first = limiter.try_acquire().expect("the first slot");
        let second = limiter.try_acquire().expect("the second slot");
        assert!(limiter.try_acquire().is_none(), "the third slot is refused");
        drop(first);
        let third = limiter.try_acquire().expect("a freed slot is reused");
        drop(second);
        drop(third);
    }
}
