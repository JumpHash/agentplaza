//! Clean shutdown: a stop signal runs the final state writes before the process exits.
//!
//! `install` catches SIGTERM and SIGINT with a signal handler. The handler writes one byte to a pipe. A thread named
//! `shutdown` reads that byte, runs the closure set by `on_stop`, and exits with status 0 (at once, if none is set).
//! If the closure is still running after 20 s, a watchdog thread exits with status 1 instead, without the final
//! writes.
//!
//! No signal mask is changed. A caught signal reverts to its default action on exec, so the processes apid starts
//! (weft, the node link, git, and so on) still stop on SIGTERM or SIGINT, and the parent-death signal weft_hub.rs
//! gives weft reaches it when apid ends.
//!
//! `SA_RESTART` restarts most system calls that the handler interrupts. The handler only runs when apid is stopping
//! anyway.
//!
//! Unix only: the calls come from `libc`, which apid depends on for Unix alone. Elsewhere `install` does nothing and a
//! stop signal keeps its default action, as before.
#![allow(unsafe_code)]

use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

type StopFn = Box<dyn Fn() + Send + Sync>;

static ON_STOP: OnceLock<StopFn> = OnceLock::new();

/// How long the stop closure may run before the watchdog exits without waiting for it.
#[cfg(unix)]
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The write end of the stop pipe, for the signal handler. It stays -1 until `install` sets it.
#[cfg(unix)]
static WAKE: AtomicI32 = AtomicI32::new(-1);

/// Sets the closure that runs when a stop signal arrives. The first one set is kept.
pub fn on_stop(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_STOP.set(Box::new(f));
}

/// Catches SIGTERM and SIGINT and starts the `shutdown` thread that runs the stop closure. Call it once, early in
/// `main`. It does not need to run before other threads start, since it changes no signal mask.
#[cfg(unix)]
pub fn install() {
    let (reader, writer) = std::io::pipe().expect("apid: cannot create the stop pipe");
    // The write end is kept open for the life of the process, so the handler always has a pipe to write to. std
    // opens both ends close-on-exec, so no child inherits them.
    let fd = std::os::fd::IntoRawFd::into_raw_fd(writer);
    // SAFETY: `fd` is an open descriptor that this function owns; F_GETFL only reads its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert_ne!(flags, -1, "apid: cannot read the stop pipe's flags");
    // SAFETY: as above; O_NONBLOCK stops the handler from blocking on a full pipe.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    assert_ne!(rc, -1, "apid: cannot make the stop pipe non-blocking");
    WAKE.store(fd, Ordering::Release);
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: a sigaction is plain data, so zeros is a valid value; the fields the call reads are set below.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        // SAFETY: `action.sa_mask` is a valid sigset_t for sigemptyset to initialise.
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        // SAFETY: `action` is fully initialised above; a null old-action pointer asks for nothing back.
        let rc = unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) };
        assert_eq!(rc, 0, "apid: cannot handle the stop signals");
    }
    std::thread::Builder::new()
        .name("shutdown".into())
        .spawn(move || wait(reader))
        .expect("apid: cannot start the shutdown thread");
}

/// Elsewhere there is no stop handling yet: the signals keep their default action.
#[cfg(not(unix))]
pub fn install() {}

/// The stop signals' handler. It may only make async-signal-safe calls: it writes one byte to the stop pipe and puts
/// errno back as it found it.
#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    let fd = WAKE.load(Ordering::Acquire);
    if fd >= 0 {
        // SAFETY: errno's location is valid for this thread, and the handler puts back the value it saves.
        let saved = unsafe { *errno() };
        // SAFETY: `fd` is the open write end of the stop pipe, which is never closed; the byte is a stack array that
        // lives for the call.
        let _ = unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
        // SAFETY: as above, errno's location is valid for this thread.
        unsafe { *errno() = saved };
    }
}

/// errno's address for this thread, so that the signal handler can put errno back.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn errno() -> *mut libc::c_int {
    // SAFETY: returns this thread's errno location, which is always valid.
    unsafe { libc::__errno_location() }
}

/// errno's address for this thread, so that the signal handler can put errno back (Apple and BSD name it `__error`).
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn errno() -> *mut libc::c_int {
    // SAFETY: returns this thread's errno location, which is always valid.
    unsafe { libc::__error() }
}

/// Reads the stop pipe. Each byte that a stop signal writes ends the process through `stop`.
#[cfg(unix)]
fn wait(mut reader: std::io::PipeReader) {
    use std::io::Read;
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            // The write end is never closed, so this cannot happen. If it did, nothing could wake this thread.
            Ok(0) => {
                eprintln!("apid: the stop pipe closed");
                return;
            }
            Ok(_) => stop(),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                eprintln!("apid: reading the stop pipe failed ({e})");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
}

/// Runs the stop closure, then exits. A watchdog thread exits with status 1 if the closure outlasts `STOP_TIMEOUT`.
/// A panic in the closure still exits, so a stop signal always ends the process.
#[cfg(unix)]
fn stop() -> ! {
    let watchdog = std::thread::Builder::new()
        .name("shutdown-watchdog".into())
        .spawn(|| {
            std::thread::sleep(STOP_TIMEOUT);
            eprintln!("apid: stopping took longer than 20 s; exiting without the final write");
            std::process::exit(1);
        });
    if watchdog.is_err() {
        eprintln!("apid: cannot start the shutdown watchdog; the final writes have no time limit");
    }
    if let Some(run) = ON_STOP.get() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    }
    std::process::exit(0);
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    /// The bug this guards: a stop-signal mask in apid was inherited by every child, which then ignored SIGTERM.
    #[test]
    fn a_process_started_after_install_still_stops_on_sigterm() {
        super::install();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep starts");
        let pid = libc::pid_t::try_from(child.id()).expect("a pid fits");
        // SAFETY: `pid` is a live child's pid; SIGTERM is a valid signal.
        let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
        assert_eq!(rc, 0);
        let status = child.wait().expect("the child is reaped");
        assert_eq!(
            status.signal(),
            Some(libc::SIGTERM),
            "the child ended by SIGTERM, not after its 30 s: {status:?}"
        );
    }
}
