// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/transport/mmio.rs and
// src/vmm/src/devices/virtio/device.rs (trimmed: the register set, the device
// status state machine, the feature-page selection and the interrupt-status
// handling are upstream's; the `VirtioDevice` trait is this port's own,
// because there is no event manager, no metrics, no config-space plumbing and
// no userspace virtqueue processing here — every device behind this transport
// is vhost-backed, so the kernel owns the rings and the transport never walks
// a descriptor chain. Upstream's `Arc<Mutex<dyn VirtioDevice>>` becomes a
// plain `Box<dyn VirtioDevice>` for the same reason: the device is only ever
// touched through the transport, which the MMIO bus already serializes).

//! The virtio-mmio transport: the register set of virtio 1.0 §4.2.2.
//!
//! A driver finds one of these at a fixed guest-physical window (on `aarch64`
//! described by its device tree node, on `x86_64` by a `virtio_mmio.device=`
//! command-line entry) and programs it to bring the device behind it up:
//! features are negotiated through the two feature pages, each queue is
//! described by its register set, and the device is activated by the last
//! device-status write, at which point [`VirtioDevice::activate`] hands the
//! rings to the backend. Afterwards a queue notification — the driver writing
//! a queue index to `QueueNotify` — is forwarded to the device, and the device
//! raises its interrupt through the event fd the transport owns (the same one
//! the VMM registered as a KVM `irqfd`, so a kernel-space backend can signal
//! the guest without waking this process at all).

use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::pal::eventfd::EventFd;

use crate::devices::virtio::VirtioError;
use crate::devices::virtio::device_status;
use crate::vstate::bus::BusDevice;
use crate::vstate::memory::GuestMemoryMmap;

/// Interrupt status bit: the device has used-ring entries for the driver
/// (virtio-mmio `VIRTIO_MMIO_INT_VRING`).
pub const VIRTIO_MMIO_INT_VRING: u32 = 0x01;
/// Interrupt status bit: the device's configuration changed
/// (virtio-mmio `VIRTIO_MMIO_INT_CONFIG`).
pub const VIRTIO_MMIO_INT_CONFIG: u32 = 0x02;

/// Required by the virtio mmio device register layout at offset 0 from base.
const MMIO_MAGIC_VALUE: u32 = 0x7472_6976;

/// Current version specified by the mmio standard (legacy devices used 1 here).
const MMIO_VERSION: u32 = 2;

/// Vendor id. Upstream Firecracker reports 0 here (the virtio specification
/// gives the vendor its own register but no requirement on its value).
const VENDOR_ID: u32 = 0;

/// The transport-visible state of one virtqueue.
///
/// The driver writes `size`, the three ring addresses and `ready` through the
/// queue registers; `max_size` is the device's own limit, which the transport
/// reports in `QueueNumMax`. Nothing here is a ring this process interprets:
/// the backend reads the rings itself.
#[derive(Debug, Clone, Copy)]
pub struct QueueConfig {
    /// Largest ring this queue accepts (reported as `QueueNumMax`, and what
    /// the driver sizes its ring by).
    pub max_size: u32,
    /// Ring size the driver negotiated (`QueueNum`); 0 until it does.
    pub size: u32,
    /// Whether the driver marked the queue ready (`QueueReady`).
    pub ready: bool,
    /// Guest-physical address of the descriptor table (`QueueDescLow/High`).
    pub desc: u64,
    /// Guest-physical address of the available ring (`QueueDriverLow/High`).
    pub avail: u64,
    /// Guest-physical address of the used ring (`QueueDeviceLow/High`).
    pub used: u64,
}

impl QueueConfig {
    /// A queue the device exposes with room for `max_size` descriptors, not
    /// yet configured by the driver.
    pub fn new(max_size: u32) -> Self {
        Self {
            max_size,
            size: 0,
            ready: false,
            desc: 0,
            avail: 0,
            used: 0,
        }
    }

    /// Clears the driver's side of the queue, keeping the device's limit.
    pub fn reset(&mut self) {
        let max_size = self.max_size;
        *self = Self::new(max_size);
    }
}

/// A virtio device driven by the [`VirtioMmioTransport`] in front of it.
///
/// Every method is total with respect to guest input: the transport calls them
/// while decoding a register access, so an implementation reports what it
/// cannot honour through [`VirtioError`] rather than panicking.
pub trait VirtioDevice: Send {
    /// The device's virtio type id (`VIRTIO_ID_*`).
    fn device_type(&self) -> u32;

    /// Every feature this device and its transport together offer the driver,
    /// as a 64-bit feature set (bit `n` is feature `n`); the transport splits
    /// it into the two feature pages of the register set.
    ///
    /// The virtio specification requires a modern device to offer
    /// `VIRTIO_F_VERSION_1` (bit 32), and the vhost backends rely on it, so an
    /// implementation must include it.
    fn features(&self) -> u64;

