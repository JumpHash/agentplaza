//! `agent-harness` — the AgentPlaza in-guest agent harness.
//!
//! The harness is the guest end of the plaza's core loop: it runs inside the
//! microVM `init` forked, reads its identity from the kernel command line
//! (`plaza.port=`, `plaza.vm=`, `plaza.role=`, `plaza.keepalive=1`), opens the
//! one AF_VSOCK connection to the orchestrator (`VMADDR_CID_HOST`, port 5000 by
//! default) and speaks one of the two sessions:
//!
//! * **v1** (no `plaza.keepalive=1`): the frozen one-shot session —
//!   `hello` → `welcome` → `echo` tool call → `uptime` tool call → `bye`. The
//!   guest's `init` acts on its exit status (a completed session reboots the
//!   guest). This is byte-for-byte what the acceptance suites drive.
//! * **keepalive** (`plaza.keepalive=1`, the agent fleet): the long-lived
//!   session — `hello` (with the role) → `welcome` → a heartbeat every 5 s and
//!   a `tool_result` for every `tool_call` the hub sends, until the hub says
//!   `stop`, which is answered with `bye`. This mode never exits on its own
//!   account: any failure is printed and retried with a jittered backoff, and
//!   the guest's `init` keeps the VM up when it does end. The tools it serves
//!   are `echo` and `uptime`, the download into the guest's writable `/workspace`
//!   volume, and the agent's own shell and file tools (`read`, `edit`, `find`,
//!   `search`, `run`, `run_tests`) — see [`crate::tools`], [`crate::files`] and
//!   [`crate::v2`].
//!
//! There is no NIC and no other network path: the plaza rule is that a guest
//! reaches the orchestrator over vsock only. The harness holds no secrets and
//! decides nothing: it never opens a tool call, never picks an action and never
//! ends a session of its own accord.
//!
//! Every line in both directions, and every step's outcome, is printed to
//! standard output — which is the guest's serial console — so the whole loop
//! is visible in the VM's transcript. In v1 the exit status is 0 only for a
//! session that completed with `bye`; anything else prints the error and exits
//! non-zero. Malformed input from the orchestrator is logged and skipped
//! (bounded), never a panic.
//!
//! AF_VSOCK exists on Linux only, so the crate's code is Linux-gated: the
//! workspace is also type-checked for `aarch64-apple-darwin`, where this
//! binary must still build, and there it refuses to run rather than pretend to
//! be a guest harness.

// Safe Rust except the two syscall boundaries `std` lacks: `vsock` (AF_VSOCK) and `term` (the PTY calls).
#![deny(unsafe_code)]
// Static glibc warns that getaddrinfo/getpwuid_r need its NSS libraries at runtime. The guest never
// resolves a name: the proxy is the literal 127.0.0.1:3128 and ureq sends every host through it.
#![cfg_attr(
    all(target_os = "linux", target_feature = "crt-static"),
    allow(linker_messages)
)]

#[cfg(target_os = "linux")]
mod acp;
#[cfg(target_os = "linux")]
mod agent;
#[cfg(target_os = "linux")]
mod anchors;
#[cfg(target_os = "linux")]
mod bash;
#[cfg(target_os = "linux")]
mod checkpoints;
#[cfg(target_os = "linux")]
mod code_intel;
// Pure parsing, built on every host so its tests run anywhere; only the Linux guest calls it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod cmdline;
#[cfg(target_os = "linux")]
mod egress;
#[cfg(target_os = "linux")]
mod engines;
#[cfg(target_os = "linux")]
mod envd;
#[cfg(target_os = "linux")]
mod files;
#[cfg(target_os = "linux")]
mod helpers;
#[cfg(target_os = "linux")]
mod ide;
#[cfg(target_os = "linux")]
mod keepalive;
#[cfg(target_os = "linux")]
mod lane;
#[cfg(target_os = "linux")]
mod library;
#[cfg(target_os = "linux")]
mod plan;
#[cfg(target_os = "linux")]
mod restore;
#[cfg(target_os = "linux")]
mod script;
#[cfg(target_os = "linux")]
mod session;
#[cfg(target_os = "linux")]
mod term;
#[cfg(target_os = "linux")]
mod tool_batch;
#[cfg(target_os = "linux")]
mod tool_search;
#[cfg(target_os = "linux")]
mod tools;
#[cfg(target_os = "linux")]
mod trust;
#[cfg(target_os = "linux")]
mod v2;
#[cfg(target_os = "linux")]
mod vsock;
#[cfg(target_os = "linux")]
mod weft_lane;

