//! The daemon's vsock stream to its node: the way a workstation VM reaches its hub when it has no NIC (rule 0010).
//!
//! `vsock://<cid>:<port>` names a stream to the host (CID 2) on a port the node relays (see `apz_station::vsock`).
//! The stream is an ordinary `Read + Write` for the hub's TLS and WebSocket, which run end to end on top of it.
//! Linux uses its kernel's AF_VSOCK. Windows uses the AF_VSOCK provider the virtio-win `viosock` driver installs, and
//! a Windows guest without that driver is told so. Other platforms have no AF_VSOCK, so a vsock address is refused.

#[cfg(target_os = "linux")]
pub use linux::VsockStream;

#[cfg(windows)]
pub use winsock::VsockStream;

#[cfg(not(any(target_os = "linux", windows)))]
pub use other::VsockStream;

/// The address family of vsock: `AF_VSOCK` on Linux, and 40 on Windows with the viosock provider.
#[cfg(any(target_os = "linux", windows, test))]
const AF_VSOCK: u16 = 40;

/// The `SOCKADDR_VM` both kernels read: a family, a reserved word, the port, the CID and four zero bytes, 16 bytes in
/// all. Linux's `sockaddr_vm` and viosock's `SOCKADDR_VM` have this layout, so one definition serves both, and the
/// const checks below fail the build if the layout ever stops matching.
#[cfg(any(target_os = "linux", windows, test))]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct SockaddrVm {
    svm_family: u16,
    svm_reserved1: u16,
    svm_port: u32,
    svm_cid: u32,
    svm_zero: [u8; 4],
}

#[cfg(any(target_os = "linux", windows, test))]
impl SockaddrVm {
    /// The size of the structure, in bytes.
    const SIZE: usize = 16;

    /// The address of `cid`:`port`.
    fn new(cid: u32, port: u32) -> Self {
        Self {
            svm_family: AF_VSOCK,
            svm_reserved1: 0,
            svm_port: port,
            svm_cid: cid,
            svm_zero: [0; 4],
        }
    }

    /// The structure's bytes as the kernel reads them: each field in native order, at its C offset.
    fn to_bytes(self) -> [u8; Self::SIZE] {
        let mut image = [0u8; Self::SIZE];
        image[0..2].copy_from_slice(&self.svm_family.to_ne_bytes());
        image[2..4].copy_from_slice(&self.svm_reserved1.to_ne_bytes());
        image[4..8].copy_from_slice(&self.svm_port.to_ne_bytes());
        image[8..12].copy_from_slice(&self.svm_cid.to_ne_bytes());
        image[12..16].copy_from_slice(&self.svm_zero);
        image
    }
}