    /// Records the features the driver acknowledged for feature page `page`
    /// (the value it wrote to `DriverFeatures`). Bits the device did not offer
    /// are an error.
    fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError>;

    /// The device's queues: one entry per queue index, in driver order.
    fn queues(&self) -> &[QueueConfig];

    /// The same queues, for the transport to write the driver's register
    /// values into.
    fn queues_mut(&mut self) -> &mut [QueueConfig];

    /// The driver is done initializing and the device must start: `mem` is the
    /// guest memory the rings live in and `irq` the event fd that signals the
    /// device's interrupt to the guest.
    fn activate(&mut self, mem: &GuestMemoryMmap, irq: &EventFd) -> Result<(), VirtioError>;

    /// The driver read `data.len()` bytes of the device configuration space at
    /// `offset` (the register window's `DeviceConfig` area, virtio 1.0
    /// §4.2.2.2).
    ///
    /// A device with no configuration space leaves `data` alone — the caller
    /// zero-fills it, exactly as an access to an address no device answers for
    /// reads. A device that has one must fill every byte it owns, because a
    /// guest reads it as the value of a field: a virtio-vsock guest takes its
    /// own CID from here (`struct virtio_vsock_config { __le64 guest_cid; }`),
    /// and a device that reports zero leaves its guest sending packets the
    /// host's vhost drops as misaddressed.
    fn read_config(&mut self, offset: u64, data: &mut [u8]);

    /// The driver wrote `data` to the device configuration space at `offset`.
    /// The transport forwards every write in the configuration window here; a
    /// device whose fields are not the driver's to write ignores the bytes.
    /// The default ignores them, for a device with no writable configuration.
    fn write_config(&mut self, _offset: u64, _data: &[u8]) {}

    /// The driver notified the device that queue `index` has new buffers.
    fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError>;

    /// The driver reset the device: return to the state before activation.
    fn reset(&mut self);
}

/// The virtio-mmio transport of one device.
pub struct VirtioMmioTransport {
    /// The device this transport exposes.
    device: Box<dyn VirtioDevice>,
    /// The feature page the driver last selected for reads
    /// (`DeviceFeaturesSel`).
    features_select: u32,
    /// The feature page the driver last selected for writes
    /// (`DriverFeaturesSel`).
    acked_features_select: u32,
    /// The queue the driver last selected (`QueueSel`).
    queue_select: u32,
    /// The device status field (virtio 1.0 §2.1).
    device_status: u32,
    /// Monotonic configuration generation (`ConfigGeneration`).
    config_generation: u32,
    /// The interrupt status (`InterruptStatus`). The transport always reports
    /// `VIRTIO_MMIO_INT_VRING`; a device ORs `VIRTIO_MMIO_INT_CONFIG` into this
    /// value on a configuration change (through
    /// [`VirtioMmioTransport::interrupt_status_handle`]), and the guest's
    /// interrupt-ack write clears the bits it acknowledges.
    interrupt_status: Arc<AtomicU32>,
    /// Guest memory, handed to the device on activation so it can translate
    /// the rings the driver programmed.
    mem: GuestMemoryMmap,
    /// The device's interrupt: an event fd the VMM registered as a KVM
    /// `irqfd` for the device's GSI, so writing to it injects the interrupt
    /// into the guest. The transport owns it and passes it to the device on
    /// activation (a kernel-space backend signals the guest through it
    /// directly).
    irq: EventFd,
}

impl VirtioMmioTransport {
    /// Builds the transport of `device`. `mem` is the guest memory its rings
    /// live in and `irq` its interrupt — already registered with the
    /// hypervisor for the device's GSI.
    pub fn new(device: Box<dyn VirtioDevice>, mem: GuestMemoryMmap, irq: EventFd) -> Self {
        Self {
            device,
            features_select: 0,
            acked_features_select: 0,
            queue_select: 0,
            device_status: device_status::INIT,
            config_generation: 0,
            interrupt_status: Arc::new(AtomicU32::new(0)),
            mem,
            irq,
        }
    }

    /// The feature page the driver asked for, or 0 for a page this device does
    /// not have (the specification: a device with fewer than two feature pages
    /// reads as 0 beyond them).
    fn features_by_page(&self, page: u32) -> u32 {
        match page {
            0 => self.device.features() as u32,
            1 => (self.device.features() >> 32) as u32,
            _ => {
                eprintln!("warning: virtio-mmio: features for invalid page {page} read as 0");
                0
            }
        }
    }

