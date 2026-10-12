// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the daemon's raw-syscall boundary. On Unix that is
// `poll(2)` and `sigaction(2)`; on Windows it is `WSAPoll` (readiness of the
// listener socket) and `SetConsoleCtrlHandler` (the stop request), which safe
// Rust cannot call.

// Syscall boundary: this module is the only entry on `vsockd`'s enumerated
// `#[allow(unsafe_code)]` allowlist (see `main.rs`). It exists so that the C
// calls the daemon needs — the accept loop's shutdown-check cadence and the
// stop request — do not put an `#[allow(unsafe_code)]` on the file that owns
// the daemon's logic. Each platform has its own implementation of the same
// API: `poll_readable` (`poll(2)` on Unix, `WSAPoll` on Windows) and
// `install_signal_handlers` (`sigaction(2)` on Unix, a console control handler
// on Windows). `shutdown_requested` and the `SHUTDOWN` flag are shared.
#![allow(unsafe_code)]

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the signal or console handler; the accept loop reads it.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Whether `SIGTERM`/`SIGINT` (or a console stop event on Windows) has asked
/// the process to stop.
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Waits up to `timeout_ms` for `listener` to have a connection to accept,
/// returning `Ok(false)` on timeout. The timeout is the accept loop's
/// shutdown-check cadence, mirroring the old AF_VSOCK `wait_readable`.
#[cfg(unix)]
pub fn poll_readable(listener: &apz_uds::UnixListener, timeout_ms: i32) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    // `pollfd` is fully initialised, and `fd` is borrowed from the live
    // `listener` for the duration of the call.
    let mut pollfd = libc::pollfd {
        fd: listener.as_raw_fd(),
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

/// Waits up to `timeout_ms` for `listener` to have a connection to accept,
/// returning `Ok(false)` on timeout. Uses `WSAPoll` on the listener socket.
#[cfg(windows)]
pub fn poll_readable(listener: &apz_uds::UnixListener, timeout_ms: i32) -> io::Result<bool> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        POLLRDNORM, SOCKET, WSAGetLastError, WSAPOLLFD, WSAPoll,
    };

    // `pollfd` is fully initialised, and the socket is borrowed from the live
    // `listener` for the duration of the call.
    let mut pollfd = WSAPOLLFD {
        fd: listener.as_raw_socket() as SOCKET,
        events: POLLRDNORM,
        revents: 0,
    };
    // SAFETY: `pollfd` points to one initialised entry, which `WSAPoll` may
    // modify but whose storage stays valid across the call.
    let ret = unsafe { WSAPoll(&mut pollfd, 1, timeout_ms) };
    if ret < 0 {
        // SAFETY: `WSAGetLastError` takes no arguments and only reads the
        // calling thread's last Winsock error.
        let code = unsafe { WSAGetLastError() };
        Err(io::Error::from_raw_os_error(code))
    } else {
        Ok(ret > 0)
    }
}

/// Asks the process to stop on `SIGTERM`/`SIGINT`.
///
/// The handler only sets [`SHUTDOWN`] (the async-signal-safe thing to do); the
/// accept loop notices it within [`crate::ACCEPT_POLL_MS`], and `SA_RESTART` is
/// left off so an interrupted wait cannot hold the shutdown back.
#[cfg(unix)]
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
#[cfg(unix)]
extern "C" fn on_signal(_signal: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Asks the process to stop on a console stop event (Ctrl+C, Ctrl+Break, the
/// console window closing, logoff, or system shutdown).
///
/// The handler only sets [`SHUTDOWN`]; the accept loop notices it within
/// [`crate::ACCEPT_POLL_MS`].
#[cfg(windows)]
pub fn install_signal_handlers() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    // SAFETY: `on_console_ctrl` is an `extern "system"` function with the
    // handler signature, and it stays valid for the process's lifetime. It
    // only stores to a static `AtomicBool`.
    unsafe {
        SetConsoleCtrlHandler(Some(on_console_ctrl), 1);
    }
}

/// The console control handler: request a clean stop on the stop events, and
/// report them as handled (`TRUE`).
#[cfg(windows)]
unsafe extern "system" fn on_console_ctrl(ctrl_type: u32) -> i32 {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };

    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
        | CTRL_SHUTDOWN_EVENT => {
            SHUTDOWN.store(true, Ordering::SeqCst);
            1
        }
        _ => 0,
    }
}