use std::process::ExitCode;

#[cfg(target_os = "linux")]
use std::fmt;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::thread::sleep;
#[cfg(target_os = "linux")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
use apz_proto::line::Lines;
#[cfg(target_os = "linux")]
use cmdline::{Config, ConfigError};
#[cfg(target_os = "linux")]
use files::Workspace;
#[cfg(target_os = "linux")]
use session::{Identity, SessionError};
#[cfg(target_os = "linux")]
use vsock::{CID_ANY, HOST_CID, VsockStream, device_guest_cid};

/// The kernel command line the VMM built for this guest.
#[cfg(target_os = "linux")]
const CMDLINE_PATH: &str = "/proc/cmdline";

/// How many times the v1 session tries to connect before giving up.
#[cfg(target_os = "linux")]
const CONNECT_ATTEMPTS: usize = 40;

/// How long it waits between v1 connect attempts.
#[cfg(target_os = "linux")]
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// On a host without AF_VSOCK there is no guest to be a harness in: say so and
/// fail, rather than building a binary that cannot do its job.
#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("agent-harness: AF_VSOCK is Linux-only; this host is not a supported guest");
    ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    // An engine's MCP server for the plaza's tools, started by the engine on its stdio: it is not
    // the vsock session, and its stdout is the protocol, so it reports errors on stderr only.
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, socket] = args.as_slice()
        && flag == "--plaza-mcp"
    {
        return match acp::mcp::run_stdio(std::path::Path::new(socket)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("agent-harness: the plaza's tools: {err}");
                ExitCode::FAILURE
            }
        };
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let mut out = io::stdout().lock();
            let _ = writeln!(out, "agent-harness: {err}");
            let _ = out.flush();
            ExitCode::FAILURE
        }
    }
}

/// Why the harness stopped early.
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum HarnessError {
    /// The guest's command line could not be read: {0}
    Cmdline(io::Error),
    /// The guest's command line does not describe this VM: {0}
    Flags(ConfigError),
    /// The orchestrator could not be reached: {0}
    Connect(io::Error),
    /// The vsock device did not publish this VM's CID: {0}
    GuestCid(io::Error),
    /// The vsock device published no useful guest CID (it answered {0})
    UnassignedCid(u32),
    /// The session did not complete: {0}
    Session(SessionError),
    /// The sandbox daemon could not start: {0}
    Sandbox(io::Error),
}

#[cfg(target_os = "linux")]
impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cmdline(err) => write!(f, "cannot read {CMDLINE_PATH}: {err}"),
            Self::Flags(err) => write!(f, "unusable kernel command line: {err}"),
            Self::Connect(err) => write!(f, "cannot reach the orchestrator: {err}"),
            Self::GuestCid(err) => write!(f, "cannot read this VM's guest CID: {err}"),
            Self::UnassignedCid(cid) => write!(
                f,
                "the vsock device published no guest CID (it answered {cid}): the VM has no CID to report"
            ),
            Self::Session(err) => write!(f, "session failed: {err}"),
            Self::Sandbox(err) => write!(f, "sandbox daemon failed: {err}"),
        }
    }
}

#[cfg(target_os = "linux")]
impl std::error::Error for HarnessError {}