    /// Runs `f` on the queue the driver selected, or returns `default` when
    /// the selection is out of range (the device has fewer queues).
    fn with_queue<T>(&self, default: T, f: impl FnOnce(&QueueConfig) -> T) -> T {
        match <usize as TryFrom<u32>>::try_from(self.queue_select) {
            Ok(select) => self.device.queues().get(select).map_or(default, f),
            Err(_) => default,
        }
    }

    /// Whether the device status has every bit of `set` and none of `clr`.
    fn check_device_status(&self, set: u32, clr: u32) -> bool {
        self.device_status & (set | clr) == set
    }

    /// Applies `f` to the selected queue's driver-written fields.
    ///
    /// The specification allows the queue registers to be written only after
    /// the driver accepted the features and before it activated the device;
    /// the same holds for the ring addresses, so anything else is dropped with
    /// a warning rather than corrupting a live ring.
    fn update_queue_field(&mut self, f: impl FnOnce(&mut QueueConfig)) {
        if !self.check_device_status(
            device_status::FEATURES_OK,
            device_status::DRIVER_OK | device_status::FAILED,
        ) {
            eprintln!(
                "warning: virtio-mmio: queue register written in invalid device state {:#x}",
                self.device_status
            );
            return;
        }
        let Ok(select) = usize::try_from(self.queue_select) else {
            return;
        };
        let selected = self.queue_select;
        match self.device.queues_mut().get_mut(select) {
            Some(queue) => f(queue),
            None => {
                eprintln!(
                    "warning: virtio-mmio: queue register written for invalid queue {selected}"
                );
            }
        }
    }

    /// Records the driver's `QueueNum` write, rejecting a size this device
    /// cannot honour.
    ///
    /// The kernel's vhost ioctl (`VHOST_SET_VRING_NUM`) enforces only "non-zero,
    /// ≤ 0xffff, power of two", not this device's `QueueNumMax`, so a hostile
    /// driver could hand the backend a ring that is legal for vhost but larger
    /// than the device advertises — and the VMM would learn only from the
    /// kernel's refusal at activation. Rejecting here marks the device failed
    /// on its own, so the failure is explicit rather than deferred to the
    /// backend.
    fn set_queue_size(&mut self, value: u32) {
        if !self.check_device_status(
            device_status::FEATURES_OK,
            device_status::DRIVER_OK | device_status::FAILED,
        ) {
            eprintln!(
                "warning: virtio-mmio: queue register written in invalid device state {:#x}",
                self.device_status
            );
            return;
        }
        let Ok(select) = usize::try_from(self.queue_select) else {
            return;
        };
        let Some(queue) = self.device.queues_mut().get_mut(select) else {
            eprintln!(
                "warning: virtio-mmio: queue register written for invalid queue {}",
                self.queue_select
            );
            return;
        };
        let size = value & 0xffff;
        if size == 0 || size > queue.max_size || !size.is_power_of_two() {
            eprintln!(
                "warning: virtio-mmio: QueueNum {size:#x} is invalid: it must be a non-zero \
                 power of two no larger than {} (QueueNumMax)",
                queue.max_size
            );
            self.device_status |= device_status::FAILED;
            return;
        }
        queue.size = size;
    }

    /// The low half of a 64-bit ring address (`Queue*Low`).
    fn set_address_low(address: &mut u64, value: u32) {
        *address = (*address & 0xffff_ffff_0000_0000) | u64::from(value);
    }

    /// The high half of a 64-bit ring address (`Queue*High`).
    fn set_address_high(address: &mut u64, value: u32) {
        *address = (*address & 0x0000_0000_ffff_ffff) | (u64::from(value) << 32);
    }

    /// The interrupt status the driver reads in its interrupt handler.
    ///
    /// The status is reported as "the used ring has entries" unconditionally,
    /// because the device's event thread raises the interrupt through this
    /// transport's interrupt event fd without telling the transport which queue
    /// it was for. Reporting 0 instead would be read as a spurious interrupt by
    /// the guest's `virtio_mmio` driver, which would then never service the used
    /// ring — and 0x01 makes that driver look at every queue, which is exactly
    /// what a backend that cannot report a per-queue status needs. (Firecracker
    /// does the same for its vhost-user devices.) On top of that, the bits a
    /// device set in the shared status (`VIRTIO_MMIO_INT_CONFIG` after a
    /// configuration change) are reported until the driver acknowledges them.
    fn interrupt_status(&self) -> u32 {
        VIRTIO_MMIO_INT_VRING | self.interrupt_status.load(Ordering::SeqCst)
    }

