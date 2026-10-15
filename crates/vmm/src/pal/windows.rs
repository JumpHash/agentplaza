// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! WinSock implementation of the `vmm-sys-util` `eventfd` and `epoll` subset
//! that the virtio devices use. See [`super`] for the design.

// `unsafe` is confined to the WSAPoll and WSAGetLastError FFI calls: windows-sys
// exposes them as raw `extern` functions with no safe wrapper.
#![allow(unsafe_code)]

use std::sync::{Mutex, MutexGuard, Once};

pub type RawFd = u64;

/// A scatter/gather element with the layout of POSIX `struct iovec`. WinSock
/// has no `readv`/`writev` for these; the virtio buffers use them in-process
/// only, as (pointer, length) pairs over guest memory.
#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct iovec {
    /// Start of the buffer.
    pub iov_base: *mut std::ffi::c_void,
    /// Length of the buffer in bytes.
    pub iov_len: usize,
}
pub trait AsRawFd {
    fn as_raw_fd(&self) -> RawFd;
}

impl AsRawFd for apz_uds::UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        std::os::windows::io::AsRawSocket::as_raw_socket(self)
    }
}

impl AsRawFd for apz_uds::UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        std::os::windows::io::AsRawSocket::as_raw_socket(self)
    }
}

/// Locks a mutex, ignoring poisoning: the guarded state is plain data that is
/// left consistent by every critical section, so a panicked peer must not
/// take the device model down with it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts WinSock once. Creating any socket does this too; `Epoll::new` calls
/// it so that `WSAPoll` is never reached before WinSock is initialised.
fn ensure_winsock() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = std::net::UdpSocket::bind("127.0.0.1:0");
    });
}

pub mod eventfd {
    use super::{AsRawFd, RawFd, ensure_winsock, lock};
    use std::io::{self, ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::windows::io::AsRawSocket;
    use std::sync::{Arc, Mutex};

    /// Accepted and ignored: reads never block, they return `WouldBlock`.
    pub const EFD_NONBLOCK: i32 = 0o4000;

    struct Inner {
        counter: Mutex<u64>,
        /// Written to when the counter goes from zero to non-zero.
        tx: TcpStream,
        /// Readable while the counter is non-zero; its socket is the fd.
        rx: TcpStream,
    }

    pub struct EventFd {
        inner: Arc<Inner>,
    }

    /// A connected loopback pair. The accepted end must come from the address
    /// we connected to and its peer must be our own stream, so no other local
    /// process can be the peer of the pair.
    fn socket_pair() -> io::Result<(TcpStream, TcpStream)> {
        ensure_winsock();
        for _ in 0..2 {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let tx = TcpStream::connect(listener.local_addr()?)?;
            let tx_local = tx.local_addr()?;
            let (rx, peer) = listener.accept()?;
            if peer != tx_local {
                continue;
            }
            tx.set_nodelay(true)?;
            rx.set_nodelay(true)?;
            tx.set_nonblocking(true)?;
            rx.set_nonblocking(true)?;
            return Ok((tx, rx));
        }
        Err(io::Error::other(
            "loopback pair was connected by another process",
        ))
    }

    impl EventFd {
        pub fn new(_flags: i32) -> io::Result<EventFd> {
            let (tx, rx) = socket_pair()?;
            Ok(EventFd {
                inner: Arc::new(Inner {
                    counter: Mutex::new(0),
                    tx,
                    rx,
                }),
            })
        }

        /// Adds `v` to the counter (saturating) and signals readers.
        pub fn write(&self, v: u64) -> io::Result<()> {
            // The counter lock is held across the send so that a concurrent
            // `read` cannot drain the byte this write is about to queue.
            let mut counter = lock(&self.inner.counter);
            let before = *counter;
            *counter = before.saturating_add(v);
            if before == 0 && *counter != 0 {
                match (&self.inner.tx).write(&[1]) {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }

        /// Returns the counter and resets it to zero. `WouldBlock` when it was zero.
        pub fn read(&self) -> io::Result<u64> {
            let mut counter = lock(&self.inner.counter);
            let taken = std::mem::take(&mut *counter);
            let mut buf = [0u8; 64];
            loop {
                match (&self.inner.rx).read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => {
                        *counter = taken;
                        return Err(e);
                    }
                }
            }
            if taken == 0 {
                Err(io::Error::from(ErrorKind::WouldBlock))
            } else {
                Ok(taken)
            }
        }

        /// A second handle to the same counter and socket pair.
        pub fn try_clone(&self) -> io::Result<EventFd> {
            Ok(EventFd {
                inner: Arc::clone(&self.inner),
            })
        }
    }

    impl AsRawFd for EventFd {
        fn as_raw_fd(&self) -> RawFd {
            self.inner.rx.as_raw_socket()
        }
    }

    impl std::fmt::Debug for EventFd {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let counter = *lock(&self.inner.counter);
            f.debug_struct("EventFd")
                .field("counter", &counter)
                .field("socket", &self.as_raw_fd())
                .finish()
        }
    }
}

pub mod epoll {
    use super::{AsRawFd, RawFd, ensure_winsock, lock};
    use std::collections::HashMap;
    use std::io::{self, ErrorKind};
    use std::ops::{BitAnd, BitOr, BitOrAssign};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, Weak};
    use std::time::Duration;
    use windows_sys::Win32::Networking::WinSock::{
        POLLERR, POLLHUP, POLLRDNORM, POLLWRNORM, SOCKET, SOCKET_ERROR, WSAGetLastError, WSAPOLLFD,
        WSAPoll,
    };

