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
// API: `wait_readable` (`poll(2)` on Unix, `WSAPoll` on Windows) and
// `install_signal_handlers` (`sigaction(2)` on Unix, a console control handler
// on Windows). `shutdown_requested` and the `SHUTDOWN` flag are shared.
#![allow(unsafe_code)]

use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// Set by the signal or console handler; the accept loops read it.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// The write end of the shutdown waker (see `main.rs`), or -1 while none is set. The handler writes one byte to it,
/// so every accept loop blocked in `wait_readable` wakes at once.
static SHUTDOWN_WAKE: AtomicI64 = AtomicI64::new(-1);

/// Stores the write end of the shutdown waker for the signal or console handler to write to. The handle must stay
/// open for the process's life.
pub fn set_shutdown_wake(handle: RawHandle) {
    SHUTDOWN_WAKE.store(handle as i64, Ordering::SeqCst);
}

/// Whether `SIGTERM`/`SIGINT` (or a console stop event on Windows) has asked
/// the process to stop.
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// A handle that [`wait_readable`] can watch: a file descriptor on Unix, a socket on Windows.
#[cfg(unix)]
pub type RawHandle = std::os::fd::RawFd;
/// A handle that [`wait_readable`] can watch: a file descriptor on Unix, a socket on Windows.
#[cfg(windows)]
pub type RawHandle = std::os::windows::io::RawSocket;

/// The `timeout` argument of `poll`/`WSAPoll` in milliseconds: `-1` (no limit) for `None`, capped at `i32::MAX`.
fn timeout_millis(timeout: Option<std::time::Duration>) -> i32 {
    match timeout {
        None => -1,
        Some(dur) => dur.as_millis().min(i32::MAX as u128) as i32,
    }
}

/// Waits until one of `handles` is readable (or has hung up / errored), at most `timeout` (`None` waits without
/// limit). Returns one bool per handle, in order; all false on a timeout.
#[cfg(unix)]
pub fn wait_readable(
    handles: &[RawHandle],
    timeout: Option<std::time::Duration>,
) -> io::Result<Vec<bool>> {
    if handles.is_empty() {
        return Ok(Vec::new());
    }
    let timeout_ms = timeout_millis(timeout);
    let mut pollfds: Vec<libc::pollfd> = handles
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    loop {
        // SAFETY: `pollfds` holds one initialised entry per handle, and the vector stays alive and unmoved for the
        // call; `poll` may modify each `revents` in place and nothing else.
        let ret = unsafe {
            libc::poll(
                pollfds.as_mut_ptr(),
                pollfds.len() as libc::nfds_t,
                timeout_ms,
            )
        };
        if ret >= 0 {
            break;
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
    Ok(pollfds.iter().map(|p| p.revents != 0).collect())
}

/// Waits until one of `handles` is readable (or has hung up / errored), at most `timeout` (`None` waits without
/// limit). Returns one bool per handle, in order; all false on a timeout. Uses `WSAPoll` on the sockets.
#[cfg(windows)]
pub fn wait_readable(
    handles: &[RawHandle],
    timeout: Option<std::time::Duration>,
) -> io::Result<Vec<bool>> {
    use windows_sys::Win32::Networking::WinSock::{
        POLLRDNORM, SOCKET, WSAGetLastError, WSAPOLLFD, WSAPoll,
    };

    if handles.is_empty() {
        return Ok(Vec::new());
    }
    let timeout_ms = timeout_millis(timeout);
    let mut pollfds: Vec<WSAPOLLFD> = handles
        .iter()
        .map(|&sock| WSAPOLLFD {
            fd: sock as SOCKET,
            events: POLLRDNORM,
            revents: 0,
        })
        .collect();
    // SAFETY: `pollfds` holds one initialised entry per handle, and the vector stays alive and unmoved for the
    // call; `WSAPoll` may modify each `revents` in place and nothing else.
    let ret = unsafe { WSAPoll(pollfds.as_mut_ptr(), pollfds.len() as u32, timeout_ms) };
    if ret < 0 {
        // SAFETY: `WSAGetLastError` takes no arguments and only reads the
        // calling thread's last Winsock error.
        let code = unsafe { WSAGetLastError() };
        return Err(io::Error::from_raw_os_error(code));
    }
    Ok(pollfds.iter().map(|p| p.revents != 0).collect())
}

/// Asks the process to stop on `SIGTERM`/`SIGINT`.
///
/// The handler only sets [`SHUTDOWN`] and writes one byte to the shutdown waker
/// (both async-signal-safe), so the accept loops wake at once. `SA_RESTART` is
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
    // SAFETY: errno is thread-local, and reading it is async-signal-safe. It is saved here and restored at the
    // end, so the interrupted code sees the errno it had before the handler ran.
    #[cfg(target_os = "linux")]
    let saved = unsafe { *libc::__errno_location() };
    SHUTDOWN.store(true, Ordering::SeqCst);
    let fd = SHUTDOWN_WAKE.load(Ordering::SeqCst);
    if fd >= 0 {
        let byte = 1u8;
        // SAFETY: `write(2)` is async-signal-safe and reads one byte from the live local `byte`. `fd` is the
        // write end of the shutdown waker, which `main` keeps open for the process's life; a failed write is
        // ignored, since a full buffer means a wake is already pending.
        unsafe {
            libc::write(fd as libc::c_int, (&byte as *const u8).cast(), 1);
        }
    }
    // SAFETY: writing errno is async-signal-safe; this restores the value saved above on the same thread.
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = saved;
    }
}

