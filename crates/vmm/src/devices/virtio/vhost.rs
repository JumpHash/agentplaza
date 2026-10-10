// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: Firecracker's vsock device implements virtio-vsock in
// userspace (its own packet parser and a Unix-socket muxer), which is not what
// the plaza wants — the orchestrator listens on `AF_VSOCK` CID 2, so the guest
// must talk to the host kernel's vsock stack. This module is the binding to
// that stack: `/dev/vhost-vsock`, the ioctl sequence and the memory table that
// QEMU's `vhost_dev_init`/`vhost_virtqueue_start` perform, reimplemented from
// the kernel's `include/uapi/linux/vhost.h` and `drivers/vhost/vsock.c`. No
// code is copied from Firecracker or QEMU here (the `vhost` crate Firecracker
// uses for this interface is not a dependency of this tree), and the only
// sources used were those two kernel files plus the interface's own headers.

//! The host kernel's vhost-vsock backend.
//!
//! The device model in [`crate::devices::virtio::vsock`] hands this module the
//! rings the driver programmed; the kernel's `vhost_vsock` driver then drives
//! them: it reads the guest's packets off the transmit ring, delivers them to
//! the host's vsock stack (where an `AF_VSOCK` listener can accept them), and
//! puts replies on the receive ring, signalling the guest through the event fd
//! this backend was given as the queues' call fd.
//!
//! Measured on the development host (see `FACTS.md` next to this module):
//!
//! * this kernel's `vhost_vsock` has **two** queues, receive (0) and transmit
//!   (1) — the virtio-vsock event queue is device-side only now — so the third
//!   queue the driver sets up is deliberately left unwired, and its
//!   notifications are ignored;
//! * `VHOST_SET_VRING_ADDR` takes **host userspace addresses** (the kernel
//!   reads the rings with `copy_from_user` on the owner process's pointers),
//!   while the memory table maps guest-physical to userspace addresses for the
//!   descriptor buffers; both come from the same guest memory here;
//! * a device cannot be started (`VHOST_VSOCK_SET_RUNNING`) without a memory
//!   table, and the kernel refuses reserved CIDs (0, 1, 2) as well as CIDs
//!   another transport already claims.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_uint, c_ulong};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use vm_memory::GuestMemoryBackend;
use vmm_sys_util::eventfd::EventFd;
use vmm_sys_util::ioctl::{
    _IOC_NONE, _IOC_READ, _IOC_WRITE, ioctl, ioctl_expr, ioctl_with_mut_ref, ioctl_with_ref,
};

use crate::devices::virtio::mmio::QueueConfig;
use crate::devices::virtio::vsock::{MIN_GUEST_CID, VIRTIO_F_VERSION_1, VSOCK_FEATURES};
use crate::vstate::memory::{Address, GuestAddress, GuestMemoryMmap, GuestMemoryRegion};

/// The vhost character device that backs a vsock device.
const VHOST_VSOCK_PATH: &str = "/dev/vhost-vsock";

/// The ioctl type of the vhost interface (`linux/vhost.h`).
const VHOST_VIRTIO: c_uint = 0xAF;

/// `struct vhost_memory`'s encoding size: the fixed header
/// (`__u32 nregions; __u32 padding;`) of a structure whose region array is a
/// flexible array member. The kernel reads `nregions` trailing regions from the
/// same pointer (see [`VhostMemory`]).
const VHOST_MEMORY_HEADER_SIZE: c_uint = (2 * size_of::<u32>()) as c_uint;

/// `VHOST_GET_FEATURES = _IOR(VHOST_VIRTIO, 0x00, __u64)`.
const VHOST_GET_FEATURES: c_ulong =
    ioctl_expr(_IOC_READ, VHOST_VIRTIO, 0x00, size_of::<u64>() as c_uint);
/// `VHOST_SET_FEATURES = _IOW(VHOST_VIRTIO, 0x00, __u64)`.
const VHOST_SET_FEATURES: c_ulong =
    ioctl_expr(_IOC_WRITE, VHOST_VIRTIO, 0x00, size_of::<u64>() as c_uint);
/// `VHOST_SET_OWNER = _IO(VHOST_VIRTIO, 0x01)`.
const VHOST_SET_OWNER: c_ulong = ioctl_expr(_IOC_NONE, VHOST_VIRTIO, 0x01, 0);
/// `VHOST_SET_MEM_TABLE = _IOW(VHOST_VIRTIO, 0x03, struct vhost_memory)`.
const VHOST_SET_MEM_TABLE: c_ulong =
    ioctl_expr(_IOC_WRITE, VHOST_VIRTIO, 0x03, VHOST_MEMORY_HEADER_SIZE);
