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

#[cfg(target_os = "linux")]
mod line;
#[cfg(target_os = "linux")]
mod proto;
#[cfg(target_os = "linux")]
mod serve;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
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

/// Set by the signal handler; the accept loop reads it.
#[cfg(target_os = "linux")]
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

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

/// Polls `fd` for readability, returning `Ok(false)` on timeout. The timeout
/// is the accept loop's shutdown-check cadence, mirroring the old AF_VSOCK
/// `wait_readable`.
#[cfg(target_os = "linux")]
fn poll_readable(fd: std::os::fd::RawFd, timeout_ms: i32) -> std::io::Result<bool> {
    // SAFETY: `pollfd` is fully initialised; `fd` is valid for the duration.
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: the pollfd pointer points to one valid entry.
    let ret = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(ret > 0)
    }
}

/// Listens, serves every guest session on its own thread, and returns when a
/// signal asks it to stop.
#[cfg(target_os = "linux")]
fn run(config: Config) -> Result<(), String> {
    install_signal_handlers();
    // Bind every listener first, so a taken path fails the daemon before it
    // serves any guest (rather than one CID up and one down).
    let limiter = Arc::new(SessionLimiter::new(MAX_SESSIONS));
    let mut handles = Vec::new();
    for cid in &config.cids {
        let cid = *cid;
        let path = uds_path(&config.uds_dir, cid, config.port);
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
        let limiter = Arc::clone(&limiter);
        handles.push(thread::spawn(move || accept_loop(cid, listener, limiter)));
    }
    for handle in handles {
        let _ = handle.join();
    }
    println!("vsockd: stop requested, closing the listeners");
    Ok(())
}

/// Accepts sessions for one guest CID and serves each on its own thread.
#[cfg(target_os = "linux")]
fn accept_loop(cid: u32, listener: UnixListener, limiter: Arc<SessionLimiter>) {
    while !SHUTDOWN.load(Ordering::SeqCst) {
        match poll_readable(listener.as_raw_fd(), ACCEPT_POLL_MS) {
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

/// Asks the process to stop on `SIGTERM`/`SIGINT`.
///
/// The handler only sets a flag (the async-signal-safe thing to do); the
/// accept loop notices it within [`ACCEPT_POLL_MS`], and `SA_RESTART` is left
/// off so an interrupted wait cannot hold the shutdown back.
#[cfg(target_os = "linux")]
fn install_signal_handlers() {
    // SAFETY: `on_signal` only stores to a static `AtomicBool`, which is
    // async-signal-safe; `sigaction` is given a fully initialised action whose
    // handler stays alive for the process's lifetime.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as usize;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = 0;
        for signal in [libc::SIGTERM, libc::SIGINT] {
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

/// The signal handler: request a clean stop.
#[cfg(target_os = "linux")]
extern "C" fn on_signal(_signal: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
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