/// Asks the process to stop on a console stop event (Ctrl+C, Ctrl+Break, the
/// console window closing, logoff, or system shutdown).
///
/// The handler only sets [`SHUTDOWN`] and sends one byte to the shutdown waker;
/// the accept loops wake at once.
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
    use windows_sys::Win32::Networking::WinSock::{SOCKET, send};
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };

    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
        | CTRL_SHUTDOWN_EVENT => {
            SHUTDOWN.store(true, Ordering::SeqCst);
            let sock = SHUTDOWN_WAKE.load(Ordering::SeqCst);
            if sock >= 0 {
                // SAFETY: `send` reads one byte from the live local array. `sock` is the write end of the
                // shutdown waker, a socket that `main` keeps open for the process's life; a failed send is
                // ignored, since a full buffer means a wake is already pending.
                unsafe {
                    send(sock as SOCKET, [1u8].as_ptr(), 1, 0);
                }
            }
            1
        }
        _ => 0,
    }
}

/// The host's uptime and idle time in seconds, formatted like the first two
/// fields of Linux's `/proc/uptime`: `GetTickCount64` for the uptime, and
/// `GetSystemTimes`' idle time (summed over all processors, as Linux sums it).
#[cfg(windows)]
pub fn host_uptime() -> io::Result<String> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    // SAFETY: no arguments; reads the system tick count.
    let up_ms = unsafe { GetTickCount64() };
    let mut idle = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = idle;
    let mut user = idle;
    // SAFETY: the three out-parameters are valid, writable FILETIMEs.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // FILETIME counts 100 ns intervals.
    let idle_100ns = (u64::from(idle.dwHighDateTime) << 32) | u64::from(idle.dwLowDateTime);
    Ok(format!(
        "{:.2} {:.2}",
        up_ms as f64 / 1000.0,
        idle_100ns as f64 / 10_000_000.0
    ))
}

/// Ends a read blocked on `stream` in another thread after `stream` was shut
/// down. On Unix the shutdown itself does that, so this does nothing.
#[cfg(unix)]
pub fn wake_readers(_stream: &std::net::TcpStream) {}

/// Ends a read blocked on `stream` in another thread after `stream` was shut
/// down. A WinSock shutdown does not complete a `recv` already waiting (and
/// after `SD_RECEIVE` the peer's FIN may never complete it), so the socket's
/// pending I/O is cancelled: the waiting read returns an error, which a relay
/// treats as the end of its direction.
#[cfg(windows)]
pub fn wake_readers(stream: &std::net::TcpStream) {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::IO::CancelIoEx;

    // SAFETY: a SOCKET is a kernel handle, live for as long as `stream` is
    // borrowed; CancelIoEx with a null OVERLAPPED only cancels its pending
    // I/O requests (failing harmlessly when there are none).
    unsafe { CancelIoEx(stream.as_raw_socket() as HANDLE, std::ptr::null()) };
}