/// `VHOST_SET_VRING_NUM = _IOW(VHOST_VIRTIO, 0x10, struct vhost_vring_state)`.
const VHOST_SET_VRING_NUM: c_ulong = ioctl_expr(
    _IOC_WRITE,
    VHOST_VIRTIO,
    0x10,
    size_of::<VhostVringState>() as c_uint,
);
/// `VHOST_SET_VRING_ADDR = _IOW(VHOST_VIRTIO, 0x11, struct vhost_vring_addr)`.
const VHOST_SET_VRING_ADDR: c_ulong = ioctl_expr(
    _IOC_WRITE,
    VHOST_VIRTIO,
    0x11,
    size_of::<VhostVringAddr>() as c_uint,
);
/// `VHOST_SET_VRING_BASE = _IOW(VHOST_VIRTIO, 0x12, struct vhost_vring_state)`.
const VHOST_SET_VRING_BASE: c_ulong = ioctl_expr(
    _IOC_WRITE,
    VHOST_VIRTIO,
    0x12,
    size_of::<VhostVringState>() as c_uint,
);
/// `VHOST_SET_VRING_KICK = _IOW(VHOST_VIRTIO, 0x20, struct vhost_vring_file)`.
const VHOST_SET_VRING_KICK: c_ulong = ioctl_expr(
    _IOC_WRITE,
    VHOST_VIRTIO,
    0x20,
    size_of::<VhostVringFile>() as c_uint,
);
/// `VHOST_SET_VRING_CALL = _IOW(VHOST_VIRTIO, 0x21, struct vhost_vring_file)`.
const VHOST_SET_VRING_CALL: c_ulong = ioctl_expr(
    _IOC_WRITE,
    VHOST_VIRTIO,
    0x21,
    size_of::<VhostVringFile>() as c_uint,
);
/// `VHOST_VSOCK_SET_GUEST_CID = _IOW(VHOST_VIRTIO, 0x60, __u64)`.
const VHOST_VSOCK_SET_GUEST_CID: c_ulong =
    ioctl_expr(_IOC_WRITE, VHOST_VIRTIO, 0x60, size_of::<u64>() as c_uint);
/// `VHOST_VSOCK_SET_RUNNING = _IOW(VHOST_VIRTIO, 0x61, int)` — the argument is
/// a pointer to an `int`, not the value.
const VHOST_VSOCK_SET_RUNNING: c_ulong =
    ioctl_expr(_IOC_WRITE, VHOST_VIRTIO, 0x61, size_of::<c_int>() as c_uint);

/// `struct vhost_vring_state { __u32 index; __u32 num; }` (`linux/vhost.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VhostVringState {
    index: u32,
    num: u32,
}

/// `struct vhost_vring_addr` (`linux/vhost.h`). The three ring addresses are
/// **userspace** addresses in the owner process (the kernel reads the rings
/// with `copy_from_user`), which is why [`VhostVsock::start`] translates them.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VhostVringAddr {
    index: u32,
    flags: u32,
    desc_user_addr: u64,
    used_user_addr: u64,
    avail_user_addr: u64,
    log_guest_addr: u64,
}

/// `struct vhost_vring_file { __u32 index; __u32 fd; }` (`linux/vhost.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VhostVringFile {
    index: u32,
    fd: c_int,
}

/// `struct vhost_memory_region` (`linux/vhost.h`): one guest memory region, as
/// the guest-physical range it covers and the host address it lives at.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VhostMemoryRegion {
    guest_phys_addr: u64,
    memory_size: u64,
    userspace_addr: u64,
    flags_padding: u64,
}

/// How many guest memory regions this backend can describe. The x86_64 memory
/// map splits guest RAM around the 32-bit MMIO hole (and again around the
/// 64-bit one), so a VM has a handful of regions; more than this is reported as
/// an error rather than truncated.
const MAX_MEMORY_REGIONS: usize = 8;

/// `struct vhost_memory` (`linux/vhost.h`) with its trailing region array made
/// explicit: the kernel reads `nregions` entries from the same pointer, so a
/// fixed-capacity array is what it expects to find after the header.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VhostMemory {
    nregions: u32,
    padding: u32,
    regions: [VhostMemoryRegion; MAX_MEMORY_REGIONS],
}

