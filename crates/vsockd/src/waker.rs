// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: a wake-up for a thread blocked in `sys::wait_readable`,
// so that a command arriving on a channel can interrupt a wait on a socket.

//! A way to wake a thread waiting in `sys::wait_readable`: a connected pair of local stream sockets (AF_UNIX on
//! every platform, through `apz_uds`). `wake` writes a byte to one end; the waiting thread polls the other end.

use std::io::{self, Read, Write};

use apz_uds::UnixStream;

/// The two ends of a connected local stream pair. Both ends are non-blocking.
pub struct Waker {
    tx: UnixStream,
    rx: UnixStream,
}

impl Waker {
    /// Creates a connected pair with both ends non-blocking.
    pub fn new() -> io::Result<Self> {
        let (tx, rx) = UnixStream::pair()?;
        tx.set_nonblocking(true)?;
        rx.set_nonblocking(true)?;
        Ok(Self { tx, rx })
    }

    /// Makes the waiting thread's `wait_readable` return. Every error is ignored: a full buffer means a wake is
    /// already pending.
    pub fn wake(&self) {
        let _ = (&self.tx).write(&[1]);
    }

    /// Discards every pending wake byte, so the next wait blocks again.
    pub fn drain(&self) {
        let mut buf = [0u8; 64];
        loop {
            match (&self.rx).read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    }

    /// The readable end, for `sys::wait_readable`.
    #[cfg(unix)]
    pub fn handle(&self) -> crate::sys::RawHandle {
        use std::os::fd::AsRawFd;
        self.rx.as_raw_fd()
    }

    /// The readable end, for `sys::wait_readable`.
    #[cfg(windows)]
    pub fn handle(&self) -> crate::sys::RawHandle {
        use std::os::windows::io::AsRawSocket;
        self.rx.as_raw_socket()
    }

    /// The writable end, for a signal or console handler to wake the waiting threads with. The waker keeps it
    /// open for its own life.
    #[cfg(unix)]
    pub fn wake_handle(&self) -> crate::sys::RawHandle {
        use std::os::fd::AsRawFd;
        self.tx.as_raw_fd()
    }

    /// The writable end, for a signal or console handler to wake the waiting threads with. The waker keeps it
    /// open for its own life.
    #[cfg(windows)]
    pub fn wake_handle(&self) -> crate::sys::RawHandle {
        use std::os::windows::io::AsRawSocket;
        self.tx.as_raw_socket()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{RawHandle, wait_readable};
    use std::time::Duration;

    /// The readiness handle of `stream`.
    #[cfg(unix)]
    fn raw_of(stream: &UnixStream) -> RawHandle {
        use std::os::fd::AsRawFd;
        stream.as_raw_fd()
    }

    /// The readiness handle of `stream`.
    #[cfg(windows)]
    fn raw_of(stream: &UnixStream) -> RawHandle {
        use std::os::windows::io::AsRawSocket;
        stream.as_raw_socket()
    }

    #[test]
    fn quiet_waker_times_out() {
        let w = Waker::new().unwrap();
        let ready = wait_readable(&[w.handle()], Some(Duration::from_millis(50))).unwrap();
        assert_eq!(ready, vec![false]);
    }

    #[test]
    fn wake_is_seen_at_once_and_drain_clears_it() {
        let w = Waker::new().unwrap();
        w.wake();
        let ready = wait_readable(&[w.handle()], Some(Duration::from_secs(5))).unwrap();
        assert_eq!(ready, vec![true]);
        w.drain();
        let ready = wait_readable(&[w.handle()], Some(Duration::from_millis(50))).unwrap();
        assert_eq!(ready, vec![false]);
    }

    #[test]
    fn wake_from_another_thread_ends_an_unlimited_wait() {
        let w = Waker::new().unwrap();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                w.wake();
            });
            let ready = wait_readable(&[w.handle()], None).unwrap();
            assert_eq!(ready, vec![true]);
        });
    }

    #[test]
    fn reports_each_handle_in_order() {
        let w = Waker::new().unwrap();
        let (mut peer, mine) = UnixStream::pair().unwrap();
        peer.write_all(&[1]).unwrap();
        let ready =
            wait_readable(&[raw_of(&mine), w.handle()], Some(Duration::from_secs(5))).unwrap();
        assert_eq!(ready, vec![true, false]);
    }
}
