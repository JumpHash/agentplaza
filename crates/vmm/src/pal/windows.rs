// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! Placeholder: the WinSock implementation of `pal` (filled in by its own slice).
#![allow(unsafe_code)]

pub type RawFd = u64;
pub trait AsRawFd {
    fn as_raw_fd(&self) -> RawFd;
}
pub mod eventfd {}
pub mod epoll {}
