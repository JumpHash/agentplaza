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
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vm_memory::bitmap::BitmapSlice;
use vm_memory::{ReadVolatile, VolatileMemoryError, VolatileSlice, WriteVolatile};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Networking::WinSock as ws;
use windows_sys::Win32::System::IO::CancelIoEx;

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

/// How often a read that waits for data with a timeout looks at `read_shut`: a `shutdown(Read)` from another thread
/// ends it within this.
const READ_SHUT_POLL: Duration = Duration::from_millis(50);

/// An owned WinSock socket, closed on drop.
#[derive(Debug)]
struct Socket {
    raw: ws::SOCKET,
    /// Set by `shutdown(Read | Both)`: a read cancelled by it reports EOF.
    read_shut: AtomicBool,
    /// The read timeout in milliseconds, 0 for none. It is kept here, not set as `SO_RCVTIMEO`: WinSock leaves a socket
    /// whose receive timed out "in an indeterminate state", and on AF_UNIX a peer's half-close that arrives as a timed-out
    /// receive is cancelled can be lost, so a reader polling with short timeouts never sees the EOF. A read with a timeout
    /// waits in `WSAPoll` instead and calls `recv` only once the socket is readable (data, EOF or an error).
    read_timeout_ms: AtomicU32,
    /// Whether the socket is in non-blocking mode, where a read never waits.
    nonblocking: AtomicBool,
}

impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: this handle is owned by Socket and is closed exactly once, here.
        unsafe { ws::closesocket(self.raw) };
    }
}