#[cfg(target_os = "linux")]
fn run() -> Result<(), HarnessError> {
    let cmdline = match cmdline_flag() {
        Some(flags) => flags,
        None => fs::read_to_string(CMDLINE_PATH).map_err(HarnessError::Cmdline)?,
    };
    let config = Config::parse(&cmdline).map_err(HarnessError::Flags)?;

    // Not the locked handle: the session's lanes write the transcript from other threads, and a
    // lock guard is not `Send`. Each write takes stdout's own lock, so lines stay whole.
    let mut out = io::stdout();
    let _ = writeln!(
        out,
        "agent-harness: vm {} -> host CID {} port {}",
        config.vm, HOST_CID, config.port
    );

    if config.sandbox {
        sandbox(&mut out, config.template)
    } else if config.keepalive {
        keepalive(&config, &mut out)
    } else {
        one_shot(&config, &mut out)
    }
}

/// The command line a process was started with, when it names one (`--cmdline <flags>`): the eval suite starts its
/// harnesses this way, where a VM reads `/proc/cmdline`.
#[cfg(target_os = "linux")]
fn cmdline_flag() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == "--cmdline")
        .map(|pair| pair[1].clone())
}

/// The sandbox's home, and the user its processes run as (E2B's default user).
#[cfg(target_os = "linux")]
const SANDBOX_HOME: &str = "/home/user";
#[cfg(target_os = "linux")]
const SANDBOX_USER: u32 = bash::UNPRIVILEGED;

/// Serves the sandbox daemon (E2B's envd) on its vsock port, forever: no hub session. The egress
/// shim runs as it does for an agent, so the sandbox's processes reach the internet the same way.
/// `/home/user` is the workspace volume, linked in when the image does not have it.
#[cfg(target_os = "linux")]
fn sandbox(out: &mut (impl Write + Send), template: bool) -> Result<(), HarnessError> {
    egress::spawn_shim();
    // Best effort: a home that cannot be linked is reported by the first command that needs it
    // (its working directory is then refused), not by the daemon failing to start.
    let _ = fs::create_dir_all("/workspace");
    if fs::symlink_metadata(SANDBOX_HOME).is_err() {
        let _ = fs::create_dir_all("/home");
        if let Err(err) = std::os::unix::fs::symlink("/workspace", SANDBOX_HOME) {
            let _ = writeln!(
                out,
                "agent-harness: sandbox: cannot link {SANDBOX_HOME}: {err}"
            );
        }
    }
    let _ = writeln!(out, "agent-harness: sandbox: home {SANDBOX_HOME}");
    let _ = out.flush();
    let ctx = envd::Ctx {
        home: SANDBOX_HOME.into(),
        uid: SANDBOX_USER,
        gid: SANDBOX_USER,
        user_uid: SANDBOX_USER,
        user_gid: SANDBOX_USER,
        workdir: None,
        env: Vec::new(),
    };
    envd::serve(envd::PORT, ctx, template).map_err(HarnessError::Sandbox)
}

/// Runs the frozen v1 session once and returns when it completed.
///
/// The caller (the guest's `init`) acts on how this ended: a completed session
/// means the guest reboots, and a failure is reported with the guest left up.
#[cfg(target_os = "linux")]
fn one_shot(config: &Config, out: &mut impl Write) -> Result<(), HarnessError> {
    let (stream, attempts) = connect(config, out).map_err(HarnessError::Connect)?;
    let _ = writeln!(out, "agent-harness: connected after {attempts} attempt(s)");

    let guest_cid = guest_cid(config, out)?;
    let identity = Identity {
        guest_cid,
        vm: config.vm,
    };
    let mut lines = Lines::new(stream);
    session::run(&mut lines, out, &identity).map_err(HarnessError::Session)?;
    let _ = writeln!(out, "agent-harness: closing the connection");
    let _ = out.flush();
    Ok(())
}