    /// Pseudo handles of `Epoll` instances carry this bit, so they cannot
    /// collide with a SOCKET (which never has it set on Windows).
    const PSEUDO_BIT: RawFd = 1 << 63;
    /// Longest sleep when there is nothing to poll.
    const IDLE_CAP_MS: i32 = 50;

    static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

    /// Live `Epoll` instances by pseudo handle. Weak, so an `Epoll` is freed
    /// when its owner drops it, whatever it is registered in.
    fn registries() -> &'static Mutex<HashMap<RawFd, Weak<Registry>>> {
        static REGISTRIES: OnceLock<Mutex<HashMap<RawFd, Weak<Registry>>>> = OnceLock::new();
        REGISTRIES.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn live_registry(handle: RawFd) -> Option<Arc<Registry>> {
        lock(registries()).get(&handle).and_then(Weak::upgrade)
    }

    struct Registry {
        entries: Mutex<HashMap<RawFd, EpollEvent>>,
    }

    // Bit-compatible with the Linux `EPOLL*` values for the flags listed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct EventSet(u32);

    impl EventSet {
        pub const IN: EventSet = EventSet(0x001);
        pub const OUT: EventSet = EventSet(0x004);
        pub const ERROR: EventSet = EventSet(0x008);
        pub const HANG_UP: EventSet = EventSet(0x010);
        pub const READ_HANG_UP: EventSet = EventSet(0x2000);
        const KNOWN: u32 = 0x001 | 0x004 | 0x008 | 0x010 | 0x2000;

        pub fn bits(&self) -> u32 {
            self.0
        }

        pub fn from_bits(bits: u32) -> Option<EventSet> {
            if (bits & !Self::KNOWN) == 0 {
                Some(EventSet(bits))
            } else {
                None
            }
        }

        /// Adds the flags in `other` (bitflags' `insert`, as vmm-sys-util's
        /// `EventSet` has).
        pub fn insert(&mut self, other: EventSet) {
            self.0 |= other.0;
        }
        /// Removes the flags in `other`.
        pub fn remove(&mut self, other: EventSet) {
            self.0 &= !other.0;
        }
        /// Whether every flag in `other` is set.
        pub fn contains(&self, other: EventSet) -> bool {
            self.0 & other.0 == other.0
        }

        pub fn intersects(&self, other: EventSet) -> bool {
            self.0 & other.0 != 0
        }

        pub fn is_empty(&self) -> bool {
            self.0 == 0
        }

        pub fn empty() -> EventSet {
            EventSet(0)
        }
    }

    impl BitOr for EventSet {
        type Output = EventSet;
        fn bitor(self, rhs: EventSet) -> EventSet {
            EventSet(self.0 | rhs.0)
        }
    }