impl Socket {
    fn from_raw(raw: ws::SOCKET) -> Socket {
        Socket {
            raw,
            read_shut: AtomicBool::new(false),
            read_timeout_ms: AtomicU32::new(0),
            nonblocking: AtomicBool::new(false),
        }
    }

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
            Ok(Socket::from_raw(s))
        }
    }

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        let mut mode = u32::from(nonblocking);
        // SAFETY: mode is a valid u32 that lives for the duration of the call.
        check(unsafe { ws::ioctlsocket(self.raw, ws::FIONBIO, &mut mode) })?;
        self.nonblocking.store(nonblocking, Ordering::SeqCst);
        Ok(())
    }

    /// A timeout in milliseconds (0 for none), refusing a zero duration as std does.
    fn timeout_ms(dur: Option<Duration>) -> io::Result<u32> {
        match dur {
            None => Ok(0),
            Some(d) if d.is_zero() => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot set a 0 duration timeout",
            )),
            Some(d) => Ok(d.as_millis().clamp(1, u32::MAX as u128) as u32),
        }
    }

    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.read_timeout_ms
            .store(Self::timeout_ms(dur)?, Ordering::SeqCst);
        Ok(())
    }

    fn set_timeout(&self, option: i32, dur: Option<Duration>) -> io::Result<()> {
        let ms = Self::timeout_ms(dur)?;
        // SAFETY: optval points at a u32 that lives for the call, and optlen is its size.
        check(unsafe {
            ws::setsockopt(
                self.raw,
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
        // SAFETY: self.raw is a live socket and how is a valid shutdown mode.
        check(unsafe { ws::shutdown(self.raw, how) })?;
        // A Unix shutdown(SHUT_RD) makes a read blocked on the socket (through
        // any dup'ed descriptor) return EOF; a WinSock shutdown does not wake a
        // recv already waiting in another thread. Cancelling the socket's
        // pending I/O does, and `recv` reports the cancelled read as EOF.
        if how != ws::SD_SEND {
            self.read_shut.store(true, Ordering::SeqCst);
            // SAFETY: a SOCKET is a kernel handle; CancelIoEx with a null
            // OVERLAPPED cancels every pending I/O request on it, from any
            // thread. Failure (nothing pending) is not an error here.
            unsafe { CancelIoEx(self.raw as HANDLE, std::ptr::null()) };
        }
        Ok(())
    }

    /// Waits until a read will not block (data, EOF or an error is waiting), for the read timeout, looking at
    /// `read_shut` every [`READ_SHUT_POLL`]. `Ok(false)` when the socket was shut for reading meanwhile.
    fn wait_readable(&self, timeout_ms: u32) -> io::Result<bool> {
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        loop {
            if self.read_shut.load(Ordering::SeqCst) {
                return Ok(false);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::from_raw_os_error(ws::WSAETIMEDOUT));
            }
            let slice = left.min(READ_SHUT_POLL).as_millis().max(1) as i32;
            let mut fd = ws::WSAPOLLFD {
                fd: self.raw,
                events: ws::POLLRDNORM,
                revents: 0,
            };
            // SAFETY: fd is one valid, writable WSAPOLLFD for the duration of the call.
            let ready = unsafe { ws::WSAPoll(&mut fd, 1, slice) };
            if ready == ws::SOCKET_ERROR {
                return Err(last_error());
            }
            if ready > 0 {
                return Ok(true);
            }
        }
    }

    fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout_ms = self.read_timeout_ms.load(Ordering::SeqCst);
        if timeout_ms != 0
            && !self.nonblocking.load(Ordering::SeqCst)
            && !self.wait_readable(timeout_ms)?
        {
            // Shut down for reading while waiting: EOF, as on Unix (see `shutdown`).
            return Ok(0);
        }
        let len = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: buf is valid for writes of len bytes, and len fits in an i32.
        let n = unsafe { ws::recv(self.raw, buf.as_mut_ptr(), len, 0) };
        if n == ws::SOCKET_ERROR {
            let err = last_error();
            if self.read_shut.load(Ordering::SeqCst) {
                // Shut down for reading: EOF, as on Unix (see `shutdown`).
                return Ok(0);
            }
            Err(err)
        } else {
            Ok(n as usize)
        }
    }

    fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(i32::MAX as usize) as i32;
        // SAFETY: buf is valid for reads of len bytes, and len fits in an i32.
        let n = unsafe { ws::send(self.raw, buf.as_ptr(), len, 0) };
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
                ws::getpeername(self.raw, name, &mut len)
            } else {
                ws::getsockname(self.raw, name, &mut len)
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
///
/// Clones share one WinSock socket (closed when the last clone drops), like
/// `dup`ed descriptors on Unix: a timeout, non-blocking mode or `shutdown` set
/// through one clone applies to all of them. (A `WSADuplicateSocketW` handle
/// would not carry the timeouts, so a reader blocked on a clone could neither
/// time out nor be woken by a `shutdown` through another.)
#[derive(Debug)]
pub struct UnixStream(Arc<Socket>);

impl UnixStream {
    /// Connects to the listener bound at `path`.
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        let socket = Socket::new()?;
        let (addr, len) = sockaddr_for(path.as_ref())?;
        // SAFETY: addr is a fully initialized SOCKADDR_UN and len is its size.
        check(unsafe { ws::connect(socket.raw, sockaddr_ptr(&addr), len) })?;
        Ok(UnixStream(Arc::new(socket)))
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

    /// Another handle to the same socket (see the type's documentation).
    pub fn try_clone(&self) -> io::Result<UnixStream> {
        Ok(UnixStream(Arc::clone(&self.0)))
    }

    /// Enables or disables non-blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.set_nonblocking(nonblocking)
    }

    /// Sets the read timeout. `None` blocks indefinitely.
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(dur)
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
        self.0.raw as RawSocket
    }
}

/// Largest chunk moved through the bounce buffer of the volatile I/O impls below.
const VOLATILE_CHUNK: usize = 64 * 1024;

/// `Read` into a bounce buffer, then copy into the (possibly guest-backed) volatile slice.
fn read_volatile_via<R: Read, B: BitmapSlice>(
    reader: &mut R,
    buf: &mut VolatileSlice<B>,
) -> Result<usize, VolatileMemoryError> {
    let mut tmp = vec![0u8; buf.len().min(VOLATILE_CHUNK)];
    let n = reader
        .read(&mut tmp)
        .map_err(VolatileMemoryError::IOError)?;
    buf.copy_from(&tmp[..n]);
    Ok(n)
}

