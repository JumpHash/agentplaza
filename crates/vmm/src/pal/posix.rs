// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! POSIX (macOS) implementation of the `vmm-sys-util` `eventfd` and `epoll`
//! subset that the virtio devices use. It is also compiled into Linux test
//! builds so its tests run there; see [`super`] for the design.

// `unsafe` is confined to the pipe(2), fcntl(2), poll(2), read(2)/write(2) on the pipe, and socket(2)/connect(2) calls.
#![allow(unsafe_code)]
// On Linux this module is compiled only for its tests (Linux uses vmm-sys-util).
#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

pub use libc::iovec;
pub use std::os::fd::{AsRawFd, RawFd};

use std::io::{self, ErrorKind};
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, MutexGuard};

/// Locks a mutex, ignoring poisoning: the guarded state is plain data that is
/// left consistent by every critical section, so a panicked peer must not
/// take the device model down with it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sets `FD_CLOEXEC` on `fd`, so it is not inherited across `exec`.
fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: F_SETFD only reads an int argument; fcntl touches no memory.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Sets `O_NONBLOCK` on `fd`, keeping its other status flags.
fn set_nonblocking_fd(fd: RawFd) -> io::Result<()> {
    // SAFETY: F_GETFL takes no argument and touches no memory.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_SETFL only reads an int argument; fcntl touches no memory.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub mod eventfd {
    use super::{AsRawFd, RawFd, lock, set_cloexec, set_nonblocking_fd};
    use std::io::{self, ErrorKind};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::{Arc, Mutex};

    /// Accepted and ignored: reads never block, they return `WouldBlock`.
    pub const EFD_NONBLOCK: i32 = libc::O_NONBLOCK;

    struct Inner {
        counter: Mutex<u64>,
        /// Readable while the counter is non-zero; its descriptor is the fd.
        rx: OwnedFd,
        /// Written to when the counter goes from zero to non-zero.
        tx: OwnedFd,
    }

    pub struct EventFd {
        inner: Arc<Inner>,
    }

    /// A pipe whose ends are both close-on-exec and non-blocking. Each end is
    /// wrapped as soon as it exists, so a failed flag change closes both.
    fn pipe_pair() -> io::Result<(OwnedFd, OwnedFd)> {
        let mut fds: [libc::c_int; 2] = [-1; 2];
        // SAFETY: pipe(2) writes exactly two descriptors into the two-element array.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: pipe(2) succeeded, so fds[0] is a fresh descriptor owned by nothing else; it is wrapped exactly once.
        let rx = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        // SAFETY: as above, for fds[1], which is wrapped exactly once.
        let tx = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        for fd in [rx.as_raw_fd(), tx.as_raw_fd()] {
            set_cloexec(fd)?;
            set_nonblocking_fd(fd)?;
        }
        Ok((rx, tx))
    }

    impl EventFd {
        pub fn new(_flags: i32) -> io::Result<EventFd> {
            let (rx, tx) = pipe_pair()?;
            Ok(EventFd {
                inner: Arc::new(Inner {
                    counter: Mutex::new(0),
                    rx,
                    tx,
                }),
            })
        }

        /// Adds `v` to the counter (saturating) and signals readers.
        pub fn write(&self, v: u64) -> io::Result<()> {
            // The counter lock is held across the write so that a concurrent
            // `read` cannot drain the byte this write is about to queue.
            let mut counter = lock(&self.inner.counter);
            let before = *counter;
            *counter = before.saturating_add(v);
            if before == 0 && *counter != 0 {
                let byte = [1u8];
                // SAFETY: writes one byte from the live stack array `byte` to the pipe's write end, which this EventFd owns.
                let n = unsafe { libc::write(self.inner.tx.as_raw_fd(), byte.as_ptr().cast(), 1) };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() != ErrorKind::WouldBlock {
                        return Err(e);
                    }
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
                // SAFETY: reads at most `buf.len()` bytes into the live stack buffer `buf` from the pipe's read end, which this EventFd owns.
                let n = unsafe {
                    libc::read(
                        self.inner.rx.as_raw_fd(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                    )
                };
                if n > 0 {
                    continue;
                }
                if n == 0 {
                    break;
                }
                let e = io::Error::last_os_error();
                match e.kind() {
                    ErrorKind::WouldBlock => break,
                    ErrorKind::Interrupted => continue,
                    _ => {
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

        /// A second handle to the same counter and pipe.
        pub fn try_clone(&self) -> io::Result<EventFd> {
            Ok(EventFd {
                inner: Arc::clone(&self.inner),
            })
        }
    }

    impl AsRawFd for EventFd {
        fn as_raw_fd(&self) -> RawFd {
            self.inner.rx.as_raw_fd()
        }
    }

    impl std::fmt::Debug for EventFd {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let counter = *lock(&self.inner.counter);
            f.debug_struct("EventFd")
                .field("counter", &counter)
                .field("fd", &self.as_raw_fd())
                .finish()
        }
    }
}

pub mod epoll {
    use super::{AsRawFd, RawFd, lock};
    use std::collections::HashMap;
    use std::io::{self, ErrorKind};
    use std::ops::{BitAnd, BitOr, BitOrAssign};
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, Weak};
    use std::time::Duration;

    /// Longest sleep when there is nothing to poll.
    const IDLE_CAP_MS: i32 = 50;

    /// Pseudo handles are negative, so they cannot collide with a real
    /// descriptor (which is never negative). Starts at 2 so -1 is never issued.
    static NEXT_HANDLE: AtomicI32 = AtomicI32::new(2);

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
            self.data as RawFd
        }
    }

    pub struct Epoll {
        handle: RawFd,
        registry: Arc<Registry>,
        /// Rotates where `wait` starts reporting, so that when more entries are
        /// ready than the caller has room for, none is starved.
        cursor: AtomicUsize,
    }

    /// The poll request for one descriptor: the events the caller asked for.
    fn poll_entry(fd: RawFd, ev: EpollEvent) -> libc::pollfd {
        let mut events: libc::c_short = 0;
        if ev.events & EventSet::IN.0 != 0 {
            events |= libc::POLLIN;
        }
        if ev.events & EventSet::OUT.0 != 0 {
            events |= libc::POLLOUT;
        }
        libc::pollfd {
            fd,
            events,
            revents: 0,
        }
    }

    impl Epoll {
        pub fn new() -> io::Result<Epoll> {
            let handle = -NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
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

            // One poll entry per descriptor. `owner` maps it to its snapshot index.
            let mut pfds: Vec<libc::pollfd> = Vec::new();
            let mut owner: Vec<usize> = Vec::new();
            for (i, (fd, ev)) in snapshot.iter().enumerate() {
                if *fd < 0 {
                    // A nested Epoll: its descriptors report for this entry. Deeper
                    // nesting, and a dropped Epoll, contribute nothing.
                    if let Some(inner) = live_registry(*fd) {
                        let inner_entries: Vec<(RawFd, EpollEvent)> = lock(&inner.entries)
                            .iter()
                            .filter(|(f, _)| **f >= 0)
                            .map(|(f, e)| (*f, *e))
                            .collect();
                        for (sock, sev) in inner_entries {
                            pfds.push(poll_entry(sock, sev));
                            owner.push(i);
                        }
                    }
                } else {
                    pfds.push(poll_entry(*fd, *ev));
                    owner.push(i);
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

            let rc = {
                // SAFETY: `pfds` is an exclusively borrowed, initialised Vec of `pfds.len()` pollfd values; poll(2) reads and writes only the first `nfds` entries, and `nfds` is exactly that length.
                unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout) }
            };
            if rc < 0 {
                // An EINTR is returned as is; the callers retry.
                return Err(io::Error::last_os_error());
            }

            // Map readiness back onto each snapshot entry.
            let mut ready = vec![EventSet::empty(); snapshot.len()];
            for (pfd, &i) in pfds.iter().zip(owner.iter()) {
                let r = pfd.revents;
                let bits = &mut ready[i];
                if r & libc::POLLIN != 0 {
                    *bits |= EventSet::IN;
                }
                if r & libc::POLLHUP != 0 {
                    *bits |= EventSet::IN | EventSet::HANG_UP;
                }
                if r & libc::POLLOUT != 0 {
                    *bits |= EventSet::OUT;
                }
                if r & (libc::POLLERR | libc::POLLNVAL) != 0 {
                    *bits |= EventSet::ERROR;
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

/// Connects to the Unix socket at `path` without ever blocking: the descriptor is made non-blocking
/// *before* `connect(2)`, so a listener whose accept backlog is full fails the connect (`EAGAIN` on
/// Linux, `ECONNREFUSED` on macOS) instead of parking the calling thread (the vsock event thread).
pub fn connect_unix_nonblocking(path: &str) -> io::Result<UnixStream> {
    let bytes = path.as_bytes();
    // SAFETY: sockaddr_un is plain old data; the all-zero bit pattern is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.contains(&0) || bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "socket path is too long or contains a NUL byte",
        ));
    }

    // SAFETY: socket(2) takes no pointer arguments.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket(2) succeeded, so `fd` is a fresh descriptor owned by nothing else; the stream takes ownership and closes it on every error path below.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    set_cloexec(fd)?;
    stream.set_nonblocking(true)?;

    let offset = std::mem::offset_of!(libc::sockaddr_un, sun_path);
    let len = offset + bytes.len() + 1;
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    #[cfg(target_os = "macos")]
    {
        addr.sun_len = len as u8;
    }
    for (dst, &b) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = b as libc::c_char;
    }

    // SAFETY: `addr` is a live, initialised sockaddr_un, and `len` is at most its size: the path is shorter than `sun_path` and `len` counts the path, its NUL and the header before `sun_path`.
    let rc = unsafe { libc::connect(fd, std::ptr::addr_of!(addr).cast(), len as libc::socklen_t) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::AsRawFd;
    use super::connect_unix_nonblocking;
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

    #[test]
    fn eventfd_fd_is_readable_only_while_counted() {
        let ep = Epoll::new().unwrap();
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        ep.ctl(
            ControlOperation::Add,
            efd.as_raw_fd(),
            EpollEvent::new(EventSet::IN, 9),
        )
        .unwrap();
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(ep.wait(0, &mut out).unwrap(), 0);
        efd.write(1).unwrap();
        assert_eq!(ep.wait(0, &mut out).unwrap(), 1);
        efd.read().unwrap();
        assert_eq!(ep.wait(0, &mut out).unwrap(), 0);
    }

    #[test]
    fn pseudo_handles_are_negative_and_distinct() {
        let a = Epoll::new().unwrap();
        let b = Epoll::new().unwrap();
        assert!(a.as_raw_fd() < 0);
        assert!(b.as_raw_fd() < 0);
        assert_ne!(a.as_raw_fd(), b.as_raw_fd());
    }

    #[test]
    fn connect_unix_nonblocking_reaches_a_listener() {
        let path = std::env::temp_dir().join(format!("apz-posix-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let connected = connect_unix_nonblocking(path.to_str().unwrap());
        let accepted = listener.accept();
        let _ = std::fs::remove_file(&path);
        connected.unwrap();
        accepted.unwrap();

        let missing =
            std::env::temp_dir().join(format!("apz-posix-missing-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        assert!(connect_unix_nonblocking(missing.to_str().unwrap()).is_err());
    }

    #[test]
    fn connect_unix_nonblocking_rejects_bad_paths() {
        assert_eq!(
            connect_unix_nonblocking("/tmp/a\0b.sock")
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
        let long = "/".repeat(200);
        assert_eq!(
            connect_unix_nonblocking(&long).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }
}
