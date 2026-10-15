// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the crate's raw-syscall boundary (the C `poll(2)`,
// `uname(2)` and `socket(2)`/`connect(2)` calls, which safe Rust cannot make
// with the semantics this crate needs). Everything else in the crate is safe
// Rust under a crate-level `#![deny(unsafe_code)]`.

// Syscall boundary: this module is one of the crate's enumerated
// `#[allow(unsafe_code)]` modules (see `lib.rs`). It exists so that the C calls
// the crate needs — `poll(2)` (the builder's exit-event wait), `uname(2)` (the
// aarch64 kernel-version check) and the non-blocking `socket(2)`+`connect(2)`
// pair behind the vsock muxer's guest-initiated connects — do not force an
// `#[allow(unsafe_code)]` onto the modules that call them, which are otherwise
// entirely safe.
#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;

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

/// Sends `signal` to `pid` (`kill(2)`).
///
/// The per-VM process mode's stop control is the caller: it asks the child
/// `apz-vmm` to go with `SIGTERM` and, if the process is still there, with
/// `SIGKILL`. `std::process::Child::kill` cannot express that — it sends
/// `SIGKILL` and nothing else — and a signal that names a *process* rather
/// than the open `Child` handle is what a stop control needs, because the
/// handle itself is held by the waiter thread that is blocked in `wait(2)`.
///
/// A `pid` that no longer exists is `ESRCH`, which the caller treats as "the
/// VM is already gone".
pub fn send_signal(pid: i32, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: `kill(2)` takes two integers and touches no memory; `pid` is a
    // process id this crate owns (the child it spawned), never a pointer.
    if unsafe { libc::kill(pid, signal) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Gives the host memory `[host, host + len)` back to the kernel
/// (`madvise(2)` with `MADV_DONTNEED`). The pages drop and fault back in as
/// zeroes when next touched. This is how the balloon returns guest memory.
///
/// `madvise` never reads or writes through the pointer. A range that is not a
/// mapping of this process fails with an errno, which is returned here.
pub fn discard(host: *mut u8, len: usize) -> io::Result<()> {
    // SAFETY: `madvise` takes an address and a length and touches no memory
    // through them. The guest memory the balloon passes is an anonymous private
    // mapping of this process, and any other range is rejected with an errno.
    if unsafe { libc::madvise(host.cast(), len, libc::MADV_DONTNEED) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Pages `[host, host + len)` out of host RAM (`madvise(2)` with `MADV_PAGEOUT`): clean pages
/// backed by a file are dropped, anonymous ones go to swap where the host has it. The pages stay
/// mapped and fault back in on the next access, so the contents survive.
///
/// `madvise` never reads or writes through the pointer. A kernel older than 5.4 has no
/// `MADV_PAGEOUT` and fails with `EINVAL`, which is returned here for the caller to report.
pub fn page_out(host: *mut u8, len: usize) -> io::Result<()> {
    // SAFETY: `madvise` takes an address and a length and touches no memory through them. The
    // guest memory the reclaim passes is an anonymous or file-backed private mapping of this
    // process, and any other range is rejected with an errno.
    if unsafe { libc::madvise(host.cast(), len, libc::MADV_PAGEOUT) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Which pages of `[host, host + len)` are resident in host RAM (`mincore(2)`): one byte per page, its
/// low bit set for a resident page. The pages are not touched, so reading residency does not fault
/// any in. `host` must be page-aligned, as a mapping's start is.
pub fn resident_pages(host: *mut u8, len: usize) -> io::Result<Vec<u8>> {
    let page = crate::vstate::memory::host_page_size();
    let mut vec = vec![0u8; len.div_ceil(page)];
    // SAFETY: `mincore` reads the page state of the range and writes one byte per page of it into
    // `vec`, which holds exactly `len` rounded up to whole pages. It reads or writes no byte of the
    // range itself, and a range that is not a mapping of this process fails with an errno.
    if unsafe { libc::mincore(host.cast(), len, vec.as_mut_ptr()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(vec)
    }
}

/// Asks the kernel to start reading `[offset, offset + len)` of `file` into the page cache
/// (`posix_fadvise(2)` with `POSIX_FADV_WILLNEED`). The read is asynchronous: the call returns
/// without waiting for it. The advice is a hint, so a caller may ignore its failure.
#[cfg(target_os = "linux")]
pub fn willneed(file: &std::fs::File, offset: u64, len: u64) -> io::Result<()> {
    let offset = libc::off_t::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?;
    let len = libc::off_t::try_from(len).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: `posix_fadvise` takes a descriptor and a range and touches no memory through them. The
    // descriptor is borrowed from a live `File` for the call. It returns the error number itself
    // rather than setting `errno`.
    let err =
        unsafe { libc::posix_fadvise(file.as_raw_fd(), offset, len, libc::POSIX_FADV_WILLNEED) };
    if err == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(err))
    }
}

/// Makes `dst` share `src`'s extents (`ioctl(FICLONE)`): afterwards `dst` holds
/// `src`'s bytes at no copy cost, and the two diverge block by block as either is
/// written. The kernel refuses a filesystem that cannot share extents
/// (`EOPNOTSUPP`), a source on another filesystem (`EXDEV`) and a file it cannot
/// clone (`EINVAL`, `ENOTTY`, `EBADF`); the caller decides what those mean.
#[cfg(target_os = "linux")]
pub fn ficlone(dst: &std::fs::File, src: &std::fs::File) -> io::Result<()> {
    // SAFETY: `FICLONE` takes one integer argument, the source descriptor, and
    // touches no memory through it. Both descriptors are borrowed from live
    // `File`s for the duration of the call, and the return value is checked.
    if unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
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

/// Connect to the Unix domain socket at `path` **without blocking**.
///
/// The descriptor is put in non-blocking mode *before* `connect(2)`, so a
/// listener whose accept backlog is full fails the connect immediately with
/// `EAGAIN` (`ErrorKind::WouldBlock`) instead of parking the calling thread
/// until the listener accepts. `std`'s `UnixStream::connect` cannot express
/// that: it creates a blocking socket, connects, and only then restores
/// non-blocking mode — so with it, a full backlog blocks the caller.
///
/// That distinction is load-bearing for the vsock muxer, whose caller is the
/// VM's single vsock event thread: a blocked `connect(2)` there stops all
/// vsock RX/TX for the VM for as long as the host listener does not accept
/// (the muxer admits 1023 connections while the default listen backlog is
/// 128), and the thread cannot be woken by the ordinary exit path because it
/// is not inside `epoll_wait`. With this helper the muxer answers the guest
/// with an RST instead.
///
/// Path errors keep the semantics the callers expect: a NUL byte or a path too
/// long for `sockaddr_un` is `ErrorKind::InvalidInput`.
pub fn connect_unix_nonblocking(path: &str) -> io::Result<UnixStream> {
    /// Bytes of `sockaddr_un::sun_path`. A pathname must be shorter: one byte
    /// is reserved for its terminating NUL.
    const SUN_PATH_LEN: usize = std::mem::size_of::<libc::sockaddr_un>()
        - std::mem::offset_of!(libc::sockaddr_un, sun_path);

    let bytes = path.as_bytes();
    if bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path must not contain null bytes",
        ));
    }
    if bytes.len() >= SUN_PATH_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path must be shorter than SUN_LEN",
        ));
    }

    // SAFETY: constant, valid arguments; the returned descriptor is checked
    // below before anything else touches it.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a freshly created, valid descriptor that nothing else
    // owns; the `UnixStream` takes ownership and closes it on drop, including
    // on the error paths below.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };

    // Before `connect(2)`: a full accept backlog is then `EAGAIN` rather than a
    // block. Nothing else can observe the descriptor in between.
    stream.set_nonblocking(true)?;

    // `AF_UNIX` fits `sa_family_t` and the path length (checked above) plus its
    // NUL fits `socklen_t`; the conversions are still checked rather than
    // asserted, so a pathological path fails closed with an error.
    let family = libc::sa_family_t::try_from(libc::AF_UNIX).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "AF_UNIX does not fit sa_family_t",
        )
    })?;
    let mut addr = libc::sockaddr_un {
        sun_family: family,
        sun_path: [0; SUN_PATH_LEN],
    };
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = libc::c_char::from_ne_bytes([*src]);
    }
    let addr_len = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    let addr_len = libc::socklen_t::try_from(addr_len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path does not fit socklen_t"))?;

    // SAFETY: `stream` owns a valid AF_UNIX descriptor, and `addr` is a fully
    // initialised `sockaddr_un` (family, NUL-terminated path) that stays alive
    // for the duration of the call; `addr_len` describes it. The return value
    // is checked below.
    let ret = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            (&addr as *const libc::sockaddr_un).cast::<libc::sockaddr>(),
            addr_len,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::page_out;

    /// `page_out` advises an anonymous mapping of this process and leaves its contents readable:
    /// the pages stay mapped, and fault back in (from swap, or as they were) on the next read.
    #[test]
    fn page_out_advises_a_mapping_and_keeps_its_contents() {
        let len = 1 << 16;
        // SAFETY: an anonymous private mapping of `len` bytes, unmapped before the test returns.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED, "the test mapping");
        let host = ptr.cast::<u8>();
        // SAFETY: `host` addresses `len` writable bytes of the mapping above.
        unsafe { std::ptr::write_bytes(host, 0xAB, len) };

        let advised = page_out(host, len);
        // SAFETY: the mapping is still live, and its pages are readable after a page-out.
        let (first, last) = unsafe { (*host, *host.add(len - 1)) };
        // SAFETY: `ptr` and `len` are the mapping made above, unmapped exactly once.
        unsafe { libc::munmap(ptr, len) };

        advised.expect("MADV_PAGEOUT is accepted (Linux 5.4 or later)");
        assert_eq!((first, last), (0xAB, 0xAB));
    }
}
