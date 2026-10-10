//! AF_VSOCK: the host side of the plaza's only network path.
//!
//! The orchestrator listens on one CID and port (`VMADDR_CID_HOST:5000` by
//! contract) and accepts one connection per guest VM. This module is the whole
//! network surface of `vsockd`: it listens, accepts, and hands each connection
//! to a worker. Policy — which tools exist, what they may do — lives above it.

use std::io::{self, Read, Write};
use std::os::fd::RawFd;

/// The host's own CID (`VMADDR_CID_HOST`), the default listen CID.
pub const HOST_CID: u32 = libc::VMADDR_CID_HOST as u32;

/// `VMADDR_CID_ANY`: no CID. A transport that does not report the peer's
/// address on `accept` answers this, and then there is nothing to check a
/// guest's claimed CID against.
pub const CID_ANY: u32 = libc::VMADDR_CID_ANY as u32;

/// How many connections may be pending before `accept` starts refusing them.
const LISTEN_BACKLOG: libc::c_int = 128;

/// A connected AF_VSOCK stream to one guest.
#[derive(Debug)]
pub struct VsockStream {
    fd: RawFd,
}

impl Read for VsockStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            // SAFETY: `buf` is a valid writable slice and `self.fd` is a live
            // socket; `read` writes at most `buf.len()` bytes.
            let read = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if read < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(read as usize);
        }
    }
}

impl Write for VsockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            // SAFETY: `buf` is a valid readable slice and `self.fd` is a live
            // socket; `write` reads at most `buf.len()` bytes.
            let written = unsafe { libc::write(self.fd, buf.as_ptr().cast(), buf.len()) };
            if written < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(written as usize);
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for VsockStream {
    fn drop(&mut self) {
        // SAFETY: `self.fd` is the descriptor this type owns and closes
        // exactly once, here; nothing else may close it.
        unsafe { libc::close(self.fd) };
    }
}

/// The listening socket the orchestrator accepts guest sessions on.
#[derive(Debug)]
pub struct VsockListener {
    fd: RawFd,
}

impl VsockListener {
    /// Binds `cid`:`port` and starts listening.
    pub fn bind(cid: u32, port: u32) -> io::Result<Self> {
        // SAFETY: `socket` is called with valid constants and only creates a
        // new file descriptor; the result is checked below.
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let listener = Self { fd };

        // A restart of the daemon must not fail on the previous listener's
        // socket; a failure to set the option is not fatal, so it is ignored.
        let yes: libc::c_int = 1;
        // SAFETY: `yes` is a valid `c_int` and `fd` is a live socket; the
        // option is `SO_REUSEADDR`, whose value is an `int`.
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                std::ptr::from_ref(&yes).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };

        let addr = libc::sockaddr_vm {
            svm_family: libc::AF_VSOCK as libc::sa_family_t,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: cid,
            svm_zero: [0; 4],
        };
        // SAFETY: `addr` is a fully initialised `sockaddr_vm` and `fd` is a
        // live socket of the matching family; the length passed is exactly
        // `size_of::<sockaddr_vm>()`.
        let rc = unsafe {
            libc::bind(
                fd,
                std::ptr::from_ref(&addr).cast(),
                std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a bound socket.
        let rc = unsafe { libc::listen(fd, LISTEN_BACKLOG) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(listener)
    }

    /// Switches the listener between blocking and non-blocking `accept`.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        // SAFETY: `fd` is a live socket; `F_GETFL`/`F_SETFL` are called with
        // the descriptor it owns.
        let flags = unsafe { libc::fcntl(self.fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = if nonblocking {
            flags | libc::O_NONBLOCK
        } else {
            flags & !libc::O_NONBLOCK
        };
        // SAFETY: `fd` is a live socket and `flags` came from `F_GETFL`.
        let rc = unsafe { libc::fcntl(self.fd, libc::F_SETFL, flags) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Waits up to `timeout_ms` for a connection to be acceptable; returns
    /// `true` if one is (or an error is pending), `false` on timeout.
    ///
    /// The bounded wait is what lets the accept loop notice a shutdown request
    /// instead of sleeping in a blocking `accept`.
    pub fn wait_readable(&self, timeout_ms: libc::c_int) -> io::Result<bool> {
        let mut fds = [libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: `fds` is a one-element array of valid `pollfd` values.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(err);
        }
        Ok(rc > 0)
    }

    /// Accepts one connection, returning the stream and the peer CID the
    /// transport reported for it.
    pub fn accept(&self) -> io::Result<(VsockStream, u32)> {
        let mut addr: libc::sockaddr_vm = libc::sockaddr_vm {
            svm_family: 0,
            svm_reserved1: 0,
            svm_port: 0,
            svm_cid: 0,
            svm_zero: [0; 4],
        };
        let mut len = std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t;
        // SAFETY: `addr` is a valid `sockaddr_vm` output buffer and `len` its
        // size; `fd` is the live listening socket this type owns.
        let fd = unsafe {
            libc::accept4(
                self.fd,
                std::ptr::from_mut(&mut addr).cast(),
                &mut len,
                libc::SOCK_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((VsockStream { fd }, addr.svm_cid))
    }
}

impl Drop for VsockListener {
    fn drop(&mut self) {
        // SAFETY: `self.fd` is the descriptor this type owns and closes
        // exactly once, here; nothing else may close it.
        unsafe { libc::close(self.fd) };
    }
}