/// Errors from the vhost interface.
#[derive(Debug, thiserror::Error)]
pub enum VhostError {
    /// Cannot open {path}: {source}
    #[error("cannot open {path}: {source}")]
    Open {
        /// Path of the vhost device that could not be opened.
        path: PathBuf,
        /// The error the open failed with.
        source: io::Error,
    },
    /// The vhost ioctl {name} failed: {source}
    #[error("the vhost ioctl {name} failed: {source}")]
    Ioctl {
        /// Name of the ioctl that failed.
        name: &'static str,
        /// The errno the kernel returned.
        source: io::Error,
    },
    /// An event fd of the backend could not be used: {0}
    #[error("an event fd of the backend could not be used: {0}")]
    EventFd(io::Error),
    /// The guest CID {0} is reserved: guest CIDs start at 3
    #[error(
        "the guest CID {0} is reserved: the hypervisor is 0, loopback 1 and the host 2, so guest \
         CIDs start at 3"
    )]
    ReservedGuestCid(u64),
    /// The vhost backend does not offer {0}, which this port needs
    #[error("the vhost backend does not offer {0}, which this port needs")]
    MissingFeature(&'static str),
    /// Guest memory has {regions} regions, more than the {MAX_MEMORY_REGIONS} this backend describes
    #[error(
        "guest memory has {regions} regions, more than the {MAX_MEMORY_REGIONS} this backend \
         describes"
    )]
    TooManyMemoryRegions {
        /// How many regions the guest memory has.
        regions: usize,
    },
    /// Queue {index} was not configured by the driver (size {size}, ready {ready})
    #[error("queue {index} was not configured by the driver (size {size}, ready {ready})")]
    QueueNotConfigured {
        /// Index of the queue.
        index: u32,
        /// Ring size the driver wrote.
        size: u32,
        /// Whether the driver marked the queue ready.
        ready: bool,
    },
    /// Queue {index}'s rings at {addr:#x} are not in guest memory
    #[error("queue {index}'s rings at {addr:#x} are not in guest memory")]
    RingAddress {
        /// Index of the queue whose address could not be translated.
        index: u32,
        /// The guest-physical address that is not in guest memory.
        addr: u64,
    },
    /// The driver acknowledged features the backend does not support: {flags:#x}
    #[error("the driver acknowledged features the backend does not support: {flags:#x}")]
    UnsupportedFeatures {
        /// The offending feature set.
        flags: u64,
    },
}

impl VhostError {
    /// Wraps an errno from the ioctl named `name`.
    fn ioctl(name: &'static str, source: io::Error) -> Self {
        Self::Ioctl { name, source }
    }
}

/// The `vhost_vsock` device of one VM.
///
/// Owning this struct owns the backend: the kernel keeps the device, its memory
/// table, its rings and its CID for as long as the file descriptor is open, and
/// tears them all down when it is closed — which is what makes dropping the
/// last reference of a VM's device stop the backend.
#[derive(Debug)]
pub struct VhostVsock {
    /// The open `/dev/vhost-vsock`.
    file: File,
    /// The features the backend offers, already restricted to the ones this
    /// port forwards to the driver.
    features: u64,
    /// The guest CID this backend claims.
    guest_cid: u64,
    /// One kick event fd per wired queue: notified by [`VhostVsock::kick`] when
    /// the driver tells the device that queue has buffers, and polled by the
    /// kernel's worker.
    kicks: Vec<(u32, EventFd)>,
}

impl VhostVsock {
    /// Opens the backend and claims `guest_cid` for it.
    ///
    /// This is the "realize" half of the setup (the device exists and knows
    /// which guest it belongs to) — the half a caller can still report as a
    /// plain error before the guest boots. The rings are wired later, by
    /// [`VhostVsock::start`], once the driver has programmed them.
    ///
    /// The guest CID is validated here *and* by the kernel: it must be at least
    /// [`MIN_GUEST_CID`], and the kernel refuses a CID that another transport
    /// (or another VM's backend) already claims.
    pub fn new(guest_cid: u64) -> Result<Self, VhostError> {
        if guest_cid < MIN_GUEST_CID {
            return Err(VhostError::ReservedGuestCid(guest_cid));
        }
        let file = File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(VHOST_VSOCK_PATH)
            .map_err(|source| VhostError::Open {
                path: PathBuf::from(VHOST_VSOCK_PATH),
                source,
            })?;

        ioctl0(&file, VHOST_SET_OWNER, "VHOST_SET_OWNER")?;

        let features = ioctl_get_features(&file)? & VSOCK_FEATURES;
        if features & VIRTIO_F_VERSION_1 == 0 {
            return Err(VhostError::MissingFeature("VIRTIO_F_VERSION_1"));
        }
        set_guest_cid(&file, guest_cid)?;

        Ok(Self {
            file,
            features,
            guest_cid,
            kicks: Vec::new(),
        })
    }

    /// The features this backend offers the driver.
    pub fn features(&self) -> u64 {
        self.features
    }

