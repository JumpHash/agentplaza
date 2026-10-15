// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! One event API for the virtio devices on every platform that runs them.
//!
//! The device model (the MMIO transport, the vsock device, its event thread
//! and its muxer) is written against `vmm-sys-util`'s `EventFd` and `Epoll`.
//! Linux uses those directly. Windows and macOS have neither, so each gets an
//! implementation of the same surface: `windows` over WinSock and `posix` over
//! pipes and `poll(2)`. Both work the same way:
//!
//! * an `eventfd::EventFd` is a counter plus a channel whose read end is
//!   readable while the counter is non-zero (a connected loopback socket pair
//!   on Windows, a pipe on macOS), so it can sit in a poll set next to the
//!   muxer's host sockets;
//! * an `epoll::Epoll` is a registry polled with `WSAPoll` or `poll(2)`. Its "fd" is a
//!   pseudo handle, and an `Epoll` registered inside another one (the muxer's
//!   inside the event thread's, as on Linux) is flattened into the outer
//!   wait.
//!
//! [`AsRawFd`]/[`RawFd`] name what a poll set holds: a file descriptor on
//! Linux and macOS (or, on macOS, an `Epoll` pseudo handle), a `SOCKET` (or an
//! `Epoll` pseudo handle) on Windows.

/// The scatter/gather element the virtio buffers are built from (`readv`/`writev`
/// layout on Linux and macOS; the same layout, used only in-process, on Windows).
#[cfg(target_os = "linux")]
pub use libc::iovec;
#[cfg(target_os = "linux")]
pub use std::os::fd::{AsRawFd, RawFd};
#[cfg(target_os = "linux")]
pub use vmm_sys_util::{epoll, eventfd};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{AsRawFd, RawFd, epoll, eventfd, iovec};

// The POSIX layer is macOS's; it is also compiled into Linux test builds, so
// its unit tests run on the Linux hosts that build this crate.
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod posix;
#[cfg(target_os = "macos")]
pub use posix::{AsRawFd, RawFd, connect_unix_nonblocking, epoll, eventfd, iovec};