/// Runs the long-lived keepalive agent until the hub stops it.
///
/// This is the one mode that does not end by itself: the hub's `stop` is the
/// only clean end, and every other outcome — the vsock device not being up yet,
/// a refused connection, a hub that went away mid-session, a hub that does not
/// speak protocol v2 — is printed and retried with a jittered backoff. The
/// agent stays up and tries again; it never decides that its work is done.
#[cfg(target_os = "linux")]
fn keepalive(config: &Config, out: &mut (impl Write + Send)) -> Result<(), HarnessError> {
    // The egress shim serves the guest's outbound HTTP for as long as the agent
    // runs; a bind failure is printed there, never fatal here.
    egress::spawn_shim();
    if config.template {
        // The host snapshots a template while it waits in the loop below; this line, on the
        // serial console, is how the host knows the guest has reached it.
        let _ = writeln!(out, "agent-harness: template ready");
        let _ = out.flush();
    }
    let mut failures = 0u32;
    // Set by the first welcome the hub accepts. A template retries quickly only until then.
    let mut welcomed = false;
    // Quick attempts left after a drop following an adoption (a restore's transport reset among them).
    let mut fast_left = 0u32;
    loop {
        match keepalive_once(config, out) {
            Ok(calls) => {
                let _ = writeln!(
                    out,
                    "agent-harness: the hub stopped the agent after {calls} tool call(s); \
                     the guest stays up"
                );
                let _ = out.flush();
                return Ok(());
            }
            Err((adopted, reset, reason)) => {
                // A successful welcome resets the schedule: the counter counts
                // consecutive failures since the hub last accepted this agent,
                // so a drop after adoption is a 250 ms hop, not a 30 s wait.
                failures = if adopted {
                    1
                } else {
                    failures.saturating_add(1)
                };
                welcomed |= adopted;
                // A drop after the hub accepted this agent (a restore's transport reset among them) opens a
                // short run of quick attempts: the hub is normally still there, so the agent is back within
                // milliseconds. Only RESTORE_FAST_TRIES of them, so a daemon that is really gone costs each guest
                // a handful of connects before the normal backoff.
                if reset || adopted {
                    fast_left = keepalive::RESTORE_FAST_TRIES;
                }
                let after_restore = fast_left > 0;
                if after_restore {
                    fast_left -= 1;
                }
                let delay = keepalive::reconnect_delay(
                    config.template && !welcomed,
                    after_restore,
                    failures,
                    jitter_nanos(),
                );
                // A template waiting for its restore retries every 25 ms: only its first failure
                // is printed, so the serial console (and the template's memory) is not flooded.
                if !config.template || welcomed || failures == 1 {
                    let restored = if after_restore { " (quick retry)" } else { "" };
                    let _ = writeln!(
                        out,
                        "agent-harness: keepalive: {reason}; reconnecting in {} ms{restored} (failure {failures})",
                        delay.as_millis()
                    );
                    let _ = out.flush();
                }
                sleep(delay);
            }
        }
    }
}

/// One keepalive attempt: this VM's CID, the connection, and one session.
///
/// Fails with whether the hub had adopted the agent, whether the connection died to a vsock
/// transport reset (a restore), and a one-line reason (the setup steps' or the session's own) —
/// the reconnect loop is the only caller, and what it needs from a failure is those three things.
#[cfg(target_os = "linux")]
fn keepalive_once(
    config: &Config,
    out: &mut (impl Write + Send),
) -> Result<u64, (bool, bool, String)> {
    let guest_cid = guest_cid(config, out).map_err(|err| (false, false, err.to_string()))?;
    let stream = open_stream(config).map_err(|err| {
        (
            false,
            false,
            format!("cannot reach the orchestrator: {err}"),
        )
    })?;
    let identity = Identity {
        guest_cid,
        vm: config.vm,
    };
    // The volume the file tools work in: the guest's init mounted the plaza's
    // workspace image at /workspace before it started this binary (see
    // `crate::files`). It is created per attempt — it is a path, not a handle —
    // so a guest that never had the volume is reported by the tools that need
    // it, not by the session.
    let workspace = Workspace::default();
    let mut lines = Lines::new(stream);
    keepalive::session(
        &mut lines,
        out,
        &identity,
        &workspace,
        config.role.as_deref(),
        keepalive::heartbeat_interval(),
    )
    .map_err(|failure| {
        (
            failure.adopted,
            failure.transport_reset(),
            failure.to_string(),
        )
    })
}