    /// The guest CID this backend claims.
    pub fn guest_cid(&self) -> u64 {
        self.guest_cid
    }

    /// Wires the queues the driver configured and starts the device.
    ///
    /// `queues` are the driver's queue descriptions, in queue order, and `irq`
    /// is the device's interrupt event fd — registered by the caller with the
    /// hypervisor as an `irqfd` for the device's interrupt line, so the kernel
    /// can raise the guest's interrupt itself whenever it has put something on
    /// the receive ring.
    ///
    /// Queues the driver did not mark ready are skipped, and so are queues past
    /// the end of the backend's own queue array (the kernel answers `ENOBUFS`
    /// for those, which is how that count is learned). Because the kernel
    /// checks the rings of *its* queues when the device is started, a queue the
    /// driver never programmed makes starting fail instead of silently running
    /// half a device.
    pub fn start(
        &mut self,
        mem: &GuestMemoryMmap,
        irq: &EventFd,
        features: u64,
        queues: &[QueueConfig],
    ) -> Result<(), VhostError> {
        if features & !self.features != 0 {
            return Err(VhostError::UnsupportedFeatures { flags: features });
        }
        ioctl_write(
            &self.file,
            VHOST_SET_FEATURES,
            &features,
            "VHOST_SET_FEATURES",
        )?;
        self.set_memory_table(mem)?;

        let mut kicks = Vec::new();
        for (position, queue) in queues.iter().enumerate() {
            let Ok(index) = u32::try_from(position) else {
                break;
            };
            if !queue.ready {
                continue;
            }
            if queue.size == 0 {
                return Err(VhostError::QueueNotConfigured {
                    index,
                    size: queue.size,
                    ready: queue.ready,
                });
            }
            let kick = EventFd::new(libc::EFD_NONBLOCK).map_err(VhostError::EventFd)?;
            if !self.set_vring_num(index, queue.size)? {
                // Past the backend's queue array: this queue belongs to the
                // driver alone, and there is nothing to wire.
                break;
            }
            self.set_vring_address(index, mem, queue)?;
            ioctl_write(
                &self.file,
                VHOST_SET_VRING_BASE,
                &VhostVringState { index, num: 0 },
                "VHOST_SET_VRING_BASE",
            )?;
            ioctl_write(
                &self.file,
                VHOST_SET_VRING_KICK,
                &VhostVringFile {
                    index,
                    fd: kick.as_raw_fd(),
                },
                "VHOST_SET_VRING_KICK",
            )?;
            ioctl_write(
                &self.file,
                VHOST_SET_VRING_CALL,
                &VhostVringFile {
                    index,
                    fd: irq.as_raw_fd(),
                },
                "VHOST_SET_VRING_CALL",
            )?;
            kicks.push((index, kick));
        }
        self.kicks = kicks;

        // The kernel validates the rings of its queues here and returns an
        // error — rather than a half-running device — when one was never
        // programmed.
        ioctl_write(
            &self.file,
            VHOST_VSOCK_SET_RUNNING,
            &1i32,
            "VHOST_VSOCK_SET_RUNNING",
        )?;
        Ok(())
    }

    /// Tells the backend that queue `index` has new buffers.
    ///
    /// A queue this backend does not own (see [`VhostVsock::start`]) is not an
    /// error: the guest's driver may notify it, and there is simply no worker
    /// behind it.
    pub fn kick(&self, index: u32) -> Result<(), VhostError> {
        match self.kicks.iter().find(|(wired, _)| *wired == index) {
            Some((_, kick)) => kick.write(1).map_err(VhostError::EventFd),
            None => Ok(()),
        }
    }

    /// Programs the ring size of queue `index`, answering whether the backend
    /// has such a queue at all.
    ///
    /// The kernel's ioctl layer answers `ENOBUFS` for an index past the end of
    /// its queue array — the only way this interface reports its queue count —
    /// so that errno is the "no such queue" answer rather than a failure.
    fn set_vring_num(&self, index: u32, num: u32) -> Result<bool, VhostError> {
        let state = VhostVringState { index, num };
        // SAFETY: `VHOST_SET_VRING_NUM` reads a `struct vhost_vring_state`,
        // which is exactly `state`'s type.
        let ret = unsafe { ioctl_with_ref(&self.file, VHOST_SET_VRING_NUM, &state) };
        if ret == 0 {
            return Ok(true);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ENOBUFS) {
            Ok(false)
        } else {
            Err(VhostError::ioctl("VHOST_SET_VRING_NUM", err))
        }
    }

