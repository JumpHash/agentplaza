//! AF_VSOCK: the guest's only link to the orchestrator.
//!
//! The plaza has no NIC — a guest reaches the orchestrator over vsock and
//! nothing else — so this module is the harness's entire network surface: it
//! can open one stream, to the host CID, and it can do nothing else.

// Syscall boundary (the crate's one `unsafe` module): AF_VSOCK is not in `std`.
#![allow(unsafe_code)]

use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::time::Duration;

use apz_proto::line::ReadTimeout;

/// The orchestrator's CID (`VMADDR_CID_HOST`).
pub const HOST_CID: u32 = libc::VMADDR_CID_HOST;

/// `VMADDR_CID_ANY`: what the vsock stack reports when a transport has no CID
/// to report (a guest whose device never published one reads back this).
pub const CID_ANY: u32 = libc::VMADDR_CID_ANY;

/// `IOCTL_VM_SOCKETS_GET_LOCAL_CID = _IO(7, 0xb9)` (`include/uapi/linux/vm_sockets.h`):
/// asks the vsock stack for the local CID of the socket's transport — on a
/// guest, the CID its virtio-vsock device published.
const IOCTL_VM_SOCKETS_GET_LOCAL_CID: libc::c_ulong = ioctl_expr(7, 0xb9);

/// Builds a `_IO(type, nr)` ioctl request, as `asm-generic/ioctl.h` does:
/// direction none, `type` in bits 8-15, `nr` in bits 0-7, no argument size.
const fn ioctl_expr(kind: u32, nr: u32) -> libc::c_ulong {
    ((kind << 8) | nr) as libc::c_ulong
}

