//! Clean shutdown: a stop signal runs the final state writes before the process exits.
//!
//! `install` blocks SIGTERM and SIGINT in the calling thread, which is `main` before any other thread starts. Every
//! later thread inherits that mask, so no asynchronous handler runs. A thread named `shutdown` waits in `sigwait`;
//! when a signal arrives it runs the closure set by `on_stop` and exits with status 0 (at once, if none is set). If
//! the closure is still running after 20 s, a watchdog thread exits with status 1 instead, without the final writes.
//!
//! Only the signal-mask calls below need `unsafe`. `std::process::Command` resets the signal mask in each child, so
//! the blocked signals do not reach the processes apid starts, and those stay killable.
//!
//! Unix only: the calls come from `libc`, which apid depends on for Unix alone. Elsewhere `install` does nothing and a
//! stop signal keeps its default action, as before.
#![allow(unsafe_code)]

use std::sync::OnceLock;

type StopFn = Box<dyn Fn() + Send + Sync>;

static ON_STOP: OnceLock<StopFn> = OnceLock::new();

/// How long the stop closure may run before the watchdog exits without waiting for it.
#[cfg(unix)]
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Sets the closure that runs when a stop signal arrives. The first one set is kept.
pub fn on_stop(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_STOP.set(Box::new(f));
}

/// Blocks SIGTERM and SIGINT in this thread and starts the thread that waits for them. Call it first in `main`,
/// before any thread starts, so that every thread inherits the mask.
#[cfg(unix)]
pub fn install() {
    // SAFETY: a sigset_t is plain data; sigemptyset initialises it before sigaddset adds the two signals.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
    }
    // SAFETY: `set` is initialised above; a null old-mask pointer asks for nothing back.
    let rc = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    assert_eq!(rc, 0, "apid: cannot block the stop signals");
    std::thread::Builder::new()
        .name("shutdown".into())
        .spawn(move || wait(set))
        .expect("apid: cannot start the shutdown thread");
}

/// Elsewhere there is no stop handling yet: the signals keep their default action.
#[cfg(not(unix))]
pub fn install() {}

/// Waits for the stop signals; each one ends the process through `stop`.
#[cfg(unix)]
fn wait(set: libc::sigset_t) {
    loop {
        let mut signal: libc::c_int = 0;
        // SAFETY: `set` is a valid signal set and `signal` is a valid out-pointer for the call.
        let rc = unsafe { libc::sigwait(&set, &mut signal) };
        if rc != 0 {
            eprintln!("apid: sigwait failed ({rc})");
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        stop();
    }
}

/// Runs the stop closure, then exits. A watchdog thread exits with status 1 if the closure outlasts `STOP_TIMEOUT`.
/// A panic in the closure still exits, so the process never sits with the signals blocked and unstoppable.
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
