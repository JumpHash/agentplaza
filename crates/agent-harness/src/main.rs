//! `agent-harness` — the AgentPlaza in-guest agent harness v0.
//!
//! The harness is the guest end of the plaza's core loop: it runs inside the
//! microVM `init` forked, reads its identity from the kernel command line
//! (`plaza.cid=`, `plaza.port=`, `plaza.vm=`), opens the one AF_VSOCK
//! connection to the orchestrator (`VMADDR_CID_HOST`, port 5000 by default)
//! and performs the frozen session: `hello` → `welcome` → `echo` tool call →
//! `uptime` tool call → `bye`. There is no NIC and no other network path: the
//! plaza rule is that a guest reaches the orchestrator over vsock only.
//!
//! Every line in both directions, and every step's outcome, is printed to
//! standard output — which is the guest's serial console — so the whole loop
//! is visible in the VM's transcript. The exit status is 0 only for a session
//! that completed with `bye`; anything else prints the error and exits
//! non-zero. Malformed input from the orchestrator is logged and skipped
//! (bounded), never a panic.
//!
//! AF_VSOCK exists on Linux only, so the crate's code is Linux-gated: the
//! workspace is also type-checked for `aarch64-apple-darwin`, where this
//! binary must still build, and there it refuses to run rather than pretend to
//! be a guest harness.

// This crate is safe Rust only: `deny(unsafe_code)` makes any `unsafe` outside
// the one enumerated module below a compile error. The single entry is
//   * `vsock` — the AF_VSOCK syscall boundary (`socket`/`connect`/`poll`/
//     `fcntl`/`ioctl`/`read`/`write`/`close`), which `std` does not expose.
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
mod cmdline;
#[cfg(target_os = "linux")]
mod line;
#[cfg(target_os = "linux")]
mod proto;
#[cfg(target_os = "linux")]
mod session;
#[cfg(target_os = "linux")]
mod vsock;

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
use std::time::Duration;

#[cfg(target_os = "linux")]
use cmdline::{Config, ConfigError};
#[cfg(target_os = "linux")]
use line::StreamLines;
#[cfg(target_os = "linux")]
use session::{Identity, SessionError};
#[cfg(target_os = "linux")]
use vsock::{CID_ANY, HOST_CID, VsockStream, device_guest_cid};

/// The kernel command line the VMM built for this guest.
#[cfg(target_os = "linux")]
const CMDLINE_PATH: &str = "/proc/cmdline";

/// How many times the harness tries to connect before giving up.
#[cfg(target_os = "linux")]
const CONNECT_ATTEMPTS: usize = 40;

/// How long it waits between connect attempts.
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
        }
    }
}

#[cfg(target_os = "linux")]
impl std::error::Error for HarnessError {}

#[cfg(target_os = "linux")]
fn run() -> Result<(), HarnessError> {
    let cmdline = fs::read_to_string(CMDLINE_PATH).map_err(HarnessError::Cmdline)?;
    let config = Config::parse(&cmdline).map_err(HarnessError::Flags)?;

    let mut out = io::stdout().lock();
    let _ = writeln!(
        out,
        "agent-harness: vm {} -> host CID {} port {}",
        config.vm, HOST_CID, config.port
    );

    let (stream, attempts) = connect(&config, &mut out).map_err(HarnessError::Connect)?;
    let _ = writeln!(out, "agent-harness: connected after {attempts} attempt(s)");

    // The CID is the device's answer, never the command line's: one command
    // line is shared by every VM of a spawn, while the device gives each VM
    // its own CID and stamps that CID into every packet.
    let guest_cid = device_guest_cid().map_err(HarnessError::GuestCid)?;
    if guest_cid == CID_ANY || cid_is_reserved(guest_cid) {
        return Err(HarnessError::UnassignedCid(guest_cid));
    }
    let _ = writeln!(
        out,
        "agent-harness: the vsock device assigned guest CID {guest_cid}"
    );

    let identity = Identity {
        guest_cid,
        vm: config.vm,
    };
    let mut transport = StreamLines::new(stream);
    session::run(&mut transport, &mut out, &identity).map_err(HarnessError::Session)?;
    let _ = writeln!(out, "agent-harness: closing the connection");
    let _ = out.flush();
    Ok(())
}

/// Whether `cid` is one the vsock stack reserves (0 `HYPERVISOR`, 1 `LOCAL`
/// and 2 `HOST`), i.e. not a guest CID a session may be conducted under.
#[cfg(target_os = "linux")]
fn cid_is_reserved(cid: u32) -> bool {
    cid <= HOST_CID
}

/// Connects to the orchestrator, retrying for a bounded window, and returns
/// the stream together with the number of attempts it took.
///
/// The vsock transport is registered by a kernel module the guest's `init`
/// loads immediately before it starts this binary; the first connect can beat
/// that registration by a moment, so a refused connection is retried rather
/// than reported as a failure on the first try.
#[cfg(target_os = "linux")]
fn connect(config: &Config, log: &mut impl Write) -> io::Result<(VsockStream, usize)> {
    let mut last_error: Option<io::Error> = None;
    for attempt in 1..=CONNECT_ATTEMPTS {
        match VsockStream::connect(HOST_CID, config.port) {
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