/// Opens the session's stream: the Unix socket a `plaza.connect=uds:` line names, or the orchestrator over AF_VSOCK.
#[cfg(target_os = "linux")]
fn open_stream(config: &Config) -> io::Result<VsockStream> {
    match &config.connect {
        Some(path) => VsockStream::connect_unix(path),
        None => VsockStream::connect(HOST_CID, config.port),
    }
}

/// Reads the guest CID the vsock device assigned this VM, logging it. On the test transport the CID is the one
/// the socket is named for (vsockd serves each CID on its own socket).
///
/// The CID is the device's answer, never the command line's: one command line
/// is shared by every VM of a spawn, while the device gives each VM its own CID
/// and stamps that CID into every packet.
#[cfg(target_os = "linux")]
fn guest_cid(config: &Config, out: &mut impl Write) -> Result<u32, HarnessError> {
    let guest_cid = match &config.connect {
        Some(path) => cmdline::socket_cid(path).ok_or_else(|| {
            HarnessError::GuestCid(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{path} is not named vm<cid>_<port>"),
            ))
        })?,
        None => device_guest_cid().map_err(HarnessError::GuestCid)?,
    };
    if guest_cid == CID_ANY || cid_is_reserved(guest_cid) {
        return Err(HarnessError::UnassignedCid(guest_cid));
    }
    let _ = writeln!(
        out,
        "agent-harness: the vsock device assigned guest CID {guest_cid}"
    );
    Ok(guest_cid)
}

/// Whether `cid` is one the vsock stack reserves (0 `HYPERVISOR`, 1 `LOCAL`
/// and 2 `HOST`), i.e. not a guest CID a session may be conducted under.
#[cfg(target_os = "linux")]
fn cid_is_reserved(cid: u32) -> bool {
    cid <= HOST_CID
}

/// A jitter source for the reconnect backoff.
///
/// The clock's sub-second field is enough to decorrelate a fleet of agents that
/// lost the same hub (each VM has its own clock), and it costs no dependency —
/// a harness that holds no secrets also takes no crates it does not need.
#[cfg(target_os = "linux")]
fn jitter_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| u64::from(since.subsec_nanos()))
        .unwrap_or(0)
}

/// Connects to the orchestrator, retrying for a bounded window, and returns
/// the stream together with the number of attempts it took.
///
/// The vsock transport is registered by a kernel module the guest's `init`
/// loads immediately before it starts this binary; the first connect can beat
/// that registration by a moment, so a refused connection is retried rather
/// than reported as a failure on the first try. The keepalive session leans on
/// its own reconnect loop instead, which covers the same window and more.
#[cfg(target_os = "linux")]
fn connect(config: &Config, log: &mut impl Write) -> io::Result<(VsockStream, usize)> {
    let mut last_error: Option<io::Error> = None;
    for attempt in 1..=CONNECT_ATTEMPTS {
        match open_stream(config) {
            Ok(stream) => return Ok((stream, attempt)),
            Err(err) => {
                let _ = writeln!(
                    log,
                    "agent-harness: connect attempt {attempt}/{CONNECT_ATTEMPTS} failed: {err}"
                );
                // A refused or unroutable connection can be a transport that is
                // not registered yet, so it is worth retrying. A connect the
                // device took and never completed is not going to fix itself.
                if err.kind() == io::ErrorKind::TimedOut {
                    return Err(err);
                }
                last_error = Some(err);
            }
        }
        if attempt < CONNECT_ATTEMPTS {
            sleep(CONNECT_RETRY_DELAY);
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("the connect loop made no attempt")))
}
