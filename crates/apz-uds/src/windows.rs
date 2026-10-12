//! WinSock implementation of std's Unix-domain stream and listener API.
//!
//! Windows 10 1803 and later implement `AF_UNIX` in WinSock. This module wraps
//! raw sockets in RAII handles and exposes the same method set as std's Unix
//! types, so the crate root can re-export either implementation.

use std::io::{self, Read, Write};
use std::mem::{offset_of, size_of, zeroed};
use std::net::Shutdown;
use std::os::windows::io::{AsRawSocket, RawSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Networking::WinSock as ws;

use crate::encode_sun_path;

static WSA_INIT: Once = Once::new();
static WSA_STATUS: AtomicI32 = AtomicI32::new(0);

/// Starts WinSock 2.2 once per process and reports the outcome on every call.
fn ensure_wsa() -> io::Result<()> {
    WSA_INIT.call_once(|| {
        // SAFETY: WSADATA is plain data, so all-zero is a valid starting value.
        let mut data: ws::WSADATA = unsafe { zeroed() };
        // SAFETY: data is a valid, writable WSADATA. WSAStartup returns the error code directly.
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

/// Builds a `SOCKADDR_UN` for `path` and returns it with its length.
fn sockaddr_for(path: &Path) -> io::Result<(ws::SOCKADDR_UN, i32)> {
    let encoded = encode_sun_path(path)?;
    // SAFETY: SOCKADDR_UN holds only integers and byte arrays, so all-zero is valid.
    let mut addr: ws::SOCKADDR_UN = unsafe { zeroed() };
    addr.sun_family = ws::AF_UNIX;
    for (dst, &src) in addr.sun_path.iter_mut().zip(encoded.iter()) {
        *dst = src as i8;
    }
    Ok((addr, size_of::<ws::SOCKADDR_UN>() as i32))
}

fn sockaddr_ptr(addr: &ws::SOCKADDR_UN) -> *const ws::SOCKADDR {
    (addr as *const ws::SOCKADDR_UN).cast()
}

/// An owned WinSock socket, closed on drop.
#[derive(Debug)]
struct Socket(ws::SOCKET);

impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: this handle is owned by Socket and is closed exactly once, here.
        unsafe { ws::closesocket(self.0) };
    }
}

impl Socket {
    fn new() -> io::Result<Socket> {
        ensure_wsa()?;
        // SAFETY: a null protocol-info pointer is allowed, and the flags are valid WinSock values.
        let s = unsafe {
            ws::WSASocketW(
                ws::AF_UNIX as i32,
                ws::SOCK_STREAM,
                0,
                std::ptr::null(),
                0,
                ws::WSA_FLAG_OVERLAPPED | ws::WSA_FLAG_NO_HANDLE_INHERIT,
            )
        };
        if s == ws::INVALID_SOCKET {
            Err(last_error())
        } else {
            Ok(Socket(s))
        }
    }

    fn try_clone(&self) -> io::Result<Socket> {
        // SAFETY: WSAPROTOCOL_INFOW is plain data, so all-zero is a valid starting value.
        let mut info: ws::WSAPROTOCOL_INFOW = unsafe { zeroed() };
        // SAFETY: self.0 is a live socket, info is valid and writable, and the target process is this one.
        check(unsafe { ws::WSADuplicateSocketW(self.0, std::process::id(), &mut info) })?;
        // SAFETY: info was filled in by WSADuplicateSocketW. FROM_PROTOCOL_INFO takes af, type and protocol from it.
        let s = unsafe {
            ws::WSASocketW(
                ws::FROM_PROTOCOL_INFO,
                ws::FROM_PROTOCOL_INFO,
                ws::FROM_PROTOCOL_INFO,
                &info,
                0,
                ws::WSA_FLAG_OVERLAPPED | ws::WSA_FLAG_NO_HANDLE_INHERIT,
            )
        };
        if s == ws::INVALID_SOCKET {
            Err(last_error())
        } else {
            Ok(Socket(s))
        }
    }

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        let mut mode = u32::from(nonblocking);
        // SAFETY: mode is a valid u32 that lives for the duration of the call.
        check(unsafe { ws::ioctlsocket(self.0, ws::FIONBIO, &mut mode) })
    }

    fn set_timeout(&self, option: i32, dur: Option<Duration>) -> io::Result<()> {
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
        // SAFETY: optval points at a u32 that lives for the call, and optlen is its size.
        check(unsafe {
            ws::setsockopt(
                self.0,
                ws::SOL_SOCKET,
                option,
                (&ms as *const u32).cast(),
                size_of::<u32>() as i32,
            )
        })
    }

    fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Read => ws::SD_RECEIVE,
            Shutdown::Write => ws::SD_SEND,
            Shutdown::Both => ws::SD_BOTH,
        };
        // SAFETY: self.0 is a live socket and how is a valid shutdown mode.
        check(unsafe { ws::shutdown(self.0, how) })
    }

    fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: buf is valid for writes of len bytes, and len fits in an i32.
        let n = unsafe { ws::recv(self.0, buf.as_mut_ptr(), len, 0) };
        if n == ws::SOCKET_ERROR {
            Err(last_error())
        } else {
            Ok(n as usize)
        }
    }

    fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: buf is valid for reads of len bytes, and len fits in an i32.
        let n = unsafe { ws::send(self.0, buf.as_ptr(), len, 0) };
        if n == ws::SOCKET_ERROR {
            Err(last_error())
        } else {
            Ok(n as usize)
        }
    }

    fn socket_name(&self, peer: bool) -> io::Result<SocketAddr> {
        // SAFETY: SOCKADDR_UN is plain data, so all-zero is valid.
        let mut addr: ws::SOCKADDR_UN = unsafe { zeroed() };
        let mut len = size_of::<ws::SOCKADDR_UN>() as i32;
        let name: *mut ws::SOCKADDR = (&mut addr as *mut ws::SOCKADDR_UN).cast();
        // SAFETY: name is writable for len bytes, and len holds that size.
        let ret = unsafe {
            if peer {
                ws::getpeername(self.0, name, &mut len)
            } else {
                ws::getsockname(self.0, name, &mut len)
            }
        };
        check(ret)?;
        Ok(SocketAddr::from_raw(&addr, len))
    }
}