    /// Translates queue `queue`'s rings from guest-physical to host userspace
    /// addresses and programs them.
    fn set_vring_address(
        &self,
        index: u32,
        mem: &GuestMemoryMmap,
        queue: &QueueConfig,
    ) -> Result<(), VhostError> {
        let [desc_user_addr, used_user_addr, avail_user_addr] =
            translate_rings(index, mem, queue)?;
        let address = VhostVringAddr {
            index,
            flags: 0,
            desc_user_addr,
            used_user_addr,
            avail_user_addr,
            log_guest_addr: 0,
        };
        ioctl_write(
            &self.file,
            VHOST_SET_VRING_ADDR,
            &address,
            "VHOST_SET_VRING_ADDR",
        )
    }

    /// Hands the kernel the guest memory the rings and the packet buffers live
    /// in: one `vhost_memory_region` per guest memory region, mapping its
    /// guest-physical range to where it actually is in this process.
    fn set_memory_table(&self, mem: &GuestMemoryMmap) -> Result<(), VhostError> {
        let mut table = VhostMemory {
            nregions: 0,
            padding: 0,
            regions: [VhostMemoryRegion::default(); MAX_MEMORY_REGIONS],
        };
        for region in mem.iter() {
            let index = table.nregions as usize;
            if index == MAX_MEMORY_REGIONS {
                return Err(VhostError::TooManyMemoryRegions {
                    regions: mem.iter().count(),
                });
            }
            table.regions[index] = VhostMemoryRegion {
                guest_phys_addr: region.start_addr().raw_value(),
                memory_size: region.len(),
                userspace_addr: region.as_ptr() as u64,
                flags_padding: 0,
            };
            table.nregions += 1;
        }
        ioctl_write(
            &self.file,
            VHOST_SET_MEM_TABLE,
            &table,
            "VHOST_SET_MEM_TABLE",
        )
    }
}

/// One of the three ring areas of a virtqueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RingKind {
    /// The descriptor table.
    Desc,
    /// The available ring.
    Avail,
    /// The used ring.
    Used,
}

/// The size in bytes of one ring area of a queue of `num` descriptors, exactly
/// as the kernel's `vhost_get_vring_size` computes it for the device that owns
/// the ring.
///
/// The available and used rings carry the two trailing event-index bytes
/// (`used_event`/`avail_event`) whenever `VIRTIO_RING_F_EVENT_IDX` is in play;
/// this port always offers that feature (see [`VSOCK_FEATURES`]), so those
/// bytes are always counted here. Sizing on the larger side can only reject a
/// ring that really is too close to the end of its region, never accept one
/// whose true extent crosses it.
fn ring_extent_size(kind: RingKind, num: u32) -> usize {
    let num = num as usize;
    match kind {
        RingKind::Desc => 16 * num,
        RingKind::Avail => 4 + 2 * num + 2,
        RingKind::Used => 4 + 8 * num + 2,
    }
}

/// Translates queue `queue`'s three rings from guest-physical addresses to the
/// host userspace addresses the kernel reads and writes, rejecting any ring
/// whose whole extent is not inside a single guest memory region.
///
/// `GuestMemoryMmap::get_host_address` only checks a ring's *first* byte; the
/// kernel then reads the descriptor table and the available ring and writes the
/// used ring with `copy_from_user`/`copy_to_user` at those addresses, trusting
/// them as this process's own memory (the memory table covers only the
/// descriptor buffers, not the rings). A ring whose start is valid but whose
/// extent runs past the end of a region would therefore make the kernel touch
/// this process's memory outside guest RAM. `get_slice` validates the whole
/// extent inside one region before the address is taken.
fn translate_rings(
    index: u32,
    mem: &GuestMemoryMmap,
    queue: &QueueConfig,
) -> Result<[u64; 3], VhostError> {
    let mut addresses = [0u64; 3];
    for (slot, (kind, addr)) in [
        (RingKind::Desc, queue.desc),
        (RingKind::Used, queue.used),
        (RingKind::Avail, queue.avail),
    ]
    .into_iter()
    .enumerate()
    {
        let size = ring_extent_size(kind, queue.size);
        mem.get_slice(GuestAddress(addr), size)
            .map_err(|_| VhostError::RingAddress { index, addr })?;
        let host = mem
            .get_host_address(GuestAddress(addr))
            .map_err(|_| VhostError::RingAddress { index, addr })?;
        addresses[slot] = host as u64;
    }
    Ok(addresses)
}

