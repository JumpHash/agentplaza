//! The daemon's vsock stream to its node: the way a workstation VM reaches its hub when it has no NIC (rule 0010).
//!
//! `vsock://<cid>:<port>` names a stream to the host (CID 2) on a port the node relays (see `apz_station::vsock`).
//! The stream is an ordinary `Read + Write` for the hub's TLS and WebSocket, which run end to end on top of it.
//! Off Linux there is no AF_VSOCK, so a vsock address is refused with that reason.

#[cfg(target_os = "linux")]
pub use linux::VsockStream;

#[cfg(not(target_os = "linux"))]
pub use other::VsockStream;

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux {
    use std::io::{self, Read, Write};
    use std::os::fd::RawFd;
    use std::time::Duration;

    use apz_http::ws::Duplex;

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
            let addr = libc::sockaddr_vm {
                svm_family: libc::AF_VSOCK as libc::sa_family_t,
                svm_reserved1: 0,
                svm_port: port,
                svm_cid: cid,
                svm_zero: [0; 4],
            };
            stream.set_flag(libc::O_NONBLOCK, true)?;
            // SAFETY: `addr` is a fully initialised `sockaddr_vm` and `fd` is a live AF_VSOCK socket; the length is
            // exactly its size.
            let rc = unsafe {
                libc::connect(
                    fd,
                    std::ptr::from_ref(&addr).cast(),
                    std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
                )
            };
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

#[cfg(not(target_os = "linux"))]
mod other {
    use std::io::{self, Read, Write};
    use std::time::Duration;

    use apz_http::ws::Duplex;

    /// No AF_VSOCK off Linux: every connect is refused.
    #[derive(Debug)]
    pub struct VsockStream(());

    impl VsockStream {
        pub fn connect(_cid: u32, _port: u32) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "vsock:// hubs need Linux (AF_VSOCK); this machine cannot reach its node that way",
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
