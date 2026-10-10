// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/legacy/mod.rs (trimmed: the
// aarch64-only `gpio_pl061`/`rtc_pl031` modules and the metrics serializer
// are dropped; the serial input plumbing lives in `serial.rs`).

//! Implements legacy devices (UART, RTC etc).
//!
//! Linux/KVM-only as landed: the serial trigger and the i8042 reset port
//! ride on `EventFd`, which does not exist on macOS. The Hypervisor.framework
//! backend wraps `vm_superio` directly with its own portable trigger instead
//! of these modules.
#[cfg(target_os = "linux")]
mod i8042;
#[cfg(target_os = "linux")]
pub mod serial;

#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::ops::Deref;

#[cfg(target_os = "linux")]
use vm_superio::Trigger;
#[cfg(target_os = "linux")]
use vmm_sys_util::eventfd::EventFd;

#[cfg(target_os = "linux")]
pub use self::i8042::{I8042Device, I8042Error as I8042DeviceError};
#[cfg(target_os = "linux")]
pub use self::serial::{SerialDevice, SerialEventsWrapper, SerialOut, SerialOutInner, SerialWrapper};
#[cfg(target_os = "linux")]
/// Wrapper for implementing the trigger functionality for `EventFd`.
///
/// The trigger is used for handling events in the legacy devices.
#[derive(Debug)]
pub struct EventFdTrigger(EventFd);

#[cfg(target_os = "linux")]
impl Trigger for EventFdTrigger {
    type E = io::Error;

    fn trigger(&self) -> io::Result<()> {
        self.write(1)
    }
}

#[cfg(target_os = "linux")]
impl Deref for EventFdTrigger {
    type Target = EventFd;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(target_os = "linux")]
impl EventFdTrigger {
    /// Clone an `EventFdTrigger`.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(EventFdTrigger((**self).try_clone()?))
    }

    /// Create an `EventFdTrigger`.
    pub fn new(evt: EventFd) -> Self {
        Self(evt)
    }

    /// Get the associated event fd out of an `EventFdTrigger`.
    pub fn get_event(&self) -> EventFd {
        self.0.try_clone().unwrap()
    }
}
