// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the crate's raw-syscall boundary (the C `poll(2)` and
// `uname(2)` calls, which safe Rust cannot make). Everything else in the crate
// is safe Rust under a crate-level `#![deny(unsafe_code)]`.

// Syscall boundary: this module is one of the crate's enumerated
// `#[allow(unsafe_code)]` modules (see `lib.rs`). It exists so that the two
// one-line C calls the crate needs — `poll(2)` (the builder's exit-event wait)
// and `uname(2)` (the aarch64 kernel-version check) — do not force an
// `#[allow(unsafe_code)]` onto the modules that call them, which are otherwise
// entirely safe.
#![allow(unsafe_code)]

use std::io;
use std::os::fd::RawFd;

/// Waits until `fd` is readable, or until `timeout_ms` elapses (`-1` waits
/// forever). Returns `Ok(false)` on timeout.
///
/// `poll(2)` has no safe equivalent in `std`; this is the whole of its use.
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

/// The running kernel's release string (`uname -r`), or `None` if the
/// syscall fails.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub fn uname_release() -> Option<String> {
    // SAFETY: `utsname` is a plain C struct of fixed-size char arrays and
    // integers; an all-zero value is a valid (if unusable) initialization of
    // it, and `uname` overwrites it on success.
    let mut utsname: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `utsname` points to a valid, writable `libc::utsname` for the
    // duration of the call.
    if unsafe { libc::uname(&mut utsname) } < 0 {
        return None;
    }
    // SAFETY: `uname` guarantees `release` is a NUL-terminated C string.
    let release = unsafe { std::ffi::CStr::from_ptr(utsname.release.as_ptr()) };
    Some(release.to_string_lossy().into_owned())
}
