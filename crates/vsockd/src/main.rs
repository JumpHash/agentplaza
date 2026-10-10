//! `vsockd` — the AgentPlaza orchestrator stand-in.
//!
//! ```text
//! vsockd [--cid N] [--port P]
//! ```
//!
//! Listens on AF_VSOCK (`VMADDR_CID_HOST`, port 5000 by contract), accepts
//! one connection per guest VM, runs the frozen host side of the protocol
//! (`welcome`, `echo`, `uptime`, `bye`) and logs every line in both directions
//! to standard output. It runs until `SIGTERM` or `SIGINT`; a guest that sends
//! something this side cannot answer is logged and dropped, never guessed at.
//!
//! This is a stand-in for the real orchestrator: it exists so the plaza's core
//! loop can be exercised end to end against a real VM. It holds no policy
//! beyond the two tools the frozen slice contract defines, and no secrets.
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
mod line;
#[cfg(target_os = "linux")]
mod proto;
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
use line::StreamLines;
#[cfg(target_os = "linux")]
use serve::{HostError, SessionLimits, StdoutLog, handle_connection};

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

/// How long one read may block before the session is reconsidered. Set as
/// `SO_RCVTIMEO` on every accepted socket, so a guest that connects and then
/// sends nothing cannot pin its worker thread forever.
#[cfg(target_os = "linux")]
const RECV_TIMEOUT: Duration = Duration::from_secs(60);

/// The most time one guest session may live, from accept to close.
#[cfg(target_os = "linux")]
const SESSION_TIMEOUT: Duration = Duration::from_secs(300);

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
usage: vsockd [--uds-dir DIR] [--cids LIST] [--port P]

  --uds-dir DIR  directory for the per-VM vsock Unix sockets
                 (default /tmp/plaza-vsock)
  --cids LIST    comma-separated guest CIDs to serve, one listener per CID
                 (default 3)
  --port P       vsock port to listen on for each CID (default 5000)
  -h, --help     print this help

For each guest CID `c` the daemon listens on the Unix socket
`{DIR}/vm{c}_{port}` (the path the VMM's vsock muxer connects to when guest
`c` dials CID 2 port `port`). The guest's `hello.guest_cid` is checked against
the CID parsed from that socket path — identity by configuration, since the
muxer carries no peer address. Serves the plaza's frozen guest protocol:
hello -> welcome, tool_call (echo, uptime) -> tool_result, bye -> close. Every
line in both directions is logged; SIGTERM or SIGINT stops the daemon.";

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
}

#[cfg(target_os = "linux")]
impl Default for Config {
    fn default() -> Self {
        Self {
            uds_dir: PathBuf::from(DEFAULT_UDS_DIR),
            cids: vec![3],
            port: DEFAULT_PORT,
        }
    }
}