    impl BitOrAssign for EventSet {
        fn bitor_assign(&mut self, rhs: EventSet) {
            self.0 |= rhs.0;
        }
    }

    impl BitAnd for EventSet {
        type Output = EventSet;
        fn bitand(self, rhs: EventSet) -> EventSet {
            EventSet(self.0 & rhs.0)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ControlOperation {
        Add,
        Modify,
        Delete,
    }

    #[derive(Debug, Clone, Copy, Default)]
    pub struct EpollEvent {
        events: u32,
        data: u64,
    }

    impl EpollEvent {
        pub fn new(events: EventSet, data: u64) -> Self {
            EpollEvent {
                events: events.0,
                data,
            }
        }

        pub fn events(&self) -> u32 {
            self.events
        }

        pub fn event_set(&self) -> EventSet {
            EventSet(self.events)
        }

        pub fn data(&self) -> u64 {
            self.data
        }

        /// As in `vmm-sys-util`, the fd of an event is its data.
        pub fn fd(&self) -> RawFd {
            self.data
        }
    }

    pub struct Epoll {
        handle: RawFd,
        registry: Arc<Registry>,
        /// Rotates where `wait` starts reporting, so that when more entries are
        /// ready than the caller has room for, none is starved.
        cursor: AtomicUsize,
    }

    /// The poll request for one socket: the events the caller asked for.
    fn poll_entry(fd: RawFd, ev: EpollEvent) -> WSAPOLLFD {
        let mut events: i16 = 0;
        if ev.events & EventSet::IN.0 != 0 {
            events |= POLLRDNORM;
        }
        if ev.events & EventSet::OUT.0 != 0 {
            events |= POLLWRNORM;
        }
        WSAPOLLFD {
            fd: fd as SOCKET,
            events,
            revents: 0,
        }
    }

    impl Epoll {
        pub fn new() -> io::Result<Epoll> {
            ensure_winsock();
            let handle = PSEUDO_BIT | NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
            let registry = Arc::new(Registry {
                entries: Mutex::new(HashMap::new()),
            });
            lock(registries()).insert(handle, Arc::downgrade(&registry));
            Ok(Epoll {
                handle,
                registry,
                cursor: AtomicUsize::new(0),
            })
        }

        pub fn ctl(&self, op: ControlOperation, fd: RawFd, event: EpollEvent) -> io::Result<()> {
            let mut entries = lock(&self.registry.entries);
            match op {
                ControlOperation::Add => {
                    if entries.contains_key(&fd) {
                        return Err(io::Error::new(
                            ErrorKind::AlreadyExists,
                            "fd is already registered",
                        ));
                    }
                    entries.insert(fd, event);
                }
                ControlOperation::Modify => match entries.get_mut(&fd) {
                    Some(slot) => *slot = event,
                    None => {
                        return Err(io::Error::new(ErrorKind::NotFound, "fd is not registered"));
                    }
                },
                ControlOperation::Delete => {
                    if entries.remove(&fd).is_none() {
                        return Err(io::Error::new(ErrorKind::NotFound, "fd is not registered"));
                    }
                }
            }
            Ok(())
        }

        /// Waits up to `timeout_ms` (negative: forever) and reports up to
        /// `events.len()` ready entries. Returns how many were written.
        pub fn wait(&self, timeout_ms: i32, events: &mut [EpollEvent]) -> io::Result<usize> {
            let timeout = if timeout_ms < 0 { -1 } else { timeout_ms };

            // Snapshot first so no registry lock is held while blocking.
            let snapshot: Vec<(RawFd, EpollEvent)> = lock(&self.registry.entries)
                .iter()
                .map(|(fd, ev)| (*fd, *ev))
                .collect();

            // One poll entry per socket. `owner` maps it to its snapshot index;
            // `inner_sets` holds the nested entry's registered set (`None` for a
            // direct socket).
            let mut pfds: Vec<WSAPOLLFD> = Vec::new();
            let mut owner: Vec<usize> = Vec::new();
            let mut inner_sets: Vec<Option<EventSet>> = Vec::new();
            for (i, (fd, ev)) in snapshot.iter().enumerate() {
                if *fd & PSEUDO_BIT != 0 {
                    // A nested Epoll reports IN when any of its entries is ready for
                    // what that entry registered, as on Linux. Deeper nesting, and a
                    // dropped Epoll, contribute nothing.
                    if let Some(inner) = live_registry(*fd) {
                        let inner_entries: Vec<(RawFd, EpollEvent)> = lock(&inner.entries)
                            .iter()
                            .filter(|(f, _)| **f & PSEUDO_BIT == 0)
                            .map(|(f, e)| (*f, *e))
                            .collect();
                        for (sock, sev) in inner_entries {
                            pfds.push(poll_entry(sock, sev));
                            owner.push(i);
                            inner_sets.push(Some(sev.event_set()));
                        }
                    }
                } else {
                    pfds.push(poll_entry(*fd, *ev));
                    owner.push(i);
                    inner_sets.push(None);
                }
            }

            if pfds.is_empty() {
                // Nothing can wake us, so do not block: wait out a bounded
                // timeout and report nothing.
                let nap = if timeout < 0 {
                    IDLE_CAP_MS
                } else {
                    timeout.min(IDLE_CAP_MS)
                };
                std::thread::sleep(Duration::from_millis(nap as u64));
                return Ok(0);
            }

            let n = {
                // SAFETY: `pfds` is an exclusively borrowed, initialised Vec of
                // `pfds.len()` WSAPOLLFD values; WSAPoll reads and writes only
                // the first `len` entries, and the length passed is exactly that.
                unsafe { WSAPoll(pfds.as_mut_ptr(), pfds.len() as u32, timeout) }
            };
            if n == SOCKET_ERROR {
                // SAFETY: WSAGetLastError has no preconditions; it reads this
                // thread's last-error value.
                let code = unsafe { WSAGetLastError() };
                return Err(io::Error::from_raw_os_error(code));
            }

            // Map readiness back onto each snapshot entry.
            let mut ready = vec![EventSet::empty(); snapshot.len()];
            for ((pfd, &i), inner_set) in pfds.iter().zip(owner.iter()).zip(inner_sets.iter()) {
                let r = pfd.revents;
                let mut bits = EventSet::empty();
                if r & POLLRDNORM != 0 {
                    bits |= EventSet::IN;
                }
                if r & POLLHUP != 0 {
                    bits |= EventSet::IN | EventSet::HANG_UP;
                }
                if r & POLLWRNORM != 0 {
                    bits |= EventSet::OUT;
                }
                if r & POLLERR != 0 {
                    bits |= EventSet::ERROR;
                }
                match inner_set {
                    // A socket inside a nested Epoll counts only when it is ready for
                    // what its own entry registered (or errored or hung up); the outer
                    // entry then reports IN, and nothing else.
                    Some(inner) => {
                        let asked = (*inner & (EventSet::IN | EventSet::OUT))
                            | EventSet::ERROR
                            | EventSet::HANG_UP;
                        if !(bits & asked).is_empty() {
                            ready[i] |= EventSet::IN;
                        }
                    }
                    None => ready[i] |= bits,
                }
            }

            let len = snapshot.len();
            let start = self.cursor.fetch_add(1, Ordering::Relaxed) % len;
            let mut written = 0;
            for k in 0..len {
                if written == events.len() {
                    break;
                }
                let i = (start + k) % len;
                let (_, registered) = snapshot[i];
                // Only what was asked for, plus the always-reported error and hang-up.
                let wanted = (registered.event_set() & (EventSet::IN | EventSet::OUT))
                    | EventSet::ERROR
                    | EventSet::HANG_UP;
                let bits = ready[i] & wanted;
                if !bits.is_empty() {
                    events[written] = EpollEvent::new(bits, registered.data());
                    written += 1;
                }
            }
            Ok(written)
        }
    }

    impl Drop for Epoll {
        fn drop(&mut self) {
            lock(registries()).remove(&self.handle);
        }
    }

    impl AsRawFd for Epoll {
        fn as_raw_fd(&self) -> RawFd {
            self.handle
        }
    }

    impl std::fmt::Debug for Epoll {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Epoll")
                .field("handle", &self.handle)
                .finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AsRawFd;
    use super::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
    use super::eventfd::{EFD_NONBLOCK, EventFd};
    use std::io::ErrorKind;

    #[test]
    fn eventfd_counts_and_would_block() {
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        assert_eq!(efd.read().unwrap_err().kind(), ErrorKind::WouldBlock);
        efd.write(2).unwrap();
        efd.write(3).unwrap();
        assert_eq!(efd.read().unwrap(), 5);
        assert_eq!(efd.read().unwrap_err().kind(), ErrorKind::WouldBlock);
    }

    #[test]
    fn eventfd_clone_shares_the_counter() {
        let a = EventFd::new(EFD_NONBLOCK).unwrap();
        let b = a.try_clone().unwrap();
        b.write(1).unwrap();
        assert_eq!(a.read().unwrap(), 1);
    }

    #[test]
    fn epoll_reports_eventfd_readiness() {
        let ep = Epoll::new().unwrap();
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        ep.ctl(
            ControlOperation::Add,
            efd.as_raw_fd(),
            EpollEvent::new(EventSet::IN, 7),
        )
        .unwrap();
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(ep.wait(0, &mut out).unwrap(), 0);
        efd.write(1).unwrap();
        assert_eq!(ep.wait(1000, &mut out).unwrap(), 1);
        assert_eq!(out[0].data(), 7);
        assert!(out[0].event_set().contains(EventSet::IN));
        efd.read().unwrap();
        assert_eq!(ep.wait(0, &mut out).unwrap(), 0);
    }

    #[test]
    fn nested_epoll_reports_inner_as_readable() {
        let inner = Epoll::new().unwrap();
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        inner
            .ctl(
                ControlOperation::Add,
                efd.as_raw_fd(),
                EpollEvent::new(EventSet::IN, 1),
            )
            .unwrap();
        let outer = Epoll::new().unwrap();
        outer
            .ctl(
                ControlOperation::Add,
                inner.as_raw_fd(),
                EpollEvent::new(EventSet::IN, 42),
            )
            .unwrap();
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(outer.wait(0, &mut out).unwrap(), 0);
        efd.write(1).unwrap();
        assert_eq!(outer.wait(1000, &mut out).unwrap(), 1);
        assert_eq!(out[0].data(), 42);
        assert!(out[0].event_set().contains(EventSet::IN));
    }

    #[test]
    fn nested_epoll_reports_inner_writability_as_readable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let a = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_b, _) = listener.accept().unwrap();
        let inner = Epoll::new().unwrap();
        inner
            .ctl(
                ControlOperation::Add,
                std::os::windows::io::AsRawSocket::as_raw_socket(&a) as _,
                EpollEvent::new(EventSet::OUT, 1),
            )
            .unwrap();
        let outer = Epoll::new().unwrap();
        outer
            .ctl(
                ControlOperation::Add,
                inner.as_raw_fd(),
                EpollEvent::new(EventSet::IN, 42),
            )
            .unwrap();
        // A fresh connection's send buffer is empty, so the inner socket is writable.
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(outer.wait(1000, &mut out).unwrap(), 1);
        assert_eq!(out[0].data(), 42);
        assert!(out[0].event_set().contains(EventSet::IN));
    }

    #[test]
    fn ctl_add_modify_delete_errors() {
        let ep = Epoll::new().unwrap();
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        let fd = efd.as_raw_fd();
        let ev = EpollEvent::new(EventSet::IN, 1);
        assert_eq!(
            ep.ctl(ControlOperation::Modify, fd, ev).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert_eq!(
            ep.ctl(ControlOperation::Delete, fd, ev).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        ep.ctl(ControlOperation::Add, fd, ev).unwrap();
        assert_eq!(
            ep.ctl(ControlOperation::Add, fd, ev).unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );
        efd.write(1).unwrap();
        ep.ctl(ControlOperation::Delete, fd, ev).unwrap();
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(ep.wait(0, &mut out).unwrap(), 0);
    }
}
