// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the daemon's raw-syscall boundary (`poll(2)` and
// `sigaction(2)`, which safe Rust cannot call).

// Syscall boundary: this module is the only entry on `vsockd`'s enumerated
// `#[allow(unsafe_code)]` allowlist (see `main.rs`). It exists so that the two
// C calls the daemon needs — `poll(2)` (the accept loop's shutdown-check
// cadence) and `sigaction(2)` (the `SIGTERM`/`SIGINT` stop request) — do not
// put an `#[allow(unsafe_code)]` on the file that owns the daemon's logic.
#![allow(unsafe_code)]

use std::io;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the signal handler; the accept loop reads it.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Whether `SIGTERM`/`SIGINT` has asked the process to stop.
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Polls `fd` for readability, returning `Ok(false)` on timeout. The timeout
/// is the accept loop's shutdown-check cadence, mirroring the old AF_VSOCK
/// `wait_readable`.
pub fn poll_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    // `pollfd` is fully initialised, and `fd` is borrowed from a live owner
    // for the duration of the call.
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pollfd` points to one initialised entry, which `poll` may
    // modify but whose storage stays valid across the call.
    let ret = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret > 0)
    }
}

/// Asks the process to stop on `SIGTERM`/`SIGINT`.
///
/// The handler only sets [`SHUTDOWN`] (the async-signal-safe thing to do); the
/// accept loop notices it within [`crate::ACCEPT_POLL_MS`], and `SA_RESTART` is
/// left off so an interrupted wait cannot hold the shutdown back.
pub fn install_signal_handlers() {
    // SAFETY: `on_signal` only stores to a static `AtomicBool`, which is
    // async-signal-safe; `sigaction` is given a fully initialised action whose
    // handler stays alive for the process's lifetime.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = 0;
        for signal in [libc::SIGTERM, libc::SIGINT] {
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

/// The signal handler: request a clean stop.
extern "C" fn on_signal(_signal: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}
