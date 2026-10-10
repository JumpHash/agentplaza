//! AF_VSOCK: the guest's only link to the orchestrator.
//!
//! The plaza has no NIC — a guest reaches the orchestrator over vsock and
//! nothing else — so this module is the harness's entire network surface: it
//! can open one stream, to the host CID, and it can do nothing else.

use std::io::{self, Read, Write};
use std::os::fd::RawFd;

/// The orchestrator's CID (`VMADDR_CID_HOST`).
pub const HOST_CID: u32 = libc::VMADDR_CID_HOST as u32;

/// How long a read waits for the orchestrator before failing, in seconds.
///
/// Every wait on the peer is bounded: a half-dead connection must not park the
/// harness forever, because the guest has no other way to notice.
const READ_TIMEOUT_SECS: i64 = 30;

/// How long one connect attempt may take before it is abandoned, in
/// milliseconds. A device that has accepted the request but never completes it
/// is not going to fix itself, so this bounds the whole attempt (the caller's
/// retry loop exists for a transport that is not registered yet, which fails
/// immediately instead).
const CONNECT_TIMEOUT_MS: i32 = 5000;

/// A connected AF_VSOCK stream to the orchestrator.
#[derive(Debug)]
pub struct VsockStream {
    fd: RawFd,
}

impl VsockStream {
    /// Connects to `cid`:`port`, giving up after [`CONNECT_TIMEOUT_MS`], and
    /// sets [`READ_TIMEOUT_SECS`] as the socket's receive timeout so a read
    /// from a peer that has gone silent fails (with
    /// [`io::ErrorKind::WouldBlock`]) instead of blocking forever.
    pub fn connect(cid: u32, port: u32) -> io::Result<Self> {
        // SAFETY: `socket` is called with valid constants and only creates a
        // new file descriptor; the result is checked below.
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let stream = Self { fd };
        let addr = libc::sockaddr_vm {
            svm_family: libc::AF_VSOCK as libc::sa_family_t,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: cid,
            svm_zero: [0; 4],
        };

        // The connect runs non-blocking so its completion can be awaited with
        // a deadline rather than trusted to return.
        stream.set_flag(libc::O_NONBLOCK, true)?;
        // SAFETY: `addr` is a fully initialised `sockaddr_vm` and `fd` is a
        // live socket of the matching family; the length passed is exactly
        // `size_of::<sockaddr_vm>()`.
        let rc = unsafe {
            libc::connect(
                fd,
                std::ptr::from_ref(&addr).cast(),
                std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            let err = io::Error::last_os_error();
            // A non-blocking connect in progress is the expected outcome.
            if err.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(err);
            }
            stream.wait_connected(CONNECT_TIMEOUT_MS)?;
        }

        stream.set_flag(libc::O_NONBLOCK, false)?;
        stream.set_read_timeout()?;
        Ok(stream)
    }

    /// Waits until a non-blocking connect completes, or `timeout_ms` passes.
    fn wait_connected(&self, timeout_ms: i32) -> io::Result<()> {
        let mut fds = [libc::pollfd {
            fd: self.fd,
            events: libc::POLLOUT,
            revents: 0,
        }];
        // SAFETY: `fds` is a one-element array of valid `pollfd` values.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        if rc == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("the vsock connection did not complete within {timeout_ms} ms"),
            ));
        }
        // `poll` cannot report the connect's own result, only that it is done:
        // the error, if any, is on the socket.
        let mut error: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `error` and `len` are valid out-parameters for the `SO_ERROR`
        // integer option on the live socket `self.fd`.
        let rc = unsafe {
            libc::getsockopt(
                self.fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                std::ptr::from_mut(&mut error).cast(),
                &mut len,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        Ok(())
    }

    /// Adds or removes one file-status flag.
    fn set_flag(&self, flag: libc::c_int, on: bool) -> io::Result<()> {
        // SAFETY: `self.fd` is a live socket; `F_GETFL`/`F_SETFL` are called
        // with the descriptor this type owns.
        let flags = unsafe { libc::fcntl(self.fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = if on { flags | flag } else { flags & !flag };
        // SAFETY: `self.fd` is a live socket and `flags` came from `F_GETFL`.
        let rc = unsafe { libc::fcntl(self.fd, libc::F_SETFL, flags) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Sets the socket's receive timeout.
    fn set_read_timeout(&self) -> io::Result<()> {
        let timeout = libc::timeval {
            tv_sec: READ_TIMEOUT_SECS,
            tv_usec: 0,
        };
        // SAFETY: `timeout` is a valid `timeval` and `self.fd` is a live
        // socket; the option is `SO_RCVTIMEO`, whose value is a `timeval`.
        let rc = unsafe {
            libc::setsockopt(
                self.fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                std::ptr::from_ref(&timeout).cast(),
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
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