/// Reads `--uds-dir`, `--cids` and `--port`, leaving the rest at its default.
#[cfg(target_os = "linux")]
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("missing value for {flag}"));
        match argument.as_str() {
            "--uds-dir" => config.uds_dir = PathBuf::from(value("--uds-dir")?),
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
    let mut handles = Vec::new();
    let mut listener_paths = Vec::new();
    for cid in &config.cids {
        let cid = *cid;
        let path = uds_path(&config.uds_dir, cid, config.port);
        clear_stale_listener(&path, cid)?;
        let listener = UnixListener::bind(&path).map_err(|err| {
            format!("cannot listen on {} for guest CID {cid}: {err}", path.display())
        })?;
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
        handles.push(thread::spawn(move || accept_loop(cid, listener, limiter)));
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
                println!(
                    "vsockd: warning: cannot remove {}: {err}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

/// Makes `path` bindable: a socket file left by an earlier `vsockd` (a crash,
/// or a stop that could not clean up) makes `bind` fail with "address already
/// in use" although nothing listens on it. The file is stale only if nothing
/// answers a connection: a live answer means another listener already owns
/// the path — this daemon refuses to take an orchestrator's place — while an
/// unanswered probe (a dead socket, or any other leftover file in the
/// daemon's own 0700 directory) is removed so the listener can bind again.
#[cfg(target_os = "linux")]
fn clear_stale_listener(path: &Path, cid: u32) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    match UnixStream::connect(path) {
        Ok(_) => Err(format!(
            "another listener is already serving {} for guest CID {cid} — refusing to take it over",
            path.display()
        )),
        Err(_) => std::fs::remove_file(path).map_err(|err| {
            format!("cannot remove the stale socket {}: {err}", path.display())
        }),
    }
}

/// Accepts sessions for one guest CID and serves each on its own thread.
#[cfg(target_os = "linux")]
fn accept_loop(cid: u32, listener: UnixListener, limiter: Arc<SessionLimiter>) {
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
                    thread::spawn(move || {
                        let _guard = guard;
                        serve_guest(stream, cid);
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
fn serve_guest(stream: UnixStream, peer_cid: u32) {
    if let Err(err) = stream.set_read_timeout(Some(RECV_TIMEOUT)) {
        println!(
            "vsockd: session with cid {peer_cid} dropped: cannot set a read timeout ({err})"
        );
        return;
    }
    let deadline = Instant::now().checked_add(SESSION_TIMEOUT);
    let mut transport = StreamLines::new(stream);
    let limits = SessionLimits {
        max_calls: MAX_TOOL_CALLS,
        deadline,
    };
    match handle_connection(&mut transport, peer_cid, limits, &StdoutLog) {
        Ok(summary) => println!(
            "vsockd: session with guest {} vm {} ended cleanly after {} tool calls",
            summary.guest_cid, summary.vm, summary.calls
        ),
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
        ])
        .expect("explicit flags parse");
        assert_eq!(config.uds_dir, PathBuf::from("/tmp/x"));
        assert_eq!(config.cids, vec![3, 4, 5]);
        assert_eq!(config.port, 6000);
    }

    #[test]
    fn bad_arguments_are_reported() {
        assert!(parse_args(vec!["--port".to_string()]).is_err());
        assert!(parse_args(vec!["--port".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(vec!["--cids".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(vec!["--cids".to_string(), "".to_string()]).is_err());
        assert!(parse_args(vec!["--nope".to_string()]).is_err());
    }

    /// F4: the socket directory is created `0700`, and an existing wider one is
    /// re-tightened - `0700` is what keeps another local user from squatting a
    /// listener path or impersonating the orchestrator.
    #[test]
    fn the_uds_dir_is_created_and_tightened_to_0700() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("plaza-vsockd-uds-test-{}", std::process::id()));
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
        assert_eq!(mode & 0o777, 0o700, "a pre-existing socket directory is tightened");

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    /// A path that does not exist is bindable as-is; a leftover file is
    /// removed (nothing answers on it); a live listener keeps its path and
    /// the takeover is refused.
    #[test]
    fn stale_listener_files_are_cleared_but_live_listeners_are_kept() {
        let dir = std::env::temp_dir().join(format!("plaza-vsockd-stale-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_uds_dir(&dir).expect("the socket directory is created");

        // Absent: bindable without a word.
        let fresh = dir.join("vm30_5000");
        clear_stale_listener(&fresh, 30).expect("an absent path is bindable");
        assert!(!fresh.exists(), "an absent path stays absent");

        // Stale: a leftover file nothing answers is removed.
        let stale = dir.join("vm31_5000");
        std::fs::write(&stale, b"").expect("a leftover socket file");
        clear_stale_listener(&stale, 31).expect("a stale file is cleared");
        assert!(!stale.exists(), "the stale file is removed");

        // Live: a listener that answers keeps its path; the takeover is
        // refused (the probe connection is closed immediately).
        let live = dir.join("vm32_5000");
        let listener = UnixListener::bind(&live).expect("a live listener binds");
        let answer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the probe connects");
            drop(stream);
        });
        let refusal = clear_stale_listener(&live, 32)
            .expect_err("a live listener must not be taken over");
        assert!(
            refusal.contains("refusing to take it over"),
            "the refusal says why: {refusal}"
        );
        answer.join().expect("the probe connection is answered");
        assert!(live.exists(), "the live listener's path is kept");

        std::fs::remove_dir_all(&dir).expect("clean up");
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