/// Runs a vhost ioctl that takes no argument.
fn ioctl0(file: &File, request: c_ulong, name: &'static str) -> Result<(), VhostError> {
    // SAFETY: `file` is an open /dev/vhost-vsock descriptor and `request` is
    // one of this module's request codes whose interface takes no argument, so
    // the kernel ignores the argument slot.
    let ret = unsafe { ioctl(file, request) };
    if ret < 0 {
        Err(VhostError::ioctl(name, io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

/// Runs a vhost ioctl that reads a value of type `T` from the caller.
fn ioctl_write<T>(
    file: &File,
    request: c_ulong,
    arg: &T,
    name: &'static str,
) -> Result<(), VhostError> {
    // SAFETY: `request` is the request code declared for `T` in this module
    // (from `linux/vhost.h`), so the kernel reads exactly `size_of::<T>()`
    // bytes from the pointer it is handed — the encoding in `request`'s size
    // field and the pointee's type are kept in step by the declarations above.
    let ret = unsafe { ioctl_with_ref(file, request, arg) };
    if ret < 0 {
        Err(VhostError::ioctl(name, io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

/// Reads the backend's feature set (`VHOST_GET_FEATURES`).
fn ioctl_get_features(file: &File) -> Result<u64, VhostError> {
    let mut features = 0u64;
    // SAFETY: `VHOST_GET_FEATURES` fills a `__u64` at the given pointer, which
    // is exactly `features`' type.
    let ret = unsafe { ioctl_with_mut_ref(file, VHOST_GET_FEATURES, &mut features) };
    if ret < 0 {
        Err(VhostError::ioctl(
            "VHOST_GET_FEATURES",
            io::Error::last_os_error(),
        ))
    } else {
        Ok(features)
    }
}

/// Claims `guest_cid` for the backend (`VHOST_VSOCK_SET_GUEST_CID`).
fn set_guest_cid(file: &File, guest_cid: u64) -> Result<(), VhostError> {
    ioctl_write(
        file,
        VHOST_VSOCK_SET_GUEST_CID,
        &guest_cid,
        "VHOST_VSOCK_SET_GUEST_CID",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::vstate::memory::mib_to_bytes;

    /// The request codes must be the kernel's, byte for byte: an ioctl number
    /// carries its argument's size in the encoding, and a mismatch is not a
    /// compile error — the kernel answers `ENOTTY` at run time, on a live VM.
    ///
    /// The expected values are the ones the host's own `linux/vhost.h` macros
    /// produce (printed by compiling a program against that header; the host
    /// kernel is 7.2.8-arch1-2).
    #[test]
    fn ioctl_request_codes_match_the_kernels() {
        assert_eq!(VHOST_GET_FEATURES, 0x8008_af00);
        assert_eq!(VHOST_SET_FEATURES, 0x4008_af00);
        assert_eq!(VHOST_SET_OWNER, 0x0000_af01);
        assert_eq!(VHOST_SET_MEM_TABLE, 0x4008_af03);
        assert_eq!(VHOST_SET_VRING_NUM, 0x4008_af10);
        assert_eq!(VHOST_SET_VRING_ADDR, 0x4028_af11);
        assert_eq!(VHOST_SET_VRING_BASE, 0x4008_af12);
        assert_eq!(VHOST_SET_VRING_KICK, 0x4008_af20);
        assert_eq!(VHOST_SET_VRING_CALL, 0x4008_af21);
        assert_eq!(VHOST_VSOCK_SET_GUEST_CID, 0x4008_af60);
        assert_eq!(VHOST_VSOCK_SET_RUNNING, 0x4004_af61);
    }

    /// The argument layouts must match the kernel's structures: these are the
    /// sizes the ioctl encodings above carry, and the kernel copies exactly
    /// that many bytes from (and to) the pointers this module passes it.
    #[test]
    fn ioctl_argument_layouts_match_the_kernels() {
        assert_eq!(size_of::<VhostVringState>(), 8);
        assert_eq!(size_of::<VhostVringFile>(), 8);
        assert_eq!(size_of::<VhostVringAddr>(), 40);
        // `struct vhost_memory`'s fixed part, which is what its ioctl number
        // encodes, is only the two `__u32`s in front of the region array.
        assert_eq!(VHOST_MEMORY_HEADER_SIZE, 8);
        assert_eq!(size_of::<VhostMemoryRegion>(), 32);
    }

    /// A CID the hypervisor stack reserves must be refused before the device is
    /// opened, so the error names the caller's mistake rather than an errno.
    #[test]
    fn reserved_guest_cids_are_refused() {
        for cid in 0..MIN_GUEST_CID {
            assert!(matches!(
                VhostVsock::new(cid),
                Err(VhostError::ReservedGuestCid(rejected)) if rejected == cid
            ));
        }
    }

    /// The feature set this port forwards must contain what a modern device
    /// needs and must not contain what the backend must not be told:
    /// `VIRTIO_F_ACCESS_PLATFORM` would make it expect an IOTLB (a vIOMMU) and
    /// `VHOST_F_LOG_ALL` a dirty log, and this VMM has neither.
    #[test]
    fn the_forwarded_features_exclude_the_iotlb_and_logging_bits() {
        assert_ne!(VSOCK_FEATURES & VIRTIO_F_VERSION_1, 0);
        assert_eq!(VSOCK_FEATURES & (1 << 33), 0, "VIRTIO_F_ACCESS_PLATFORM");
        assert_eq!(VSOCK_FEATURES & (1 << 26), 0, "VHOST_F_LOG_ALL");
    }

    /// Opens a backend for a CID no other process uses, or `None` when the
    /// host cannot run these probes (a machine without the module loaded has no
    /// `/dev/vhost-vsock` to open; a cross-target runner emulates the ioctls
    /// away and answers `ENOTTY`). Saying so is honest: the test reports
    /// nothing rather than pretending to have checked the device.
    ///
    /// A wrong ioctl *encoding* would look like the same `ENOTTY`, which is why
    /// the encodings are also asserted on their own, without a device, by
    /// `ioctl_request_codes_match_the_kernels` and
    /// `ioctl_argument_layouts_match_the_kernels` — those run everywhere.
    ///
    /// The CID is drawn from the top of the valid range, offset by the process
    /// id and by a per-call counter, so a probe can never collide with the CIDs
    /// a running fleet, another test binary or this binary's parallel tests use.
    fn probe_backend() -> Option<VhostVsock> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let cid =
            0x8000_0000 + u64::from(std::process::id()) + NEXT.fetch_add(1, Ordering::Relaxed);
        match VhostVsock::new(cid) {
            Ok(backend) => Some(backend),
            Err(VhostError::Open { .. }) => None,
            // The interface is not there (an emulated ioctl answers ENOTTY).
            Err(VhostError::Ioctl { source, .. })
                if source.raw_os_error() == Some(libc::ENOTTY) =>
            {
                None
            }
            Err(err) => panic!("the host's vhost-vsock device refused a fresh CID {cid}: {err}"),
        }
    }

    /// Three queues' worth of ring addresses inside a 4 MiB guest memory, as a
    /// driver that finished configuring the device would have left them: each
    /// queue's descriptor table, available ring and used ring in memory nothing
    /// else of the probe touches.
    fn probe_queues() -> Vec<QueueConfig> {
        (0..3u64)
            .map(|index| {
                let desc = 0x10_000 * (index + 1);
                QueueConfig {
                    max_size: 256,
                    size: 64,
                    ready: true,
                    desc,
                    avail: desc + 0x1000,
                    used: desc + 0x2000,
                }
            })
            .collect()
    }

    /// The acceptance probe for this backend, run against the host's real
    /// `/dev/vhost-vsock`: the ioctl sequence this module implements must take
    /// a device all the way from "owned" to "running", with the memory table
    /// and the rings this VMM program, and the kernel must accept the guest
    /// CID.
    ///
    /// This is what the device model cannot check on its own — everything above
    /// it is exercised by the MMIO transport's unit tests — and it is the same
    /// sequence a booting VM drives, so a change that breaks it is caught here
    /// rather than at the first guest's `hello`.
    #[test]
    fn the_host_can_start_a_vhost_vsock_device() {
        let Some(mut backend) = probe_backend() else {
            return;
        };
        let features = backend.features();
        assert_ne!(
            features & VIRTIO_F_VERSION_1,
            0,
            "the kernel's vhost-vsock must offer VIRTIO_F_VERSION_1"
        );

        let mem = crate::vstate::memory::tests::single_region_mem(mib_to_bytes(4));
        let irq = EventFd::new(libc::EFD_NONBLOCK).expect("event fd");
        backend
            .start(&mem, &irq, features, &probe_queues())
            .expect("starting the device with wired rings");

        // Kicking a queue the backend owns is a plain event fd write...
        backend.kick(0).expect("kick the receive queue");
        // ...while a queue it does not have (the virtio-vsock event queue,
        // which is the guest's alone) is accepted and ignored.
        backend
            .kick(2)
            .expect("kick a queue the backend does not have");
        // The device count the kernel reports is what makes that distinction
        // work: the probe's third queue must have been past the end of the
        // backend's own queue array.
        assert!(
            !backend.set_vring_num(2, 64).expect("probe queue 2"),
            "this kernel's vhost-vsock has an event queue after all"
        );
    }

    /// A CID another backend already claims must not be taken twice: the second
    /// VM's device fails to open rather than both VMs sharing one CID (which
    /// would deliver a guest's packets to the wrong place).
    #[test]
    fn a_guest_cid_cannot_be_claimed_twice() {
        let Some(first) = probe_backend() else {
            return;
        };
        let cid = first.guest_cid();
        assert!(matches!(
            VhostVsock::new(cid),
            Err(VhostError::Ioctl {
                name: "VHOST_VSOCK_SET_GUEST_CID",
                ..
            })
        ));
        drop(first);
        // With the first backend gone, the CID is free again.
        let second = VhostVsock::new(cid).expect("the CID must be free again");
        assert_eq!(second.guest_cid(), cid);
    }

    /// A queue whose three rings are entirely inside one guest memory region
    /// translates to non-zero host addresses.
    #[test]
    fn rings_that_lie_inside_one_region_translate() {
        let mem = crate::vstate::memory::tests::single_region_mem(mib_to_bytes(4));
        let queue = QueueConfig {
            max_size: 256,
            size: 64,
            ready: true,
            desc: 0x10_000,
            avail: 0x11_000,
            used: 0x12_000,
        };
        let [desc, used, avail] =
            translate_rings(0, &mem, &queue).expect("in-range rings translate");
        assert_ne!(desc, 0);
        assert_ne!(used, 0);
        assert_ne!(avail, 0);
    }

    /// A ring whose start address is valid but whose extent runs past the end
    /// of the guest memory region is rejected, for each of the three ring
    /// kinds, with `RingAddress` naming the offending start.
    #[test]
    fn rings_whose_extent_crosses_a_region_end_are_rejected() {
        let region_size = mib_to_bytes(4);
        let mem = crate::vstate::memory::tests::single_region_mem(region_size);
        // A legal power-of-two size the kernel's vhost ioctl accepts.
        let num = 0x8000u32;

        // The descriptor table: 16 * num bytes. Start it one byte before the
        // region end, so its last byte would land outside the region.
        let desc = region_size - ring_extent_size(RingKind::Desc, num) + 1;
        let queue = QueueConfig {
            max_size: 256,
            size: num,
            ready: true,
            desc: desc as u64,
            avail: 0x2_0000,
            used: 0x3_0000,
        };
        assert!(matches!(
            translate_rings(0, &mem, &queue),
            Err(VhostError::RingAddress { index: 0, addr }) if addr == desc as u64
        ));

        // The used ring: 4 + 8 * num + 2 bytes.
        let used = region_size - ring_extent_size(RingKind::Used, num) + 1;
        let queue = QueueConfig {
            max_size: 256,
            size: num,
            ready: true,
            desc: 0x1_0000,
            avail: 0x2_0000,
            used: used as u64,
        };
        assert!(matches!(
            translate_rings(0, &mem, &queue),
            Err(VhostError::RingAddress { index: 0, addr }) if addr == used as u64
        ));

        // The available ring: 4 + 2 * num + 2 bytes.
        let avail = region_size - ring_extent_size(RingKind::Avail, num) + 1;
        let queue = QueueConfig {
            max_size: 256,
            size: num,
            ready: true,
            desc: 0x1_0000,
            avail: avail as u64,
            used: 0x3_0000,
        };
        assert!(matches!(
            translate_rings(0, &mem, &queue),
            Err(VhostError::RingAddress { index: 0, addr }) if addr == avail as u64
        ));
    }

    /// The available and used ring extents include the two trailing
    /// event-index bytes (`VIRTIO_RING_F_EVENT_IDX`), so a ring that only fits
    /// when those bytes are *not* counted is still rejected — validating a
    /// larger extent can only fail closed.
    #[test]
    fn event_index_bytes_are_counted_in_the_extent() {
        let region_size = mib_to_bytes(4);
        let mem = crate::vstate::memory::tests::single_region_mem(region_size);
        let num = 256u32;

        // A used ring placed so `4 + 8 * num` bytes end exactly at the region
        // end fits without the event-index bytes but crosses with them.
        let used = region_size - (4 + 8 * num as usize);
        let queue = QueueConfig {
            max_size: 256,
            size: num,
            ready: true,
            desc: 0x1_0000,
            avail: 0x2_0000,
            used: used as u64,
        };
        assert!(matches!(
            translate_rings(0, &mem, &queue),
            Err(VhostError::RingAddress { index: 0, .. })
        ));

        // The same rings far from any region end translate fine.
        let queue = QueueConfig {
            max_size: 256,
            size: num,
            ready: true,
            desc: 0x1_0000,
            avail: 0x2_0000,
            used: 0x3_0000,
        };
        translate_rings(0, &mem, &queue).expect("in-range rings translate");
    }
}
