// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the host end of a guest's vsock on Linux, for a VM that the kernel's vhost-vsock device carries
// (the QEMU workstation backend, docs/design/workstations.md section 10). A guest's `connect(2)` to the host's CID
// arrives on an `AF_VSOCK` listener here, which names the guest by the CID it connected from. The plaza VMM's own muxer
// is unchanged: it turns a plaza guest's connections into unix sockets, and this module serves none of them.

// Syscall boundary: this module is one of the crate's enumerated `#[allow(unsafe_code)]` modules (see `lib.rs`). `AF_VSOCK`
// is not in `std`, so `socket(2)`, `bind(2)`, `listen(2)`, `accept4(2)`, `setsockopt(2)`, `shutdown(2)`, `fcntl(2)` and the
// `read(2)`/`write(2)` of a connected socket are made here, each with its own SAFETY note.
#![allow(unsafe_code)]

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

/// The CID that a guest uses for the host's end of its vsock, and that a host listener's peers are numbered from.
/// `VMADDR_CID_ANY` is any local CID: a listener bound to it takes a guest's connection to the host on its port.
const VMADDR_CID_ANY: u32 = u32::MAX;
/// How many accepted connections may wait for the relay to take them.
const BACKLOG: libc::c_int = 16;

/// A listening `AF_VSOCK` socket on the host, nonblocking, so [`Listener::accept`] never waits.
#[derive(Debug)]
pub struct Listener {
    fd: OwnedFd,
}

impl Listener {
    /// Listens on `port` for connections from any guest.
    pub fn bind(port: u32) -> io::Result<Self> {
        let fd = new_socket()?;
        let addr = sockaddr(VMADDR_CID_ANY, port);
        // SAFETY: `addr` is a fully initialised `sockaddr_vm`, `fd` is a live AF_VSOCK socket, and the length is `addr`'s size.
        let rc = unsafe {
            libc::bind(
                fd.as_raw_fd(),
                std::ptr::from_ref(&addr).cast(),
                size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a live socket, bound above; `listen(2)` takes an integer backlog and touches no memory.
        if unsafe { libc::listen(fd.as_raw_fd(), BACKLOG) } < 0 {
            return Err(io::Error::last_os_error());
        }
        set_nonblocking(fd.as_raw_fd(), true)?;
        Ok(Self { fd })
    }

    /// The next waiting connection, with the CID of the guest that made it, or `None` when none is waiting.
    pub fn accept(&self) -> io::Result<Option<(Stream, u32)>> {
        let mut addr = sockaddr(0, 0);
        let mut len = size_of::<libc::sockaddr_vm>() as libc::socklen_t;
        // SAFETY: `addr` and `len` are valid out-parameters, and `len` is `addr`'s size, which `accept4(2)` writes back.
        let fd = unsafe {
            libc::accept4(
                self.fd.as_raw_fd(),
                std::ptr::from_mut(&mut addr).cast(),
                &mut len,
                libc::SOCK_CLOEXEC,
            )
        };
        if fd < 0 {
            let err = io::Error::last_os_error();
            return if err.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(err)
            };
        }
        // SAFETY: `accept4` returned a new descriptor, which nothing else owns.
        let stream = Stream {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        };
        Ok(Some((stream, addr.svm_cid)))
    }
}

/// A connected `AF_VSOCK` stream to a guest. It blocks, unless a read or write timeout is set.
#[derive(Debug)]
pub struct Stream {
    fd: OwnedFd,
}

impl Stream {
    /// Another handle on the same connection, for one direction of a relay.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            fd: self.fd.try_clone()?,
        })
    }

    /// Sets how long one read may wait (`None` waits without limit), so a relay can see its stop flag.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        set_timeout(self.fd.as_raw_fd(), libc::SO_RCVTIMEO, timeout)
    }

    /// Sets how long one write may wait (`None` waits without limit).
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        set_timeout(self.fd.as_raw_fd(), libc::SO_SNDTIMEO, timeout)
    }

    /// Ends one or both directions of the connection.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Read => libc::SHUT_RD,
            Shutdown::Write => libc::SHUT_WR,
            Shutdown::Both => libc::SHUT_RDWR,
        };
        // SAFETY: `shutdown(2)` takes a live socket and an integer, and touches no memory.
        if unsafe { libc::shutdown(self.fd.as_raw_fd(), how) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `buf` is valid for `buf.len()` bytes of writes, and `fd` is a live descriptor.
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: `buf` is valid for `buf.len()` bytes of reads, and `fd` is a live descriptor.
        let n = unsafe { libc::write(self.fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn sockaddr(cid: u32, port: u32) -> libc::sockaddr_vm {
    libc::sockaddr_vm {
        svm_family: libc::AF_VSOCK as libc::sa_family_t,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: cid,
        svm_zero: [0; 4],
    }
}

fn new_socket() -> io::Result<OwnedFd> {
    // SAFETY: `socket(2)` creates a descriptor and touches no memory; the result is checked before it is owned.
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn set_nonblocking(fd: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: `F_GETFL` takes no pointer; `fd` is a live descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = if on {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    // SAFETY: `F_SETFL` takes an integer flag word; `fd` is a live descriptor.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn set_timeout(fd: RawFd, option: libc::c_int, timeout: Option<Duration>) -> io::Result<()> {
    // A zero timeval is no timeout on Linux, which is what `None` asks for.
    let value = match timeout {
        Some(limit) => libc::timeval {
            tv_sec: libc::time_t::try_from(limit.as_secs()).unwrap_or(libc::time_t::MAX),
            // Under a second, so it fits any `suseconds_t`.
            tv_usec: limit.subsec_micros() as libc::suseconds_t,
        },
        None => libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
    };
    // SAFETY: `value` is a fully initialised `timeval`, passed with its size; `fd` is a live socket.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            std::ptr::from_ref(&value).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
