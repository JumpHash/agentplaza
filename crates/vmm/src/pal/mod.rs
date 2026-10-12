// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! One event API for the virtio devices on every platform that runs them.
//!
//! The device model (the MMIO transport, the vsock device, its event thread
//! and its muxer) is written against `vmm-sys-util`'s `EventFd` and `Epoll`.
//! Linux uses those directly. Windows has neither, so [`windows`] implements
//! the same surface over WinSock:
//!
//! * an [`eventfd::EventFd`] is a counter plus a connected loopback socket
//!   pair whose read end is readable while the counter is non-zero, so it can
//!   sit in a poll set next to the muxer's host sockets;
//! * an [`epoll::Epoll`] is a registry polled with `WSAPoll`. Its "fd" is a
//!   pseudo handle, and an `Epoll` registered inside another one (the muxer's
//!   inside the event thread's, as on Linux) is flattened into the outer
//!   wait.
//!
//! [`AsRawFd`]/[`RawFd`] name what a poll set holds: a file descriptor on
//! Linux, a `SOCKET` (or an `Epoll` pseudo handle) on Windows.

#[cfg(target_os = "linux")]
pub use std::os::fd::{AsRawFd, RawFd};
#[cfg(target_os = "linux")]
pub use vmm_sys_util::{epoll, eventfd};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{AsRawFd, RawFd, epoll, eventfd};