/// Address of a Unix-domain socket. Unnamed sockets have no path.
#[derive(Clone, Debug)]
pub struct SocketAddr {
    path: Option<PathBuf>,
}

impl SocketAddr {
    /// Returns the socket path, or `None` for an unnamed socket.
    pub fn as_pathname(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Returns `true` for an unnamed socket.
    pub fn is_unnamed(&self) -> bool {
        self.path.is_none()
    }

    /// Reads the path out of a `SOCKADDR_UN` filled in by WinSock.
    fn from_raw(addr: &ws::SOCKADDR_UN, len: i32) -> SocketAddr {
        let offset = offset_of!(ws::SOCKADDR_UN, sun_path);
        let avail = (len.max(0) as usize)
            .saturating_sub(offset)
            .min(addr.sun_path.len());
        // SAFETY: sun_path is an array of addr.sun_path.len() bytes, and avail does not exceed that length.
        let raw = unsafe { std::slice::from_raw_parts(addr.sun_path.as_ptr().cast::<u8>(), avail) };
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let path = if end == 0 {
            None
        } else {
            std::str::from_utf8(&raw[..end]).ok().map(PathBuf::from)
        };
        SocketAddr { path }
    }
}

/// A connected Unix-domain stream socket.
#[derive(Debug)]
pub struct UnixStream(Socket);

impl UnixStream {
    /// Connects to the listener bound at `path`.
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        let socket = Socket::new()?;
        let (addr, len) = sockaddr_for(path.as_ref())?;
        // SAFETY: addr is a fully initialized SOCKADDR_UN and len is its size.
        check(unsafe { ws::connect(socket.0, sockaddr_ptr(&addr), len) })?;
        Ok(UnixStream(socket))
    }