    /// The shared interrupt status a device ORs its interrupt bits into.
    pub fn interrupt_status_handle(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.interrupt_status)
    }

    /// Clears the transport's registers and resets the device, as a driver
    /// reset (`Status` written as 0) requires.
    fn reset(&mut self) {
        self.device.reset();
        self.features_select = 0;
        self.acked_features_select = 0;
        self.queue_select = 0;
        self.device_status = device_status::INIT;
        // The interrupt event fd is deliberately left alone (it keeps whatever
        // the backend signalled; KVM's irqfd drains it), and
        // `config_generation` stays monotonic as the specification requires.
    }

    /// Update device status according to the state machine defined by VirtIO Spec 1.0.
    /// Please refer to VirtIO Spec 1.0, section 2.1.1 and 3.1.1.
    ///
    /// The driver MUST update device status, setting bits to indicate the completed steps
    /// of the driver initialization sequence specified in 3.1. The driver MUST NOT clear
    /// a device status bit. If the driver sets the FAILED bit, the driver MUST later reset
    /// the device before attempting to re-initialize.
    fn set_device_status(&mut self, status: u32) {
        use device_status::*;

        const VALID_TRANSITIONS: &[(u32, u32)] = &[
            (INIT, ACKNOWLEDGE),
            (ACKNOWLEDGE, ACKNOWLEDGE | DRIVER),
            (ACKNOWLEDGE | DRIVER, ACKNOWLEDGE | DRIVER | FEATURES_OK),
            (
                ACKNOWLEDGE | DRIVER | FEATURES_OK,
                ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK,
            ),
        ];

        if (status & FAILED) != 0 {
            self.device_status |= FAILED;
        } else if status == INIT {
            if self.device_status != INIT {
                self.reset();
            }
        } else if VALID_TRANSITIONS
            .iter()
            .any(|&(from, to)| self.device_status == from && status == to)
        {
            self.device_status = status;

            // Activate the device when transitioning to DRIVER_OK.
            if status == (ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK)
                && let Err(err) = self.device.activate(&self.mem, &self.irq)
            {
                eprintln!("warning: virtio-mmio: failed to activate the device: {err}");
                self.device_status |= FAILED;
            }
        } else {
            eprintln!(
                "warning: virtio-mmio: invalid driver status transition {:#x} -> {:#x}",
                self.device_status, status
            );
        }
    }
}

/// Reads a 4-byte little-endian register value, or `None` when the access is
/// not a 32-bit one (the transport's register window is 32 bits wide, as the
/// specification and the guest's `readl`/`writel` accesses assume).
fn read_le_u32(data: &[u8]) -> Option<u32> {
    let bytes = <[u8; 4]>::try_from(data).ok()?;
    Some(u32::from_le_bytes(bytes))
}

impl BusDevice for VirtioMmioTransport {
    fn read(&mut self, base: u64, offset: u64, data: &mut [u8]) {
        match offset {
            0x00..=0xff => {
                if read_le_u32(data).is_none() {
                    eprintln!(
                        "warning: invalid virtio mmio read: {base:#x}:{offset:#x}:{}",
                        data.len()
                    );
                    return;
                }
                let value = match offset {
                    0x00 => MMIO_MAGIC_VALUE,
                    0x04 => MMIO_VERSION,
                    0x08 => self.device.device_type(),
                    0x0c => VENDOR_ID,
                    0x10 => self.features_by_page(self.features_select),
                    0x34 => self.with_queue(0, |queue| queue.max_size),
                    0x44 => self.with_queue(0, |queue| u32::from(queue.ready)),
                    0x60 => self.interrupt_status(),
                    0x70 => self.device_status,
                    0xfc => self.config_generation,
                    _ => {
                        eprintln!("warning: unknown virtio mmio register read: {offset:#x}");
                        return;
                    }
                };
                data.copy_from_slice(&value.to_le_bytes());
            }
            0x100..=0xfff => self.device.read_config(offset - 0x100, data),
            _ => {
                eprintln!(
                    "warning: invalid virtio mmio read: {base:#x}:{offset:#x}:{}",
                    data.len()
                );
            }
        }
    }