/// How long a read waits for the orchestrator unless a session sets its own
/// bound: a half-dead connection must not park the harness forever.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// sets [`READ_TIMEOUT`] as the socket's receive timeout so a read
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
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        Ok(stream)
    }

    /// Connects to the Unix socket at `path`: the test transport a `plaza.connect=uds:` command line names. vsockd
    /// serves each guest CID on such a socket, so the stream reads and writes as the vsock one does and takes the
    /// same read timeout. Nothing else in the harness dials a socket.
    pub fn connect_unix(path: &str) -> io::Result<Self> {
        let bytes = path.as_bytes();
        // SAFETY: `sockaddr_un` is plain data, so all zeroes is a valid value; the fields set below are the ones
        // `connect` reads.
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if bytes.len() >= addr.sun_path.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the socket path {path} is too long"),
            ));
        }
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (slot, byte) in addr.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        // SAFETY: `socket` is called with valid constants and only creates a new file descriptor; the result is
        // checked below.
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let stream = Self { fd };
        // SAFETY: `addr` is a fully initialised `sockaddr_un` and `fd` a live socket of the matching family; the
        // length passed is exactly `size_of::<sockaddr_un>()`.
        let rc = unsafe {
            libc::connect(
                fd,
                std::ptr::from_ref(&addr).cast(),
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        Ok(stream)
    }

    /// Another handle to this connection (`dup`), so one thread can read while
    /// another writes. The connection closes once every handle has.
    pub fn try_clone(&self) -> io::Result<Self> {
        // SAFETY: `F_DUPFD_CLOEXEC` on the live descriptor this type owns returns
        // a new descriptor, which the returned handle owns; the result is checked.
        let fd = unsafe { libc::fcntl(self.fd, libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd })
    }

    /// Ends the connection in both directions, for every handle to it, so a read
    /// blocked on any of them returns.
    pub fn shutdown(&self) {
        // SAFETY: `self.fd` is a live socket this type owns; `shutdown` only
        // signals the connection, and its result is not needed.
        unsafe { libc::shutdown(self.fd, libc::SHUT_RDWR) };
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
}

/// The guest CID this VM was assigned, as the vsock stack itself reports it.
///
/// The CID is not, and cannot be, on the kernel command line: one command line
/// is shared by every VM of a spawn, while the VMM gives each VM its own CID
/// through the virtio-vsock device's configuration space — the guest's driver
/// reads that CID back and stamps it into every packet it sends, and the host
/// drops packets whose source CID it does not expect.
///
/// `IOCTL_VM_SOCKETS_GET_LOCAL_CID` is the vsock device's own way of saying
/// which CID the registered transport has (the ioctl lives on `/dev/vsock`,
/// which the loaded vsock module creates), so `hello` can name the CID the
/// packets actually carry instead of a value from a shared command line that
/// would disagree with them on every VM but one.
pub fn device_guest_cid() -> io::Result<u32> {
    // SAFETY: the path is a valid NUL-terminated C string and `open` only
    // creates a new file descriptor; the result is checked below.
    let fd = unsafe { libc::open(c"/dev/vsock".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut cid: libc::c_uint = 0;
    // SAFETY: `cid` is a valid `u32` for the ioctl to write through and `fd` is
    // the open `/dev/vsock` this function owns; the request takes no other
    // argument.
    let rc = unsafe { libc::ioctl(fd, IOCTL_VM_SOCKETS_GET_LOCAL_CID, &mut cid) };
    // Read errno before `close`, which may overwrite it.
    let err = io::Error::last_os_error();
    // SAFETY: `fd` is the descriptor opened above and is closed exactly once,
    // here.
    unsafe { libc::close(fd) };
    if rc < 0 {
        return Err(err);
    }
    Ok(cid)
}

impl ReadTimeout for VsockStream {
    /// `SO_RCVTIMEO`: a read that waits longer fails with `WouldBlock`.
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        if timeout == Some(Duration::ZERO) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a zero read timeout",
            ));
        }
        let timeout = timeout.unwrap_or(Duration::ZERO);
        let value = libc::timeval {
            tv_sec: libc::time_t::try_from(timeout.as_secs()).unwrap_or(libc::time_t::MAX),
            tv_usec: timeout.subsec_micros() as libc::suseconds_t,
        };
        // SAFETY: `value` is a valid `timeval` and `self.fd` a live socket; the
        // option is `SO_RCVTIMEO`, whose value is a `timeval`.
        let rc = unsafe {
            libc::setsockopt(
                self.fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                std::ptr::from_ref(&value).cast(),
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

/// How many connections may wait for `accept` before the kernel refuses more.
const LISTEN_BACKLOG: libc::c_int = 64;

/// A listening AF_VSOCK socket: the sandbox daemon's accept loop (see [`crate::envd`]).
///
/// The peer is the host's gateway, the only thing the device routes to this guest, so the
/// listener does not check which CID a connection came from.
#[derive(Debug)]
pub struct VsockListener {
    fd: RawFd,
}

impl VsockListener {
    /// Listens on `port` for connections from any CID.
    pub fn bind(port: u32) -> io::Result<Self> {
        // SAFETY: `socket` is called with valid constants and only creates a new file
        // descriptor; the result is checked, and owned by `listener` from here on, so an
        // early return closes it.
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let listener = Self { fd };
        let addr = libc::sockaddr_vm {
            svm_family: libc::AF_VSOCK as libc::sa_family_t,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: CID_ANY,
            svm_zero: [0; 4],
        };
        // SAFETY: `addr` is a fully initialised `sockaddr_vm`, `fd` a live socket of the
        // matching family, and the length is exactly `size_of::<sockaddr_vm>()`.
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
        // SAFETY: `fd` is a live socket that is bound and not yet listening.
        let rc = unsafe { libc::listen(fd, LISTEN_BACKLOG) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(listener)
    }

    /// Waits for the next connection. The returned stream has no read timeout: an idle
    /// keepalive client is not an error.
    pub fn accept(&self) -> io::Result<VsockStream> {
        loop {
            // SAFETY: `fd` is the live listening socket this type owns. Null address pointers
            // are allowed by `accept4` when the peer's address is not wanted. The new
            // descriptor is owned by the returned stream, which closes it.
            let fd = unsafe {
                libc::accept4(
                    self.fd,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC,
                )
            };
            if fd < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(VsockStream { fd });
        }
    }
}

impl Drop for VsockListener {
    fn drop(&mut self) {
        // SAFETY: `self.fd` is the listening descriptor this type owns and closes exactly once,
        // here.
        unsafe { libc::close(self.fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request number is an ABI: `IOCTL_VM_SOCKETS_GET_LOCAL_CID` is
    /// `_IO(7, 0xb9)` in `include/uapi/linux/vm_sockets.h`, and a wrong number
    /// would be answered by some other handler (or by none) rather than fail
    /// loudly, so it is pinned here.
    #[test]
    fn the_local_cid_ioctl_request_is_the_headers() {
        assert_eq!(IOCTL_VM_SOCKETS_GET_LOCAL_CID, 0x7b9);
        assert_eq!(CID_ANY, 0xffff_ffff);
        assert_eq!(HOST_CID, 2);
    }

    /// The test transport: a `plaza.connect=uds:` stream reaches the socket it names and carries bytes both ways,
    /// and a socket that is not there is refused.
    #[test]
    fn connect_unix_reaches_the_socket_a_test_names_and_carries_bytes_both_ways() {
        use std::io::{Read, Write};
        let path = std::env::temp_dir().join(format!("apz-connect-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let mut stream = VsockStream::connect_unix(path.to_str().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        stream.write_all(b"ping\n").unwrap();
        let mut got = [0u8; 5];
        peer.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"ping\n");
        peer.write_all(b"pong\n").unwrap();
        let mut back = [0u8; 5];
        stream.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"pong\n");
        let _ = std::fs::remove_file(&path);
        assert!(VsockStream::connect_unix("/nonexistent/apz/vm1_5000").is_err());
    }
}