#[cfg(any(target_os = "linux", windows, test))]
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<SockaddrVm>() == SockaddrVm::SIZE);
    assert!(offset_of!(SockaddrVm, svm_family) == 0);
    assert!(offset_of!(SockaddrVm, svm_reserved1) == 2);
    assert!(offset_of!(SockaddrVm, svm_port) == 4);
    assert!(offset_of!(SockaddrVm, svm_cid) == 8);
    assert!(offset_of!(SockaddrVm, svm_zero) == 12);
};

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux {
    use std::io::{self, Read, Write};
    use std::os::fd::RawFd;
    use std::time::Duration;

    use apz_http::ws::Duplex;

    use super::SockaddrVm;

    /// How long one connect may take, in milliseconds.
    const CONNECT_TIMEOUT_MS: i32 = 10_000;

    /// A connected AF_VSOCK stream.
    #[derive(Debug)]
    pub struct VsockStream {
        fd: RawFd,
    }

    impl VsockStream {
        /// Connects to `cid`:`port`, giving up after [`CONNECT_TIMEOUT_MS`].
        pub fn connect(cid: u32, port: u32) -> io::Result<Self> {
            // SAFETY: `socket` only creates a descriptor; the result is checked below.
            let fd =
                unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let stream = Self { fd };
            let addr = SockaddrVm::new(cid, port).to_bytes();
            stream.set_flag(libc::O_NONBLOCK, true)?;
            // SAFETY: `addr` is the 16-byte image of a `sockaddr_vm` (see `SockaddrVm`) and `fd` is a live AF_VSOCK
            // socket; the length is the image's length.
            let rc =
                unsafe { libc::connect(fd, addr.as_ptr().cast(), addr.len() as libc::socklen_t) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EINPROGRESS) {
                    return Err(err);
                }
                stream.wait_connected()?;
            }
            stream.set_flag(libc::O_NONBLOCK, false)?;
            Ok(stream)
        }

        /// Waits until the non-blocking connect completes, or the connect timeout passes.
        fn wait_connected(&self) -> io::Result<()> {
            let mut fds = [libc::pollfd {
                fd: self.fd,
                events: libc::POLLOUT,
                revents: 0,
            }];
            // SAFETY: `fds` is a one-element array of valid `pollfd` values.
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 1, CONNECT_TIMEOUT_MS) };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            if rc == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the vsock connection to the node did not complete in time",
                ));
            }
            let mut error: libc::c_int = 0;
            let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: `error` and `len` are valid out-parameters for `SO_ERROR` on the live socket `self.fd`.
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

        fn set_flag(&self, flag: libc::c_int, on: bool) -> io::Result<()> {
            // SAFETY: `self.fd` is a live descriptor this type owns.
            let flags = unsafe { libc::fcntl(self.fd, libc::F_GETFL) };
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }
            let flags = if on { flags | flag } else { flags & !flag };
            // SAFETY: `self.fd` is live and `flags` came from `F_GETFL`.
            if unsafe { libc::fcntl(self.fd, libc::F_SETFL, flags) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Read for VsockStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            loop {
                // SAFETY: `buf` is a valid writable slice of `buf.len()` bytes and `self.fd` is live.
                let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n >= 0 {
                    return Ok(n as usize);
                }
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
            }
        }
    }

    impl Write for VsockStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            loop {
                // SAFETY: `buf` is a valid readable slice and `self.fd` is live; `MSG_NOSIGNAL` keeps a closed peer
                // from raising SIGPIPE.
                let n = unsafe {
                    libc::send(self.fd, buf.as_ptr().cast(), buf.len(), libc::MSG_NOSIGNAL)
                };
                if n >= 0 {
                    return Ok(n as usize);
                }
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Duplex for VsockStream {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            let tv = timeout.map_or(
                libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                |d| libc::timeval {
                    tv_sec: d.as_secs().min(i64::MAX as u64) as libc::time_t,
                    tv_usec: libc::suseconds_t::from(d.subsec_micros()),
                },
            );
            // SAFETY: `tv` is a valid `timeval` for `SO_RCVTIMEO` on the live socket `self.fd`.
            let rc = unsafe {
                libc::setsockopt(
                    self.fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    std::ptr::from_ref(&tv).cast(),
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                )
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for VsockStream {
        fn drop(&mut self) {
            // SAFETY: this type owns `fd` and closes it once.
            unsafe { libc::close(self.fd) };
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod winsock {
    use std::io::{self, Read, Write};
    use std::mem::{size_of, zeroed};
    use std::sync::Once;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::time::Duration;

    use apz_http::ws::Duplex;
    use windows_sys::Win32::Networking::WinSock as ws;

    use super::{AF_VSOCK, SockaddrVm};

    /// How long one connect may take, in milliseconds.
    const CONNECT_TIMEOUT_MS: i32 = 10_000;

    /// What a guest without the virtio-win `viosock` driver is told, so the owner knows what to install.
    const NO_PROVIDER: &str =
        "this Windows guest has no vsock provider: install the virtio-win viosock driver";

    static WSA_INIT: Once = Once::new();
    static WSA_STATUS: AtomicI32 = AtomicI32::new(0);

    /// Starts WinSock 2.2 once per process and reports the outcome on every call.
    fn ensure_wsa() -> io::Result<()> {
        WSA_INIT.call_once(|| {
            // SAFETY: WSADATA is plain data, so all-zero is a valid starting value.
            let mut data: ws::WSADATA = unsafe { zeroed() };
            // SAFETY: `data` is a valid, writable WSADATA. WSAStartup returns the error code directly.
            let status = unsafe { ws::WSAStartup(0x0202, &mut data) };
            WSA_STATUS.store(status, Ordering::Relaxed);
        });
        match WSA_STATUS.load(Ordering::Relaxed) {
            0 => Ok(()),
            code => Err(io::Error::from_raw_os_error(code)),
        }
    }

    fn last_error() -> io::Error {
        // SAFETY: WSAGetLastError has no preconditions.
        io::Error::from_raw_os_error(unsafe { ws::WSAGetLastError() })
    }

    fn check(ret: i32) -> io::Result<()> {
        if ret == ws::SOCKET_ERROR {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    /// Creates the AF_VSOCK socket, not inherited by a child process. WinSock answers a family with no provider with
    /// one of the "not supported" codes, and those become the message that names the driver to install.
    fn new_socket() -> io::Result<ws::SOCKET> {
        ensure_wsa()?;
        // SAFETY: a null protocol-info pointer is allowed, and the flags are valid WinSock values.
        let raw = unsafe {
            ws::WSASocketW(
                i32::from(AF_VSOCK),
                ws::SOCK_STREAM,
                0,
                std::ptr::null(),
                0,
                ws::WSA_FLAG_NO_HANDLE_INHERIT,
            )
        };
        if raw != ws::INVALID_SOCKET {
            return Ok(raw);
        }
        let err = last_error();
        let missing = matches!(
            err.raw_os_error(),
            Some(code) if code == ws::WSAEAFNOSUPPORT
                || code == ws::WSAEPFNOSUPPORT
                || code == ws::WSAEPROTONOSUPPORT
                || code == ws::WSAESOCKTNOSUPPORT
        );
        if missing {
            return Err(io::Error::new(io::ErrorKind::Unsupported, NO_PROVIDER));
        }
        Err(err)
    }

    fn set_nonblocking(raw: ws::SOCKET, on: bool) -> io::Result<()> {
        let mut mode = u32::from(on);
        // SAFETY: `mode` is a valid u32 that lives for the duration of the call.
        check(unsafe { ws::ioctlsocket(raw, ws::FIONBIO, &mut mode) })
    }

    /// Sets `SO_RCVTIMEO` or `SO_SNDTIMEO` in milliseconds: `None` waits forever, as on Linux.
    fn set_timeout(raw: ws::SOCKET, option: i32, dur: Option<Duration>) -> io::Result<()> {
        let ms: u32 = match dur {
            None => 0,
            Some(d) if d.is_zero() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cannot set a 0 duration timeout",
                ));
            }
            Some(d) => d.as_millis().clamp(1, u32::MAX as u128) as u32,
        };
        // SAFETY: `optval` points at a u32 that lives for the call, and `optlen` is its size.
        check(unsafe {
            ws::setsockopt(
                raw,
                ws::SOL_SOCKET,
                option,
                (&ms as *const u32).cast(),
                size_of::<u32>() as i32,
            )
        })
    }

    /// A connected AF_VSOCK stream: a WinSock socket, closed on drop.
    #[derive(Debug)]
    pub struct VsockStream {
        raw: ws::SOCKET,
    }

    impl VsockStream {
        /// Connects to `cid`:`port`, giving up after [`CONNECT_TIMEOUT_MS`]. Without a viosock provider this fails
        /// with [`NO_PROVIDER`].
        pub fn connect(cid: u32, port: u32) -> io::Result<Self> {
            // From here `stream` owns the socket, so every early return closes it.
            let stream = Self { raw: new_socket()? };
            set_nonblocking(stream.raw, true)?;
            let addr = SockaddrVm::new(cid, port).to_bytes();
            // SAFETY: `addr` is the 16-byte image of a SOCKADDR_VM (see `SockaddrVm`) and `raw` is a live socket;
            // the length is the image's length.
            let rc = unsafe { ws::connect(stream.raw, addr.as_ptr().cast(), addr.len() as i32) };
            if rc == ws::SOCKET_ERROR {
                let err = last_error();
                if err.raw_os_error() != Some(ws::WSAEWOULDBLOCK) {
                    return Err(err);
                }
                stream.wait_connected()?;
            }
            set_nonblocking(stream.raw, false)?;
            Ok(stream)
        }

        /// Waits until the non-blocking connect completes, or the connect timeout passes.
        fn wait_connected(&self) -> io::Result<()> {
            let mut fds = ws::WSAPOLLFD {
                fd: self.raw,
                events: ws::POLLWRNORM,
                revents: 0,
            };
            // SAFETY: `fds` is one valid WSAPOLLFD, and the count is 1.
            let rc = unsafe { ws::WSAPoll(&mut fds, 1, CONNECT_TIMEOUT_MS) };
            if rc == ws::SOCKET_ERROR {
                return Err(last_error());
            }
            if rc == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the vsock connection to the node did not complete in time",
                ));
            }
            let mut error: i32 = 0;
            let mut len = size_of::<i32>() as i32;
            // SAFETY: `error` and `len` are valid out-parameters for `SO_ERROR` on the live socket `self.raw`.
            let rc = unsafe {
                ws::getsockopt(
                    self.raw,
                    ws::SOL_SOCKET,
                    ws::SO_ERROR,
                    (&mut error as *mut i32).cast(),
                    &mut len,
                )
            };
            check(rc)?;
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error));
            }
            Ok(())
        }

        /// Sets how long one write may block (`SO_SNDTIMEO`); `None` waits forever.
        pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            set_timeout(self.raw, ws::SO_SNDTIMEO, timeout)
        }
    }

    impl Read for VsockStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let len = buf.len().min(i32::MAX as usize) as i32;
            // SAFETY: `buf` is valid for `len` writable bytes, and `len` fits in an i32.
            let n = unsafe { ws::recv(self.raw, buf.as_mut_ptr(), len, 0) };
            if n == ws::SOCKET_ERROR {
                Err(last_error())
            } else {
                Ok(n as usize)
            }
        }
    }

    impl Write for VsockStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let len = buf.len().min(i32::MAX as usize) as i32;
            // SAFETY: `buf` is valid for `len` readable bytes, and `len` fits in an i32.
            let n = unsafe { ws::send(self.raw, buf.as_ptr(), len, 0) };
            if n == ws::SOCKET_ERROR {
                Err(last_error())
            } else {
                Ok(n as usize)
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Duplex for VsockStream {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            set_timeout(self.raw, ws::SO_RCVTIMEO, timeout)
        }
    }

    impl Drop for VsockStream {
        fn drop(&mut self) {
            // SAFETY: this type owns `raw` and closes it once.
            unsafe { ws::closesocket(self.raw) };
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod other {
    use std::io::{self, Read, Write};
    use std::time::Duration;

    use apz_http::ws::Duplex;

    /// No AF_VSOCK off Linux and Windows: every connect is refused.
    #[derive(Debug)]
    pub struct VsockStream(());

    impl VsockStream {
        pub fn connect(_cid: u32, _port: u32) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "vsock:// hubs need Linux (AF_VSOCK) or Windows (the viosock provider); this machine cannot reach its node that way",
            ))
        }
    }

    impl Read for VsockStream {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }

    impl Write for VsockStream {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Duplex for VsockStream {
        fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::vsock::{config_address, link_address, parse_vsock};

    #[test]
    fn the_sockaddr_vm_image_is_family_reserved_port_cid_then_zero() {
        assert_eq!(
            AF_VSOCK, 40,
            "the vsock family is 40 on Linux and on Windows"
        );
        let image = SockaddrVm::new(2, 5101).to_bytes();
        assert_eq!(image.len(), 16);
        assert_eq!(&image[0..2], &40u16.to_ne_bytes());
        assert_eq!(&image[2..4], &0u16.to_ne_bytes());
        assert_eq!(&image[4..8], &5101u32.to_ne_bytes());
        assert_eq!(&image[8..12], &2u32.to_ne_bytes());
        assert_eq!(&image[12..16], &[0u8; 4]);
    }

    #[test]
    fn the_node_addresses_parse_to_cid_and_port_and_others_do_not() {
        assert_eq!(parse_vsock("vsock://2:5101"), Some((2, 5101)));
        assert_eq!(parse_vsock(&config_address()), Some((2, 5101)));
        assert_eq!(parse_vsock(&link_address()), Some((2, 5100)));
        for other in [
            "https://hub:7443",
            "vsock://2",
            "vsock://:5101",
            "vsock://2:",
            "vsock://+2:5101",
            "vsock://4294967296:1",
        ] {
            assert_eq!(parse_vsock(other), None, "{other:?}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_shared_layout_is_the_kernels_sockaddr_vm() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<libc::sockaddr_vm>(), SockaddrVm::SIZE);
        assert_eq!(
            offset_of!(libc::sockaddr_vm, svm_family),
            offset_of!(SockaddrVm, svm_family)
        );
        assert_eq!(
            offset_of!(libc::sockaddr_vm, svm_reserved1),
            offset_of!(SockaddrVm, svm_reserved1)
        );
        assert_eq!(
            offset_of!(libc::sockaddr_vm, svm_port),
            offset_of!(SockaddrVm, svm_port)
        );
        assert_eq!(
            offset_of!(libc::sockaddr_vm, svm_cid),
            offset_of!(SockaddrVm, svm_cid)
        );
        assert_eq!(
            offset_of!(libc::sockaddr_vm, svm_zero),
            offset_of!(SockaddrVm, svm_zero)
        );
        assert_eq!(libc::AF_VSOCK, i32::from(AF_VSOCK));
    }
}
