// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! POSIX (macOS) implementation of the `vmm-sys-util` `eventfd` and `epoll`
//! subset that the virtio devices use. It is also compiled into Linux test
//! builds so its tests run there; see [`super`] for the design.

// `unsafe` is confined to the pipe(2), fcntl(2), poll(2), read(2)/write(2) on the pipe, socket(2)/connect(2), and getrlimit(2)/setrlimit(2) calls.
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
        /// Readable while the counter is non-zero.
        rx: OwnedFd,
        /// Written to when the counter goes from zero to non-zero.
        tx: OwnedFd,
    }

    pub struct EventFd {
        inner: Arc<Inner>,
        /// A duplicate of the pipe's read end: its own descriptor, on the same
        /// pipe. This is the fd that is registered and polled, so each handle
        /// has a distinct number, as a Linux `dup` of an eventfd does.
        fd: OwnedFd,
    }

    /// Queues one byte on the pipe, which makes its read end readable. A full pipe already holds a
    /// byte, so `WouldBlock` counts as done.
    fn queue_wake_byte(tx: &OwnedFd) -> io::Result<()> {
        let byte = [1u8];
        loop {
            // SAFETY: writes one byte from the live stack array `byte` to the pipe's write end, which the caller owns.
            let n = unsafe { libc::write(tx.as_raw_fd(), byte.as_ptr().cast(), 1) };
            if n >= 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                ErrorKind::WouldBlock => return Ok(()),
                ErrorKind::Interrupted => continue,
                _ => return Err(e),
            }
        }
    }

    /// Duplicates `fd` as a new close-on-exec descriptor.
    fn dup_cloexec(fd: RawFd) -> io::Result<OwnedFd> {
        // SAFETY: F_DUPFD_CLOEXEC takes an int argument and touches no memory; `fd` is only read.
        let new = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if new < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fcntl succeeded, so `new` is a fresh descriptor owned by nothing else; it is wrapped exactly once.
        Ok(unsafe { OwnedFd::from_raw_fd(new) })
    }

    /// A pipe whose ends are both close-on-exec and non-blocking. Each end is
    /// wrapped as soon as it exists, so a failed flag change closes both.
    ///
    /// macOS has neither `pipe2` nor `SOCK_CLOEXEC`, so close-on-exec is set
    /// right after creation. A `fork` and `exec` by another thread in between
    /// would let the child inherit the descriptor. std's own `pipe` on macOS
    /// has the same window, and it cannot be closed from here.
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
            let fd = dup_cloexec(rx.as_raw_fd())?;
            Ok(EventFd {
                inner: Arc::new(Inner {
                    counter: Mutex::new(0),
                    rx,
                    tx,
                }),
                fd,
            })
        }

        /// Adds `v` to the counter (saturating) and signals readers.
        pub fn write(&self, v: u64) -> io::Result<()> {
            // The counter lock is held across the write so that a concurrent
            // `read` cannot drain the byte this write is about to queue.
            let mut counter = lock(&self.inner.counter);
            let before = *counter;
            *counter = before.saturating_add(v);
            if before == 0
                && *counter != 0
                && let Err(e) = queue_wake_byte(&self.inner.tx)
            {
                // No byte was queued: undo the increment so the counter and the pipe agree.
                *counter = before;
                return Err(e);
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
                        // Bytes already drained are gone. If the restored counter is non-zero, a byte
                        // must be queued again, or no later write (which queues only on 0 -> non-zero)
                        // would wake a reader. A failure to re-arm is ignored: the error is returned.
                        *counter = taken;
                        if taken != 0 {
                            let _ = queue_wake_byte(&self.inner.tx);
                        }
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

        /// A second handle to the same counter and pipe, with its own descriptor.
        ///
        /// One difference from Linux remains: a registration in an `Epoll` is
        /// by this handle's own descriptor, so it ends when this handle is
        /// dropped, even while a clone lives on (Linux keeps it until the last
        /// duplicate closes). Every caller keeps the handle it registered for
        /// as long as the registration.
        pub fn try_clone(&self) -> io::Result<EventFd> {
            Ok(EventFd {
                inner: Arc::clone(&self.inner),
                fd: dup_cloexec(self.fd.as_raw_fd())?,
            })
        }
    }

    impl AsRawFd for EventFd {
        fn as_raw_fd(&self) -> RawFd {
            self.fd.as_raw_fd()
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
    use super::eventfd::EventFd;
    use super::{AsRawFd, RawFd, lock};
    use std::cell::OnceCell;
    use std::collections::HashMap;
    use std::io::{self, ErrorKind};
    use std::ops::{BitAnd, BitOr, BitOrAssign};
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, Weak};
    use std::time::{Duration, Instant};

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

    /// Whether `fd` names no open descriptor (`fcntl(F_GETFD)` fails with `EBADF`).
    fn is_closed(fd: RawFd) -> bool {
        // SAFETY: F_GETFD only reads the descriptor's flags; any value of `fd` is allowed.
        let rc = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        rc < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EBADF)
    }

    /// The registrations of one Epoll, and the wakes of the threads now waiting on it.
    struct Registry {
        entries: Mutex<HashMap<RawFd, EpollEvent>>,
        /// The wakes of the threads waiting on this registry, directly or through an outer Epoll that
        /// flattens it. Every successful `ctl` writes each one, so a `wait` blocked in poll(2) looks at
        /// the registrations again.
        waiters: Mutex<Vec<Arc<EventFd>>>,
    }

    thread_local! {
        /// This thread's wake descriptor, created by its first `wait`.
        static THREAD_WAKE: OnceCell<Arc<EventFd>> = const { OnceCell::new() };
    }

    /// This thread's wake descriptor, created on first use.
    fn this_thread_wake() -> io::Result<Arc<EventFd>> {
        THREAD_WAKE.with(|cell| -> io::Result<Arc<EventFd>> {
            if let Some(wake) = cell.get() {
                return Ok(Arc::clone(wake));
            }
            let wake = Arc::new(EventFd::new(0)?);
            // Only this thread sets the cell, and only once, so `set` cannot fail.
            let _ = cell.set(Arc::clone(&wake));
            Ok(wake)
        })
    }

    /// A waiting thread's wake, registered with the registries one `wait` pass polls. Dropping it
    /// unregisters the wake from each, so every return path leaves nothing registered.
    struct Registration {
        wake: Arc<EventFd>,
        registries: Vec<Arc<Registry>>,
    }

    impl Registration {
        fn new(wake: Arc<EventFd>) -> Registration {
            Registration {
                wake,
                registries: Vec::new(),
            }
        }

        /// Registers the wake with `registry`, unless it already is.
        fn add(&mut self, registry: &Arc<Registry>) {
            if self.registries.iter().any(|r| Arc::ptr_eq(r, registry)) {
                return;
            }
            lock(&registry.waiters).push(Arc::clone(&self.wake));
            self.registries.push(Arc::clone(registry));
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            for registry in &self.registries {
                lock(&registry.waiters).retain(|w| !Arc::ptr_eq(w, &self.wake));
            }
        }
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

    /// Where one poll entry came from.
    struct Source {
        /// Snapshot index of the entry the descriptor reports for. Unused for a wake descriptor.
        owner: usize,
        /// For a descriptor of a nested Epoll, the events its inner entry registered.
        inner: Option<EventSet>,
        /// The nested registry it was read from, or `None` for a direct entry.
        home: Option<Arc<Registry>>,
        /// Whether this is the waiting thread's wake descriptor. Never reported.
        wake: bool,
    }

    impl Epoll {
        pub fn new() -> io::Result<Epoll> {
            let handle = -NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
            let registry = Arc::new(Registry {
                entries: Mutex::new(HashMap::new()),
                waiters: Mutex::new(Vec::new()),
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
            // Release the lock before waking, so the woken waiter can take it.
            drop(entries);
            for wake in lock(&self.registry.waiters).iter() {
                let _ = wake.write(1);
            }
            Ok(())
        }

        /// Waits up to `timeout_ms` (negative: forever) and reports up to
        /// `events.len()` ready entries. Returns how many were written.
        ///
        /// A `ctl` from any thread wakes a blocked wait, which then looks at the
        /// registrations again. With nothing registered it blocks for the whole
        /// timeout, as Linux `epoll_wait` does.
        ///
        /// Concurrent waiters are supported, as on Linux: each waiting thread
        /// registers its own wake with every registry it polls, and a change wakes
        /// them all.
        pub fn wait(&self, timeout_ms: i32, events: &mut [EpollEvent]) -> io::Result<usize> {
            let deadline = (timeout_ms >= 0)
                .then(|| Instant::now() + Duration::from_millis(timeout_ms as u64));
            let wake = this_thread_wake()?;
            loop {
                // This pass's registration: the wake is registered with this registry, then drained.
                // A change made before the snapshot below is in it, and one made after writes the wake.
                let mut registration = Registration::new(Arc::clone(&wake));
                registration.add(&self.registry);
                let _ = wake.read();
                // Snapshot first so no registry lock is held while blocking.
                let snapshot: Vec<(RawFd, EpollEvent)> = lock(&self.registry.entries)
                    .iter()
                    .map(|(fd, ev)| (*fd, *ev))
                    .collect();
                // Each nested Epoll is registered with before its entries are snapshotted below.
                let mut nested: HashMap<RawFd, Arc<Registry>> = HashMap::new();
                for (fd, _) in &snapshot {
                    if *fd >= 0 {
                        continue;
                    }
                    if let Some(inner) = live_registry(*fd) {
                        registration.add(&inner);
                        nested.insert(*fd, inner);
                    }
                }

                // One poll entry per descriptor. `sources` says which snapshot entry it
                // reports for, and which registry it was registered in.
                let mut pfds: Vec<libc::pollfd> = Vec::new();
                let mut sources: Vec<Source> = Vec::new();
                for (i, (fd, ev)) in snapshot.iter().enumerate() {
                    if *fd < 0 {
                        // A nested Epoll. As on Linux, it is readable when any of its own
                        // entries is ready for the events that entry asked for, so each inner
                        // entry is polled with its own registered set. Deeper nesting, and a
                        // dropped Epoll, contribute nothing.
                        if let Some(inner) = nested.get(fd) {
                            let inner_entries: Vec<(RawFd, EpollEvent)> = lock(&inner.entries)
                                .iter()
                                .filter(|(f, _)| **f >= 0)
                                .map(|(f, e)| (*f, *e))
                                .collect();
                            for (sock, sev) in inner_entries {
                                pfds.push(poll_entry(sock, sev));
                                sources.push(Source {
                                    owner: i,
                                    inner: Some(sev.event_set()),
                                    home: Some(Arc::clone(inner)),
                                    wake: false,
                                });
                            }
                        }
                    } else {
                        pfds.push(poll_entry(*fd, *ev));
                        sources.push(Source {
                            owner: i,
                            inner: None,
                            home: None,
                            wake: false,
                        });
                    }
                }

                // This thread's wake. It is always polled, so the set is never empty and
                // poll(2) blocks for the full timeout.
                pfds.push(poll_entry(
                    wake.as_raw_fd(),
                    EpollEvent::new(EventSet::IN, 0),
                ));
                sources.push(Source {
                    owner: 0,
                    inner: None,
                    home: None,
                    wake: true,
                });
                // Rounded up to the next millisecond, so a positive remainder never becomes a busy 0.
                let timeout = match deadline {
                    None => -1,
                    Some(d) => d
                        .saturating_duration_since(Instant::now())
                        .as_micros()
                        .div_ceil(1000)
                        .min(i32::MAX as u128) as i32,
                };

                let rc = {
                    // SAFETY: `pfds` is an exclusively borrowed, initialised Vec of `pfds.len()` pollfd values; poll(2) reads and writes only the first `nfds` entries, and `nfds` is exactly that length.
                    unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout) }
                };
                if rc < 0 {
                    // An EINTR is returned as is; the callers retry.
                    return Err(io::Error::last_os_error());
                }

                // Map readiness back onto each snapshot entry.
                let mut woke = false;
                let mut ready = vec![EventSet::empty(); snapshot.len()];
                // Descriptors found closed (POLLNVAL), with the registry each came from.
                let mut closed: Vec<(Option<Arc<Registry>>, RawFd)> = Vec::new();
                for (pfd, src) in pfds.iter().zip(sources.iter()) {
                    if src.wake {
                        // A wake is never reported. A readable one is drained and marks the
                        // registrations as changed.
                        if pfd.revents & libc::POLLIN != 0 {
                            let _ = wake.read();
                            woke = true;
                        }
                        continue;
                    }
                    let r = pfd.revents;
                    if r & libc::POLLNVAL != 0 {
                        // Closed without a Delete. Linux drops such a descriptor by itself, so
                        // report nothing for it and drop it from its registry below. That is
                        // a change of registrations, so an otherwise empty pass looks again
                        // rather than end an infinite wait with nothing.
                        closed.push((src.home.clone(), pfd.fd));
                        woke = true;
                        continue;
                    }
                    let mut bits = EventSet::empty();
                    if r & libc::POLLIN != 0 {
                        bits |= EventSet::IN;
                    }
                    if r & libc::POLLHUP != 0 {
                        bits |= EventSet::IN | EventSet::HANG_UP;
                    }
                    if r & libc::POLLOUT != 0 {
                        bits |= EventSet::OUT;
                    }
                    if r & libc::POLLERR != 0 {
                        bits |= EventSet::ERROR;
                    }
                    match src.inner {
                        // Linux semantics for a nested epoll: it is readable when one of its
                        // entries is ready for the events that entry registered, whatever the
                        // outer entry asked for. So the outer entry gets IN and nothing else.
                        Some(inner) => {
                            let inner_wanted = (inner & (EventSet::IN | EventSet::OUT))
                                | EventSet::ERROR
                                | EventSet::HANG_UP;
                            if bits.intersects(inner_wanted) {
                                ready[src.owner] |= EventSet::IN;
                            }
                        }
                        None => ready[src.owner] |= bits,
                    }
                }
                // Drop the closed descriptors, each from the registry it came from. Only
                // brief locks: none is held while polling. A descriptor reopened under the
                // same number since the poll is kept: each is removed only if it is still
                // closed when checked under its registry's lock.
                for (home, fd) in closed {
                    let registry = home.as_deref().unwrap_or(&*self.registry);
                    let mut entries = lock(&registry.entries);
                    if is_closed(fd) {
                        entries.remove(&fd);
                    }
                }

                let len = snapshot.len();
                let start = if len == 0 {
                    0
                } else {
                    self.cursor.fetch_add(1, Ordering::Relaxed) % len
                };
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
                if written > 0 {
                    return Ok(written);
                }
                // Nothing to report. Look again only if the registrations changed and the
                // deadline has not passed; otherwise the timeout is over.
                let deadline_passed = deadline.is_some_and(|d| Instant::now() >= d);
                if !woke || deadline_passed {
                    return Ok(0);
                }
            }
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
    // macOS has no `SOCK_CLOEXEC`, so close-on-exec is set right after creation. A `fork` and `exec`
    // by another thread in between would let the child inherit the socket, which would keep a vsock
    // connection open after the muxer closes it. std's own sockets on macOS have the same window; it
    // cannot be closed from here.
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

/// macOS's `OPEN_MAX`: the highest soft limit on open descriptors `raise_open_files_limit` sets.
pub const OPEN_FILES_CAP: u64 = 10_240;

/// Raises this process's soft limit on open descriptors to its hard limit, capped at `cap`, and
/// returns the new soft limit. macOS starts processes at 256, which one VM's devices, the muxer's
/// connections and its event descriptors can exhaust; a failed `accept` then stalls the guest's
/// vsock. A limit already at least `cap` is left alone.
pub fn raise_open_files_limit(cap: u64) -> io::Result<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit(2) writes one `rlimit` into the live, exclusively borrowed `limit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let current = limit.rlim_cur;
    let wanted = limit.rlim_max.min(cap);
    if wanted <= current {
        return Ok(current);
    }
    limit.rlim_cur = wanted as libc::rlim_t;
    // SAFETY: setrlimit(2) reads one `rlimit` from the live `limit` and changes no memory.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(wanted)
}

#[cfg(test)]
mod tests {
    use super::AsRawFd;
    use super::connect_unix_nonblocking;
    use super::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
    use super::eventfd::{EFD_NONBLOCK, EventFd};
    use std::io::ErrorKind;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

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
    fn try_clone_gives_a_distinct_descriptor_on_the_same_counter() {
        let a = EventFd::new(EFD_NONBLOCK).unwrap();
        let b = a.try_clone().unwrap();
        assert_ne!(a.as_raw_fd(), b.as_raw_fd());
        b.write(3).unwrap();
        assert_eq!(a.read().unwrap(), 3);
        let ep = Epoll::new().unwrap();
        ep.ctl(
            ControlOperation::Add,
            b.as_raw_fd(),
            EpollEvent::new(EventSet::IN, 4),
        )
        .unwrap();
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(ep.wait(0, &mut out).unwrap(), 0);
        a.write(1).unwrap();
        assert_eq!(ep.wait(0, &mut out).unwrap(), 1);
        assert_eq!(out[0].data(), 4);
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
    fn a_registration_from_another_thread_wakes_a_blocked_wait() {
        let ep = Arc::new(Epoll::new().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = Arc::clone(&ep);
        let handle = std::thread::spawn(move || {
            let mut out = [EpollEvent::default(); 4];
            tx.send(waiter.wait(-1, &mut out).unwrap()).unwrap();
        });
        std::thread::sleep(Duration::from_millis(100));
        // Quiet on registration: the waiter can only see it by the wake that `ctl` writes.
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        ep.ctl(
            ControlOperation::Add,
            efd.as_raw_fd(),
            EpollEvent::new(EventSet::IN, 5),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        efd.write(1).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        handle.join().unwrap();
    }

    #[test]
    fn an_empty_epoll_blocks_for_its_timeout() {
        let ep = Epoll::new().unwrap();
        let started = Instant::now();
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(ep.wait(150, &mut out).unwrap(), 0);
        assert!(started.elapsed() >= Duration::from_millis(100));
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
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let inner = Epoll::new().unwrap();
        inner
            .ctl(
                ControlOperation::Add,
                a.as_raw_fd(),
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
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(outer.wait(0, &mut out).unwrap(), 1);
        assert_eq!(out[0].data(), 42);
        assert!(out[0].event_set().contains(EventSet::IN));
    }

    #[test]
    fn two_waiters_both_see_a_registration() {
        let ep = Arc::new(Epoll::new().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let waiter = Arc::clone(&ep);
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                let mut out = [EpollEvent::default(); 4];
                tx.send(waiter.wait(-1, &mut out).unwrap()).unwrap();
            }));
        }
        drop(tx);
        std::thread::sleep(Duration::from_millis(150));
        // Quiet on registration: the waiters can only see it by the wakes that `ctl` writes.
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        ep.ctl(
            ControlOperation::Add,
            efd.as_raw_fd(),
            EpollEvent::new(EventSet::IN, 5),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        efd.write(1).unwrap();
        for _ in 0..2 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        drop(efd);
    }

    #[test]
    fn a_waiter_on_the_inner_epoll_and_one_on_the_outer_both_wake() {
        let inner = Arc::new(Epoll::new().unwrap());
        let outer = Arc::new(Epoll::new().unwrap());
        outer
            .ctl(
                ControlOperation::Add,
                inner.as_raw_fd(),
                EpollEvent::new(EventSet::IN, 42),
            )
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut handles = Vec::new();
        for ep in [Arc::clone(&inner), Arc::clone(&outer)] {
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                let mut out = [EpollEvent::default(); 4];
                tx.send(ep.wait(-1, &mut out).unwrap()).unwrap();
            }));
        }
        drop(tx);
        std::thread::sleep(Duration::from_millis(150));
        let efd = EventFd::new(EFD_NONBLOCK).unwrap();
        inner
            .ctl(
                ControlOperation::Add,
                efd.as_raw_fd(),
                EpollEvent::new(EventSet::IN, 5),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        efd.write(1).unwrap();
        for _ in 0..2 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        drop(efd);
    }

    #[test]
    fn a_closed_descriptor_is_dropped_not_reported() {
        // Never open, so poll(2) reports POLLNVAL for it.
        const CLOSED_FD: i32 = 1_000_000;
        let ep = Epoll::new().unwrap();
        let ev = EpollEvent::new(EventSet::IN, CLOSED_FD as u64);
        ep.ctl(ControlOperation::Add, CLOSED_FD, ev).unwrap();
        let mut out = [EpollEvent::default(); 4];
        assert_eq!(ep.wait(0, &mut out).unwrap(), 0);
        assert_eq!(
            ep.ctl(ControlOperation::Delete, CLOSED_FD, ev)
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
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
