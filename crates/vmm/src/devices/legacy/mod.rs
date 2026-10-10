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
mod i8042;
pub mod serial;

use std::io;
use std::ops::Deref;

use vm_superio::Trigger;
use vmm_sys_util::eventfd::EventFd;

pub use self::i8042::{I8042Device, I8042Error as I8042DeviceError};
pub use self::serial::{SerialDevice, SerialEventsWrapper, SerialOut, SerialOutInner, SerialWrapper};

/// Wrapper for implementing the trigger functionality for `EventFd`.
///
/// The trigger is used for handling events in the legacy devices.
#[derive(Debug)]
pub struct EventFdTrigger(EventFd);

impl Trigger for EventFdTrigger {
    type E = io::Error;

    fn trigger(&self) -> io::Result<()> {
        self.write(1)
    }
}

impl Deref for EventFdTrigger {
    type Target = EventFd;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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