    fn write(&mut self, base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        match offset {
            0x00..=0xff => {
                let Some(value) = read_le_u32(data) else {
                    eprintln!(
                        "warning: invalid virtio mmio write: {base:#x}:{offset:#x}:{}",
                        data.len()
                    );
                    return None;
                };
                match offset {
                    0x14 => self.features_select = value,
                    0x20 => {
                        if self.check_device_status(
                            device_status::DRIVER,
                            device_status::FEATURES_OK
                                | device_status::FAILED
                                | device_status::DEVICE_NEEDS_RESET,
                        ) {
                            let page = self.acked_features_select;
                            if let Err(err) = self.device.ack_features_by_page(page, value) {
                                eprintln!(
                                    "warning: virtio-mmio: failed to acknowledge feature page \
                                     {page} ({value:#x}): {err}"
                                );
                                self.device_status |= device_status::FAILED;
                            }
                        } else {
                            eprintln!(
                                "warning: virtio-mmio: features acknowledged in invalid device \
                                 state {:#x}",
                                self.device_status
                            );
                        }
                    }
                    0x24 => self.acked_features_select = value,
                    0x30 => self.queue_select = value,
                    0x38 => self.set_queue_size(value),
                    0x44 => self.update_queue_field(|queue| queue.ready = value == 1),
                    0x50 => {
                        if let Err(err) = self.device.notify_queue(value) {
                            eprintln!(
                                "warning: virtio-mmio: queue {value} notification failed: {err}"
                            );
                        }
                    }
                    0x64 => {
                        // Interrupt acknowledgement: the driver clears the bits
                        // it has handled. VRING is not stored, so only the
                        // device-set bits (CONFIG) are affected.
                        self.interrupt_status.fetch_and(!value, Ordering::SeqCst);
                    }
                    0x70 => self.set_device_status(value),
                    0x80 => self
                        .update_queue_field(|queue| Self::set_address_low(&mut queue.desc, value)),
                    0x84 => self
                        .update_queue_field(|queue| Self::set_address_high(&mut queue.desc, value)),
                    0x90 => self
                        .update_queue_field(|queue| Self::set_address_low(&mut queue.avail, value)),
                    0x94 => self.update_queue_field(|queue| {
                        Self::set_address_high(&mut queue.avail, value)
                    }),
                    0xa0 => self
                        .update_queue_field(|queue| Self::set_address_low(&mut queue.used, value)),
                    0xa4 => self
                        .update_queue_field(|queue| Self::set_address_high(&mut queue.used, value)),
                    _ => {
                        eprintln!("warning: unknown virtio mmio register write: {offset:#x}");
                    }
                }
            }
            // The device configuration space, forwarded to the device.
            0x100..=0xfff => self.device.write_config(offset - 0x100, data),
            _ => {
                eprintln!(
                    "warning: invalid virtio mmio write: {base:#x}:{offset:#x}:{}",
                    data.len()
                );
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::vstate::memory::tests::single_region_mem;

    /// What the transport asked of the mock device.
    #[derive(Debug, Default)]
    struct MockState {
        acked: u64,
        activated: usize,
        notified: Vec<u32>,
        resets: usize,
        config_written: Vec<(u64, Vec<u8>)>,
    }

    /// A device that records what the transport asks of it, so the register
    /// decode and the status state machine can be tested without a hypervisor
    /// or a backend.
    #[derive(Debug)]
    struct MockDevice {
        features: u64,
        queues: Vec<QueueConfig>,
        /// When set, feature acknowledgements are refused, as a device whose
        /// driver asked for a feature it never offered would refuse them.
        refuse_features: bool,
        state: Arc<Mutex<MockState>>,
    }

    impl VirtioDevice for MockDevice {
        fn device_type(&self) -> u32 {
            19
        }

        fn features(&self) -> u64 {
            self.features
        }

        fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError> {
            let offered = match page {
                0 => self.features as u32,
                1 => (self.features >> 32) as u32,
                _ => 0,
            };
            if self.refuse_features || value & !offered != 0 {
                return Err(VirtioError::UnsupportedFeatures {
                    flags: u64::from(value) << (32 * page),
                });
            }
            let mut state = self.state.lock().expect("mock state");
            let mask = 0xffff_ffff_u64 << (32 * page);
            state.acked = (state.acked & !mask) | (u64::from(value) << (32 * page));
            Ok(())
        }

        fn queues(&self) -> &[QueueConfig] {
            &self.queues
        }

        fn queues_mut(&mut self) -> &mut [QueueConfig] {
            &mut self.queues
        }

        fn read_config(&mut self, offset: u64, data: &mut [u8]) {
            for (index, byte) in data.iter_mut().enumerate() {
                *byte = u8::try_from(offset).unwrap_or(0).wrapping_add(index as u8);
            }
        }

        fn write_config(&mut self, offset: u64, data: &[u8]) {
            self.state
                .lock()
                .expect("mock state")
                .config_written
                .push((offset, data.to_vec()));
        }

        fn activate(&mut self, _mem: &GuestMemoryMmap, _irq: &EventFd) -> Result<(), VirtioError> {
            self.state.lock().expect("mock state").activated += 1;
            Ok(())
        }

        fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError> {
            self.state.lock().expect("mock state").notified.push(index);
            Ok(())
        }

        fn reset(&mut self) {
            let mut state = self.state.lock().expect("mock state");
            state.resets += 1;
            state.acked = 0;
            drop(state);
            for queue in &mut self.queues {
                queue.reset();
            }
        }
    }

    /// A transport over a mock device with `queues` queues, plus the mock's
    /// record of what the transport asked it to do.
    fn mock_transport(
        features: u64,
        queues: usize,
    ) -> (VirtioMmioTransport, Arc<Mutex<MockState>>) {
        let state = Arc::new(Mutex::new(MockState::default()));
        let device = MockDevice {
            features,
            queues: (0..queues)
                .map(|_| QueueConfig::new(256))
                .collect::<Vec<_>>(),
            refuse_features: false,
            state: Arc::clone(&state),
        };
        let transport = VirtioMmioTransport::new(
            Box::new(device),
            single_region_mem(0x1000),
            EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).expect("event fd"),
        );
        (transport, state)
    }

    /// Reads one 32-bit register.
    fn read_u32(transport: &mut VirtioMmioTransport, offset: u64) -> u32 {
        let mut data = [0u8; 4];
        transport.read(0, offset, &mut data);
        u32::from_le_bytes(data)
    }

    /// Writes one 32-bit register.
    fn write_u32(transport: &mut VirtioMmioTransport, offset: u64, value: u32) {
        transport.write(0, offset, &value.to_le_bytes());
    }

    #[test]
    fn config_writes_reach_the_device_and_config_interrupt_is_raised_and_acked() {
        let (mut transport, state) = mock_transport(0, 1);

        // A driver write into the configuration window (offset 0x100 + 4).
        transport.write(0, 0x104, &[1, 2, 3, 4]);
        assert_eq!(
            state.lock().expect("mock state").config_written,
            vec![(4, vec![1, 2, 3, 4])]
        );

        // Nothing pending: only the VRING bit is reported.
        assert_eq!(read_u32(&mut transport, 0x60), VIRTIO_MMIO_INT_VRING);

        // A device signals a configuration change through the shared status.
        transport
            .interrupt_status_handle()
            .fetch_or(VIRTIO_MMIO_INT_CONFIG, Ordering::SeqCst);
        assert_eq!(
            read_u32(&mut transport, 0x60),
            VIRTIO_MMIO_INT_VRING | VIRTIO_MMIO_INT_CONFIG
        );

        // The driver acknowledges CONFIG; the bit clears and VRING remains.
        write_u32(&mut transport, 0x64, VIRTIO_MMIO_INT_CONFIG);
        assert_eq!(read_u32(&mut transport, 0x60), VIRTIO_MMIO_INT_VRING);
    }

    /// Drives the status state machine to `DRIVER`, the point at which the
    /// specification lets the driver acknowledge the device's features.
    fn acknowledge_driver(transport: &mut VirtioMmioTransport) {
        use device_status::*;
        write_u32(transport, 0x70, ACKNOWLEDGE);
        write_u32(transport, 0x70, ACKNOWLEDGE | DRIVER);
    }

    /// Drives the status state machine up to (but not including) `DRIVER_OK`.
    fn negotiate(transport: &mut VirtioMmioTransport) {
        use device_status::*;
        acknowledge_driver(transport);
        write_u32(transport, 0x70, ACKNOWLEDGE | DRIVER | FEATURES_OK);
    }

    /// The `DRIVER_OK` status value.
    fn driver_ok() -> u32 {
        use device_status::*;
        ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK
    }

    #[test]
    fn identity_registers_are_the_virtio_mmio_ones() {
        let (mut transport, _state) = mock_transport(1 << 32, 3);
        assert_eq!(read_u32(&mut transport, 0x00), 0x7472_6976);
        assert_eq!(read_u32(&mut transport, 0x04), 2);
        assert_eq!(read_u32(&mut transport, 0x08), 19);
        assert_eq!(read_u32(&mut transport, 0x0c), 0);
        assert_eq!(read_u32(&mut transport, 0x70), 0);
        assert_eq!(read_u32(&mut transport, 0xfc), 0);
    }

    /// A read of the device configuration space reaches the device with the
    /// offset it asked for (this is how a guest learns its own vsock CID).
    #[test]
    fn the_configuration_space_is_read_from_the_device() {
        let (mut transport, _state) = mock_transport(1 << 32, 3);
        let mut data = [0u8; 8];
        transport.read(0, 0x100, &mut data);
        assert_eq!(data, [0, 1, 2, 3, 4, 5, 6, 7]);
        let mut data = [0u8; 4];
        transport.read(0, 0x110, &mut data);
        assert_eq!(data, [0x10, 0x11, 0x12, 0x13]);
    }

    #[test]
    fn features_are_read_by_page() {
        let (mut transport, _state) = mock_transport(0x0000_0001_8000_0001, 3);
        assert_eq!(read_u32(&mut transport, 0x10), 0x8000_0001);
        write_u32(&mut transport, 0x14, 1);
        assert_eq!(read_u32(&mut transport, 0x10), 0x0000_0001);
        // A page the device does not have reads as no features.
        write_u32(&mut transport, 0x14, 7);
        assert_eq!(read_u32(&mut transport, 0x10), 0);
    }

    #[test]
    fn driver_features_are_acked_per_page() {
        let (mut transport, state) = mock_transport(0x0000_0001_0000_0003, 3);
        acknowledge_driver(&mut transport);
        write_u32(&mut transport, 0x24, 0);
        write_u32(&mut transport, 0x20, 0x3);
        write_u32(&mut transport, 0x24, 1);
        write_u32(&mut transport, 0x20, 0x1);
        negotiate(&mut transport);
        assert_eq!(state.lock().expect("state").acked, 0x0000_0001_0000_0003);
        assert_eq!(read_u32(&mut transport, 0x70) & device_status::FAILED, 0);

        // A driver asking for a feature that was never offered is refused, and
        // the device is marked failed rather than driven with a feature set
        // nobody agreed on.
        let (mut transport, _state) = mock_transport(0x0000_0001_0000_0001, 3);
        acknowledge_driver(&mut transport);
        write_u32(&mut transport, 0x20, 0x2);
        assert_eq!(
            read_u32(&mut transport, 0x70) & device_status::FAILED,
            device_status::FAILED
        );
    }

    #[test]
    fn queue_registers_round_trip() {
        let (mut transport, _state) = mock_transport(1 << 32, 3);
        negotiate(&mut transport);
        write_u32(&mut transport, 0x30, 1);
        assert_eq!(read_u32(&mut transport, 0x34), 256);
        write_u32(&mut transport, 0x38, 128);
        write_u32(&mut transport, 0x80, 0x1234_5000);
        write_u32(&mut transport, 0x84, 0x1);
        write_u32(&mut transport, 0x90, 0x2000);
        write_u32(&mut transport, 0x94, 0);
        write_u32(&mut transport, 0xa0, 0x3000);
        write_u32(&mut transport, 0xa4, 0);
        write_u32(&mut transport, 0x44, 1);
        assert_eq!(read_u32(&mut transport, 0x44), 1);
        let queue = transport.device.queues()[1];
        assert_eq!(queue.size, 128);
        assert!(!transport.device.queues()[0].ready);
        assert_eq!(queue.desc, 0x1_1234_5000);
        assert_eq!(queue.avail, 0x2000);
        assert_eq!(queue.used, 0x3000);

        // A queue index the device does not have reads as a queue of no size.
        write_u32(&mut transport, 0x30, 9);
        assert_eq!(read_u32(&mut transport, 0x34), 0);
        assert_eq!(read_u32(&mut transport, 0x44), 0);
    }

    /// `QueueNum` must be a non-zero power of two no larger than the size the
    /// device advertises; anything else fails the device on its own rather
    /// than being handed to the backend for the kernel to refuse.
    #[test]
    fn queue_num_is_validated_at_write_time() {
        use device_status::FAILED;

        // A valid power of two within the advertised maximum is recorded.
        let (mut transport, _state) = mock_transport(1 << 32, 1);
        negotiate(&mut transport);
        write_u32(&mut transport, 0x38, 128);
        assert_eq!(transport.device.queues()[0].size, 128);
        assert_eq!(read_u32(&mut transport, 0x70) & FAILED, 0);

        // Zero, a non-power-of-two, an oversized power of two, and the
        // largest legal u16 all fail the device instead of reaching the
        // backend, and leave the queue unconfigured.
        for bad in [0u32, 3, 512, 0xffff] {
            let (mut transport, _state) = mock_transport(1 << 32, 1);
            negotiate(&mut transport);
            write_u32(&mut transport, 0x38, bad);
            assert_eq!(
                read_u32(&mut transport, 0x70) & FAILED,
                FAILED,
                "QueueNum {bad} must fail the device"
            );
            assert_eq!(transport.device.queues()[0].size, 0);
        }
    }

    #[test]
    fn driver_ok_activates_the_device() {
        let (mut transport, state) = mock_transport(1 << 32, 3);
        negotiate(&mut transport);
        write_u32(&mut transport, 0x30, 0);
        write_u32(&mut transport, 0x38, 64);
        write_u32(&mut transport, 0x44, 1);
        assert_eq!(state.lock().expect("state").activated, 0);
        write_u32(&mut transport, 0x70, driver_ok());
        assert_eq!(state.lock().expect("state").activated, 1);
        assert_eq!(read_u32(&mut transport, 0x70) & device_status::FAILED, 0);
    }

    #[test]
    fn a_failed_activation_marks_the_device_failed() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let device = MockDevice {
            features: 1 << 32,
            queues: vec![QueueConfig::new(256)],
            refuse_features: false,
            state: Arc::clone(&state),
        };
        // A device that cannot activate: `activate` reports the failure the
        // same way a backend that cannot be wired does.
        struct FailingDevice(Box<MockDevice>);
        impl VirtioDevice for FailingDevice {
            fn device_type(&self) -> u32 {
                self.0.device_type()
            }
            fn features(&self) -> u64 {
                self.0.features()
            }
            fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError> {
                self.0.ack_features_by_page(page, value)
            }
            fn queues(&self) -> &[QueueConfig] {
                self.0.queues()
            }
            fn queues_mut(&mut self) -> &mut [QueueConfig] {
                self.0.queues_mut()
            }
            fn read_config(&mut self, offset: u64, data: &mut [u8]) {
                self.0.read_config(offset, data)
            }
            fn activate(
                &mut self,
                _mem: &GuestMemoryMmap,
                _irq: &EventFd,
            ) -> Result<(), VirtioError> {
                Err(VirtioError::NoSuchQueue(0))
            }
            fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError> {
                self.0.notify_queue(index)
            }
            fn reset(&mut self) {
                self.0.reset();
            }
        }
        let mut transport = VirtioMmioTransport::new(
            Box::new(FailingDevice(Box::new(device))),
            single_region_mem(0x1000),
            EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).expect("event fd"),
        );
        negotiate(&mut transport);
        write_u32(&mut transport, 0x70, driver_ok());
        assert_eq!(
            read_u32(&mut transport, 0x70) & device_status::FAILED,
            device_status::FAILED
        );
    }