    /// Connects, then switches the stream to non-blocking mode.
    ///
    /// AF_UNIX connects complete immediately on Windows, so no pending
    /// connect state has to be tracked.
    pub fn connect_nonblocking<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(stream)
    }

    /// Creates a connected pair of streams, as `std`'s `UnixStream::pair` does.
    ///
    /// WinSock has no socketpair, so this binds a temporary listener, connects
    /// to it, accepts, and removes the socket file.
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "apz-uds-pair-{}-{}-{}.sock",
            std::process::id(),
            n,
            nanos
        ));
        let result = (|| -> io::Result<(UnixStream, UnixStream)> {
            let listener = UnixListener::bind(&path)?;
            let client = UnixStream::connect(&path)?;
            let (server, _) = listener.accept()?;
            Ok((client, server))
        })();
        let _ = std::fs::remove_file(&path);
        result
    }

    /// Duplicates the handle; both handles refer to the same connection.
    pub fn try_clone(&self) -> io::Result<UnixStream> {
        Ok(UnixStream(self.0.try_clone()?))
    }

    /// Enables or disables non-blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.set_nonblocking(nonblocking)
    }

    /// Sets the read timeout. `None` blocks indefinitely.
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.0.set_timeout(ws::SO_RCVTIMEO, dur)
    }

    /// Sets the write timeout. `None` blocks indefinitely.
    pub fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.0.set_timeout(ws::SO_SNDTIMEO, dur)
    }

    /// Shuts down the read half, the write half, or both.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.0.shutdown(how)
    }

    /// Returns the address this socket is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.socket_name(false)
    }

    /// Returns the address of the connected peer.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.0.socket_name(true)
    }
}

impl Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.recv(buf)
    }
}

impl Read for &UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.recv(buf)
    }
}

impl Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.send(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for &UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.send(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AsRawSocket for UnixStream {
    fn as_raw_socket(&self) -> RawSocket {
        self.0 .0 as RawSocket
    }
}

/// A Unix-domain socket listening for connections.
#[derive(Debug)]
pub struct UnixListener(Socket, PathBuf);

impl UnixListener {
    /// Binds to `path` and starts listening. The socket file is left in place
    /// when the listener is dropped, as with std's `UnixListener`.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
        let path = path.as_ref();
        let socket = Socket::new()?;
        let (addr, len) = sockaddr_for(path)?;
        // SAFETY: addr is a fully initialized SOCKADDR_UN and len is its size.
        check(unsafe { ws::bind(socket.0, sockaddr_ptr(&addr), len) })?;
        // SAFETY: socket is bound and not yet listening.
        check(unsafe { ws::listen(socket.0, 128) })?;
        Ok(UnixListener(socket, path.to_path_buf()))
    }

    /// Accepts the next connection, blocking unless non-blocking mode is set.
    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        // SAFETY: SOCKADDR_UN is plain data, so all-zero is valid.
        let mut addr: ws::SOCKADDR_UN = unsafe { zeroed() };
        let mut len = size_of::<ws::SOCKADDR_UN>() as i32;
        let out: *mut ws::SOCKADDR = (&mut addr as *mut ws::SOCKADDR_UN).cast();
        // SAFETY: out is writable for len bytes, and len holds that size.
        let s = unsafe { ws::accept(self.0 .0, out, &mut len) };
        if s == ws::INVALID_SOCKET {
            return Err(last_error());
        }
        Ok((UnixStream(Socket(s)), SocketAddr::from_raw(&addr, len)))
    }

    /// Iterates over incoming connections, blocking on each `next` call.
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }

    /// Enables or disables non-blocking mode for `accept`.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.set_nonblocking(nonblocking)
    }

    /// Duplicates the listening handle.
    pub fn try_clone(&self) -> io::Result<UnixListener> {
        Ok(UnixListener(self.0.try_clone()?, self.1.clone()))
    }

    /// Returns the path this listener is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr {
            path: Some(self.1.clone()),
        })
    }
}

impl AsRawSocket for UnixListener {
    fn as_raw_socket(&self) -> RawSocket {
        self.0 .0 as RawSocket
    }
}

/// Iterator over incoming connections, returned by [`UnixListener::incoming`].
#[derive(Debug)]
pub struct Incoming<'a> {
    listener: &'a UnixListener,
}

impl Iterator for Incoming<'_> {
    type Item = io::Result<UnixStream>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.listener.accept().map(|(stream, _)| stream))
    }
}