/// Copy out of the (possibly guest-backed) volatile slice into a bounce buffer, then `Write` it.
fn write_volatile_via<W: Write, B: BitmapSlice>(
    writer: &mut W,
    buf: &VolatileSlice<B>,
) -> Result<usize, VolatileMemoryError> {
    let mut tmp = vec![0u8; buf.len().min(VOLATILE_CHUNK)];
    let n = buf.copy_to(&mut tmp[..]);
    writer
        .write(&tmp[..n])
        .map_err(VolatileMemoryError::IOError)
}

impl ReadVolatile for UnixStream {
    fn read_volatile<B: BitmapSlice>(
        &mut self,
        buf: &mut VolatileSlice<B>,
    ) -> Result<usize, VolatileMemoryError> {
        read_volatile_via(self, buf)
    }
}

impl ReadVolatile for &UnixStream {
    fn read_volatile<B: BitmapSlice>(
        &mut self,
        buf: &mut VolatileSlice<B>,
    ) -> Result<usize, VolatileMemoryError> {
        read_volatile_via(self, buf)
    }
}

impl WriteVolatile for UnixStream {
    fn write_volatile<B: BitmapSlice>(
        &mut self,
        buf: &VolatileSlice<B>,
    ) -> Result<usize, VolatileMemoryError> {
        write_volatile_via(self, buf)
    }
}

impl WriteVolatile for &UnixStream {
    fn write_volatile<B: BitmapSlice>(
        &mut self,
        buf: &VolatileSlice<B>,
    ) -> Result<usize, VolatileMemoryError> {
        write_volatile_via(self, buf)
    }
}

/// A Unix-domain socket listening for connections.
#[derive(Debug)]
pub struct UnixListener(Arc<Socket>, PathBuf);

impl UnixListener {
    /// Binds to `path` and starts listening. The socket file is left in place
    /// when the listener is dropped, as with std's `UnixListener`.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
        let path = path.as_ref();
        let socket = Socket::new()?;
        let (addr, len) = sockaddr_for(path)?;
        // SAFETY: addr is a fully initialized SOCKADDR_UN and len is its size.
        check(unsafe { ws::bind(socket.raw, sockaddr_ptr(&addr), len) })?;
        // SAFETY: socket is bound and not yet listening.
        check(unsafe { ws::listen(socket.raw, 128) })?;
        Ok(UnixListener(Arc::new(socket), path.to_path_buf()))
    }

    /// Accepts the next connection, blocking unless non-blocking mode is set.
    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        // SAFETY: SOCKADDR_UN is plain data, so all-zero is valid.
        let mut addr: ws::SOCKADDR_UN = unsafe { zeroed() };
        let mut len = size_of::<ws::SOCKADDR_UN>() as i32;
        let out: *mut ws::SOCKADDR = (&mut addr as *mut ws::SOCKADDR_UN).cast();
        // SAFETY: out is writable for len bytes, and len holds that size.
        let s = unsafe { ws::accept(self.0.raw, out, &mut len) };
        if s == ws::INVALID_SOCKET {
            return Err(last_error());
        }
        // An accepted socket inherits the listener's non-blocking mode.
        let stream = Socket::from_raw(s);
        stream
            .nonblocking
            .store(self.0.nonblocking.load(Ordering::SeqCst), Ordering::SeqCst);
        Ok((
            UnixStream(Arc::new(stream)),
            SocketAddr::from_raw(&addr, len),
        ))
    }

    /// Iterates over incoming connections, blocking on each `next` call.
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }

    /// Enables or disables non-blocking mode for `accept`.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.set_nonblocking(nonblocking)
    }

    /// Another handle to the same listening socket.
    pub fn try_clone(&self) -> io::Result<UnixListener> {
        Ok(UnixListener(Arc::clone(&self.0), self.1.clone()))
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
        self.0.raw as RawSocket
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
