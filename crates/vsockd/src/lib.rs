//! The vsockd library: the egress proxy (`egress`, with its TLS intercept in `egress_tls`) and the modules it
//! stands on. The `vsockd` binary serves the guest CIDs with them; `noded` runs the same proxy for each Mac
//! project account, so a sandboxed session's connections are checked and injected the way a guest's are.

// Safe Rust except `sys`, the `poll(2)`/`sigaction(2)` boundary.
#![deny(unsafe_code)]

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod apid;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod egress;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod sys;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod waker;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
use apz_uds::UnixStream;

/// The shutdown waker, created by `shutdown_waker`.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
static SHUTDOWN_WAKER: std::sync::OnceLock<crate::waker::Waker> = std::sync::OnceLock::new();

/// Woken once when the daemon is asked to stop, so every accept thread returns at once. Never drained.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub fn shutdown_waker() -> &'static crate::waker::Waker {
    SHUTDOWN_WAKER.get_or_init(|| {
        let waker = crate::waker::Waker::new().expect("vsockd: cannot create the shutdown waker");
        sys::set_shutdown_wake(waker.wake_handle());
        waker
    })
}
/// Test helper: everything `stream` sends until it closes, read with short polled timeouts up to
/// `deadline`. On Windows a read already blocked on an AF_UNIX socket is not reliably woken when
/// the peer closes, but a fresh one sees the close; so no read here waits longer than a poll.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub fn read_until_closed(stream: &UnixStream, deadline: Duration) -> Vec<u8> {
    use std::io::{ErrorKind, Read};
    // macOS refuses a read timeout on a socket whose peer has already closed (EINVAL). Then the reads
    // below return what is buffered and the close at once, so the timeout is not needed.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
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