    #[test]
    fn queue_notifications_reach_the_device() {
        let (mut transport, state) = mock_transport(1 << 32, 3);
        write_u32(&mut transport, 0x50, 1);
        write_u32(&mut transport, 0x50, 2);
        assert_eq!(state.lock().expect("state").notified, vec![1, 2]);
    }

    #[test]
    fn a_reset_returns_the_device_and_the_registers_to_init() {
        let (mut transport, state) = mock_transport(1 << 32, 3);
        negotiate(&mut transport);
        write_u32(&mut transport, 0x30, 0);
        write_u32(&mut transport, 0x38, 64);
        write_u32(&mut transport, 0x44, 1);
        write_u32(&mut transport, 0x24, 1);
        write_u32(&mut transport, 0x20, 1);
        write_u32(&mut transport, 0x70, driver_ok());
        write_u32(&mut transport, 0x70, 0);
        assert_eq!(state.lock().expect("state").resets, 1);
        assert_eq!(state.lock().expect("state").acked, 0);
        assert_eq!(read_u32(&mut transport, 0x70), 0);
        assert_eq!(read_u32(&mut transport, 0x44), 0);
        assert_eq!(read_u32(&mut transport, 0x34), 256);
        assert!(transport.device.queues().iter().all(|queue| !queue.ready));
    }

    #[test]
    fn queue_registers_written_out_of_order_are_dropped() {
        // Before FEATURES_OK the driver must not program a queue; doing so must
        // not reach the device.
        let (mut transport, _state) = mock_transport(1 << 32, 3);
        write_u32(&mut transport, 0x38, 64);
        write_u32(&mut transport, 0x44, 1);
        assert_eq!(transport.device.queues()[0].size, 0);
        assert!(!transport.device.queues()[0].ready);

        // After DRIVER_OK the rings are live and must not move either.
        negotiate(&mut transport);
        write_u32(&mut transport, 0x70, driver_ok());
        write_u32(&mut transport, 0x38, 8);
        assert_eq!(transport.device.queues()[0].size, 0);
    }

    #[test]
    fn no_register_access_panics() {
        // Every offset and length a guest can produce must be decoded without
        // panicking: the register window is guest-controlled data.
        let (mut transport, _state) = mock_transport(1 << 32, 3);
        for offset in 0..0x1000u64 {
            for len in [0usize, 3, 4, 8] {
                let mut read = vec![0u8; len];
                transport.read(0, offset, &mut read);
                let write = vec![0xa5u8; len];
                transport.write(0, offset, &write);
            }
        }
        // And every register offset with a range of 32-bit values, including
        // the ones that drive the state machine.
        for offset in (0..0x100u64).step_by(4) {
            for value in [0u32, 1, 2, 4, 8, 64, 128, 0x8000_0000, u32::MAX] {
                let mut read = [0u8; 4];
                transport.read(0, offset, &mut read);
                transport.write(0, offset, &value.to_le_bytes());
            }
        }
    }
}
