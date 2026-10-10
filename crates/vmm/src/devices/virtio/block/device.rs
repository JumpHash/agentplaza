// Copyright 2021 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/block/virtio/device.rs
// and .../io/sync_io.rs (upstream commit
// fd9fc7a9f164b73002d22065d17c150d8c262d91). `DiskProperties`, the
// `virtio_blk_config` configuration space and `process_queue`'s structure
// (pop a chain, parse it, execute it, publish the used-ring entry, kick the
// guest when the driver asked to be told) are upstream's. Trimmed: no async
// io_uring engine (the only engine is the synchronous one, upstream's
// `SyncFileEngine`: seek plus a volatile transfer straight between the file and
// guest memory, so nothing is copied through an intermediate buffer and nothing
// is allocated per request), no rate limiter, no metrics, no snapshot support,
// no multi-queue, no read-only drives, and no DISCARD (its `fallocate` /
// `BLKDISCARD` ioctls are `unsafe` and a writable workspace does not need them),
// and no `CacheType` (the device always fsyncs on a flush request: the
// workspace is where an agent's work lives, so durability wins over the
// throughput of the guest's own journal commits).

//! The virtio-block device itself: one request queue over a host file.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom};
use std::path::Path;

use vm_memory::{GuestMemoryBackend, ReadVolatile, WriteVolatile};
use vmm_sys_util::eventfd::EventFd;
use zerocopy::IntoBytes;
use zerocopy::little_endian::{U16, U32, U64};

use super::request::{FinishedRequest, IoErr, Request, VIRTIO_BLK_ID_BYTES};
use super::{
    ActivateError, BLOCK_QUEUE_SIZE, MAX_DISCARD_SECTORS, SECTOR_SHIFT, SECTOR_SIZE,
    VirtioBlockError,
};
use crate::devices::virtio::queue::{InvalidAvailIdx, Queue};
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap, u64_to_usize};

/// `VIRTIO_F_VERSION_1` (feature bit 32): the device conforms to virtio 1.0.
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
/// `VIRTIO_RING_F_EVENT_IDX` (feature bit 29): the device supports the
/// used_event/avail_event notification suppression of the split ring.
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;
/// `VIRTIO_BLK_F_FLUSH` (feature bit 9): the device honours a cache flush.
const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;

/// The features this device offers the driver: the modern-transport feature,
/// notification suppression and the cache flush. It deliberately does **not**
/// offer `VIRTIO_BLK_F_BLK_SIZE`/`TOPOLOGY`/`SEG_MAX`/`RO`/`DISCARD`/
/// `WRITE_ZEROES`/`MQ`: every one of them would make the driver read a
/// configuration field this device does not need to serve (512 bytes is the
/// protocol's default logical block size), and a device that advertises support
/// it does not implement is a bug, not a feature.
pub(crate) const AVAIL_FEATURES: u64 =
    VIRTIO_F_VERSION_1 | VIRTIO_RING_F_EVENT_IDX | VIRTIO_BLK_F_FLUSH;

/// The configuration space's topology field (`VIRTIO_BLK_F_TOPOLOGY`, which is
/// not offered; it exists so the configuration space's layout is the one
/// `struct virtio_blk_config` declares).
#[derive(Debug, Clone, Copy, Default, zerocopy::Immutable, zerocopy::FromBytes, IntoBytes)]
#[repr(C)]
pub struct VirtioBlkTopology {
    /// Number of logical blocks per physical block (log2).
    pub physical_block_exp: u8,
    /// Offset of the first aligned logical block.
    pub alignment_offset: u8,
    /// Suggested minimum I/O size, in blocks.
    pub min_io_size: U16,
    /// Optimal (suggested maximum) I/O size, in blocks.
    pub opt_io_size: U32,
}

/// The virtio-blk configuration space (`struct virtio_blk_config`,
/// `linux/virtio_blk.h`).
///
/// The fields are little-endian because virtio transmits them that way; the
/// `little_endian` newtypes make the encoding explicit (upstream reads and
/// writes platform-endian `ByteValued` PODs, which is only correct on a
/// little-endian host — and needs `unsafe`, which this port does not spend on a
/// 48-byte register file).
///
/// A driver reads `capacity` unconditionally and every other field only when
/// its feature was negotiated. Only `capacity` and `blk_size` are meaningful
/// here (and `blk_size` is only meaningful to a driver that reads it anyway):
/// the device offers neither `TOPOLOGY` nor `DISCARD`, so those fields are
/// inert. They carry upstream's defaults to keep the layout byte-identical to
/// Firecracker's.
#[derive(Debug, Clone, Copy, zerocopy::Immutable, zerocopy::FromBytes, IntoBytes)]
#[repr(C)]
pub struct ConfigSpace {
    /// The volume's capacity, in 512-byte sectors.
    pub capacity: U64,
    /// Maximum segment size (`VIRTIO_BLK_F_SIZE_MAX`; not offered).
    pub size_max: U32,
    /// Maximum number of segments (`VIRTIO_BLK_F_SEG_MAX`; not offered).
    pub seg_max: U32,
    /// Legacy geometry (`VIRTIO_BLK_F_GEOMETRY`; not offered).
    pub geometry: U32,
    /// Logical block size.
    pub blk_size: U32,
    /// Block topology (`VIRTIO_BLK_F_TOPOLOGY`; not offered).
    pub topology: VirtioBlkTopology,
    /// Writeback mode (`VIRTIO_BLK_F_CONFIG_WCE`; not offered).
    pub wce: u8,
    /// Unused, zero.
    pub unused: u8,
    /// Number of virtqueues (`VIRTIO_BLK_F_MQ`; not offered).
    pub num_queues: U16,
    /// Maximum discard sectors (`VIRTIO_BLK_F_DISCARD`; not offered).
    pub max_discard_sectors: U32,
    /// Maximum discard segments (`VIRTIO_BLK_F_DISCARD`; not offered).
    pub max_discard_seg: U32,
    /// Discard alignment (`VIRTIO_BLK_F_DISCARD`; not offered).
    pub discard_sector_alignment: U32,
}

impl ConfigSpace {
    /// A configuration space for a volume of `capacity` sectors.
    fn new(capacity: u64) -> Self {
        Self {
            capacity: capacity.into(),
            size_max: 0.into(),
            seg_max: 0.into(),
            geometry: 0.into(),
            blk_size: SECTOR_SIZE.into(),
            topology: VirtioBlkTopology {
                physical_block_exp: 0,
                alignment_offset: 0,
                min_io_size: 0.into(),
                // Upstream's default: 128 logical blocks (64 KiB).
                opt_io_size: 128.into(),
            },
            wce: 0,
            unused: 0,
            num_queues: 0.into(),
            max_discard_sectors: MAX_DISCARD_SECTORS.into(),
            max_discard_seg: 1.into(),
            discard_sector_alignment: 1.into(),
        }
    }
}

/// The device's backing file and the properties the driver is told about.
#[derive(Debug)]
pub struct DiskProperties {
    /// The volume, opened read-write.
    file: File,
    /// The volume's capacity, in 512-byte sectors.
    nsectors: u64,
    /// What a `VIRTIO_BLK_T_GET_ID` request returns.
    ///
    /// Always zeroes: upstream builds it from the file's metadata (the backing
    /// device's id, the filesystem's UUID), which is host information a guest
    /// has no business reading — and in this port the guest is untrusted by
    /// design (plaza invariant 2: nothing about the host or the node reaches a
    /// guest that the operator did not deliberately put there).
    image_id: [u8; VIRTIO_BLK_ID_BYTES as usize],
}

impl DiskProperties {
    /// Opens `path` as the device's writable volume.
    ///
    /// The file must exist: a caller that configured a workspace image is
    /// telling the VMM where the volume is, and a missing image fails the boot
    /// with the path in the error rather than booting a guest with no volume.
    pub fn new(path: &Path) -> Result<Self, VirtioBlockError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| VirtioBlockError::BackingFile(err, path.display().to_string()))?;
        let size = file
            .metadata()
            .map_err(|err| VirtioBlockError::BackingFile(err, path.display().to_string()))?
            .len();
        Ok(Self {
            file,
            // A size that is not a whole number of sectors is truncated, as a
            // real block device's is.
            nsectors: size >> SECTOR_SHIFT,
            image_id: [0; VIRTIO_BLK_ID_BYTES as usize],
        })
    }

    /// The volume's capacity, in sectors: the parser's bound on every request.
    fn nsectors(&self) -> u64 {
        self.nsectors
    }

    /// The device id a `GET_ID` request returns (always zeroes; see
    /// [`DiskProperties::image_id`]).
    pub(super) fn image_id(&self) -> &[u8; VIRTIO_BLK_ID_BYTES as usize] {
        &self.image_id
    }

    /// Reads `count` bytes at `offset` into the guest's buffer at `addr`.
    ///
    /// This is upstream's `SyncFileEngine::read`: seek, then a *volatile*
    /// transfer straight between the file and guest memory, so the guest's
    /// buffer is filled in place and nothing is copied through a scratch
    /// buffer. Guest memory is volatile by contract (the guest may be running
    /// on another vCPU while this happens), which is why the transfer goes
    /// through `ReadVolatile` rather than a `&mut [u8]`.
    pub(super) fn read(
        &mut self,
        offset: u64,
        mem: &GuestMemoryMmap,
        addr: GuestAddress,
        count: u32,
    ) -> Result<u32, IoErr> {
        self.file.seek(SeekFrom::Start(offset))?;
        let mut slice = mem
            .get_slice(addr, u64_to_usize(u64::from(count)))
            .map_err(IoErr::GuestMemory)?;
        self.file.read_exact_volatile(&mut slice)?;
        Ok(count)
    }

    /// Writes `count` bytes from the guest's buffer at `addr` to `offset`.
    pub(super) fn write(
        &mut self,
        offset: u64,
        mem: &GuestMemoryMmap,
        addr: GuestAddress,
        count: u32,
    ) -> Result<u32, IoErr> {
        self.file.seek(SeekFrom::Start(offset))?;
        let slice = mem
            .get_slice(addr, u64_to_usize(u64::from(count)))
            .map_err(IoErr::GuestMemory)?;
        self.file.write_all_volatile(&slice)?;
        Ok(count)
    }

    /// Flushes the volume's write cache to the host's storage.
    ///
    /// The guest's filesystem sends a flush at every journal commit and
    /// `fsync`, and the volume is the only place an agent's work survives the
    /// VM, so the flush is a real `fsync(2)` — not the no-op a
    /// fire-and-forget write cache would be.
    pub(super) fn flush(&mut self) -> Result<u32, IoErr> {
        self.file.sync_all()?;
        Ok(0)
    }
}

/// The virtio-block device: one request queue over a host file.
///
/// The device is *synchronous*: [`VirtioBlock::process_queue`] runs inside the
/// transport's queue notification, on the vCPU thread that wrote `QueueNotify`,
/// and returns when the driver's available ring is drained. There is therefore
/// no event thread and no lock of its own — the MMIO bus already serializes
/// every access to the transport that owns this device.
#[derive(Debug)]
pub struct VirtioBlock {
    /// The workspace volume and its properties.
    disk: DiskProperties,
    /// The configuration space the driver reads the capacity from.
    config_space: ConfigSpace,
    avail_features: u64,
    acked_features: u64,
    /// The request queue. Its ring addresses arrive from the transport at
    /// activation (`activate` resolves them in guest memory).
    queue: Queue,
    /// Guest memory, `Some` exactly while the device is activated.
    mem: Option<GuestMemoryMmap>,
    /// The interrupt the transport registered as the device's irqfd, `Some`
    /// exactly while the device is activated.
    irq: Option<EventFd>,
}

impl VirtioBlock {
    /// Opens `path` as the guest's writable workspace volume.
    ///
    /// The file is opened `O_RDWR`: this device is the only writable queue in
    /// the port (invariant 3), and every request the driver sends is bounds
    /// checked against the file's size before it is served.
    pub fn new(path: &Path) -> Result<VirtioBlock, VirtioBlockError> {
        let disk = DiskProperties::new(path)?;
        let config_space = ConfigSpace::new(disk.nsectors());
        Ok(VirtioBlock {
            disk,
            config_space,
            avail_features: AVAIL_FEATURES,
            acked_features: 0,
            queue: Queue::new(BLOCK_QUEUE_SIZE),
            mem: None,
            irq: None,
        })
    }

    /// The features the device offers the driver.
    pub fn avail_features(&self) -> u64 {
        self.avail_features
    }

    /// The features the driver has acknowledged so far.
    pub fn acked_features(&self) -> u64 {
        self.acked_features
    }

    /// Records the features the driver acknowledged.
    pub fn set_acked_features(&mut self, acked: u64) {
        self.acked_features = acked;
    }

    /// Whether the driver acknowledged `feature`.
    fn has_feature(&self, feature: u64) -> bool {
        self.acked_features & feature != 0
    }

    /// The driver-visible configuration space, little-endian.
    pub fn config_as_bytes(&self) -> &[u8] {
        self.config_space.as_bytes()
    }

    /// The request queue, for the transport to program the driver's registers
    /// into before activation.
    pub fn queue_mut(&mut self) -> &mut Queue {
        &mut self.queue
    }

    /// Whether the device has been activated (and so can serve its queue).
    pub fn is_activated(&self) -> bool {
        self.mem.is_some()
    }

    /// Activates the device against `mem`, raising its interrupt through `irq`.
    ///
    /// The transport must have programmed the queue's ring addresses first,
    /// because resolving the rings in guest memory is what this does; a queue
    /// the driver never marked ready is refused by [`Queue::initialize`] and
    /// the activation fails, rather than leaving a guest with a device that
    /// never answers.
    pub fn activate(&mut self, mem: GuestMemoryMmap, irq: EventFd) -> Result<(), ActivateError> {
        self.queue.initialize(&mem)?;
        if self.has_feature(VIRTIO_RING_F_EVENT_IDX) {
            self.queue.enable_notif_suppression();
        }
        self.mem = Some(mem);
        self.irq = Some(irq);
        Ok(())
    }

    /// Resets the device to its state before activation, as the transport does
    /// when the driver writes status 0.
    pub fn reset(&mut self) {
        // A fresh queue, so a driver that re-initializes the device starts from
        // empty avail/used cursors rather than continuing a ring whose
        // addresses the transport has already dropped.
        self.queue = Queue::new(BLOCK_QUEUE_SIZE);
        self.acked_features = 0;
        self.mem = None;
        self.irq = None;
    }

    /// Serves every request the driver has published in the request queue.
    ///
    /// The loop is upstream's `VirtioBlk::process_queue`: pop the next
    /// available chain, parse and validate it, execute it, publish its
    /// used-ring entry, and stop when the ring is drained (arming the driver's
    /// notification if the driver negotiated suppression). A chain that does
    /// not parse is *consumed* with a zero-length used entry — the driver must
    /// be able to make progress, and the chain is not the device's to
    /// interpret — while an execution failure is reported to the driver as a
    /// status byte inside its own chain.
    ///
    /// The only error returned is the driver asking for more descriptors than
    /// the ring can hold ([`InvalidAvailIdx`]), which the transport logs and
    /// treats as a device failure rather than a VMM fault.
    pub fn process_queue(&mut self) -> Result<(), InvalidAvailIdx> {
        let VirtioBlock {
            disk,
            queue,
            mem,
            irq,
            ..
        } = self;
        // Not activated: the transport never notifies an inactive device, but a
        // notification that raced a reset must not walk a ring the reset
        // released.
        let Some(guest_mem) = mem.as_ref() else {
            return Ok(());
        };

        let mut used_any = false;
        while let Some(head) = queue.pop_or_enable_notification()? {
            let finished = match Request::parse(&head, guest_mem, disk.nsectors()) {
                Ok(request) => request.process(disk, head.index, guest_mem),
                Err(err) => {
                    eprintln!(
                        "warning: virtio-block: rejecting the driver's descriptor chain: {err}"
                    );
                    FinishedRequest {
                        num_bytes_to_mem: 0,
                        desc_idx: head.index,
                    }
                }
            };

            used_any = true;
            if let Err(err) = queue.add_used(finished.desc_idx, finished.num_bytes_to_mem) {
                eprintln!(
                    "warning: virtio-block: cannot publish descriptor {} in the used ring: {err}",
                    finished.desc_idx
                );
            }
        }
        queue.advance_used_ring_idx();

        if used_any
            && queue.prepare_kick()
            && let Some(irq) = irq.as_ref()
            && let Err(err) = irq.write(1)
        {
            eprintln!("warning: virtio-block: cannot raise the device interrupt: {err}");
        }
        Ok(())
    }
}

/// The `io::Error` behind a volatile-memory failure.
///
/// A transfer failure is either the file itself (an `io::Error`) or the guest
/// buffer's bounds (this port checks those in the parser, so this is a race
/// with a shrinking memory map rather than a driver error); both are reported
/// to the driver as `VIRTIO_BLK_S_IOERR`.
fn volatile_io_error(err: vm_memory::VolatileMemoryError) -> io::Error {
    match err {
        vm_memory::VolatileMemoryError::IOError(err) => err,
        other => io::Error::other(other),
    }
}

// The volatile transfer traits report their own error type; a `DiskProperties`
// transfer reports whatever it carries.
impl From<vm_memory::VolatileMemoryError> for IoErr {
    fn from(err: vm_memory::VolatileMemoryError) -> Self {
        IoErr::File(volatile_io_error(err))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::FileExt;

    use vmm_sys_util::tempfile::TempFile;

    use super::super::request::{
        RequestHeader, VIRTIO_BLK_S_IOERR, VIRTIO_BLK_S_OK, VIRTIO_BLK_S_UNSUPP,
        VIRTIO_BLK_T_DISCARD, VIRTIO_BLK_T_FLUSH, VIRTIO_BLK_T_GET_ID, VIRTIO_BLK_T_IN,
        VIRTIO_BLK_T_OUT,
    };
    use super::*;
    use crate::devices::virtio::block::test_utils::RequestDescriptorChain;
    use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};

    /// A volume holding `contents`, and a device over it.
    fn volume(contents: &[u8]) -> (TempFile, VirtioBlock) {
        let file = TempFile::new().expect("a temporary file");
        file.as_file()
            .set_len(contents.len() as u64)
            .expect("a sized volume");
        file.as_file()
            .write_all_at(contents, 0)
            .expect("the volume's contents");
        let device = VirtioBlock::new(file.as_path()).expect("a block device over the volume");
        (file, device)
    }

    /// Activates `device` against `mem` with `device`'s queue already over the
    /// [`VirtQueue`] at `mem`'s start.
    fn activate(device: &mut VirtioBlock, mem: &GuestMemoryMmap) {
        let irq = EventFd::new(libc::EFD_NONBLOCK).expect("an interrupt event fd");
        device
            .activate(mem.clone(), irq)
            .expect("the device activates against guest memory");
    }

    /// The features this device offers, exactly: adding a feature bit without
    /// implementing it (a configuration field the driver would then read) is
    /// the bug this pins.
    #[test]
    fn the_offered_features_are_exactly_the_implemented_ones() {
        let (_file, block) = volume(&[0u8; 4096]);
        assert_eq!(
            block.avail_features(),
            (1 << 32) | (1 << 29) | (1 << 9),
            "VIRTIO_F_VERSION_1 | VIRTIO_RING_F_EVENT_IDX | VIRTIO_BLK_F_FLUSH"
        );
        assert_eq!(AVAIL_FEATURES, block.avail_features());
    }

    /// The configuration space reports the volume's capacity in 512-byte
    /// sectors — the one field a driver reads from a device that negotiated no
    /// optional feature — with the 512-byte logical block size.
    #[test]
    fn the_config_space_reports_the_volume_capacity() {
        let (_file, block) = volume(&[0u8; 16 * 4096]);
        let config: ConfigSpace =
            zerocopy::FromBytes::read_from_bytes(block.config_as_bytes()).expect("a config space");
        assert_eq!(config.capacity.get(), 16 * 4096 / 512);
        assert_eq!(config.blk_size.get(), 512);
        assert_eq!(
            block.config_as_bytes().len(),
            48,
            "struct virtio_blk_config through discard_sector_alignment"
        );
    }

    /// A read request copies the volume's bytes into the driver's buffer, marks
    /// the descriptor used with the data length plus the status byte, and
    /// leaves `VIRTIO_BLK_S_OK` in the chain.
    #[test]
    fn a_read_request_returns_the_volumes_bytes() {
        let payload: Vec<u8> = (0..1024u32).map(|byte| (byte % 251) as u8).collect();
        let (_file, mut block) = volume(&payload);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_IN, 0));
        chain.set_data_len(1024);
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");

        assert_eq!(chain.status(), VIRTIO_BLK_S_OK);
        assert_eq!(chain.read_data(1024), payload);
        assert_eq!(vq.used_idx(), 1, "one descriptor is used");
        assert_eq!(
            vq.used_elem(0),
            (0, 1024 + 1),
            "the used length is the data plus the status byte"
        );
    }

    /// A write request stores the driver's bytes in the volume, and a later
    /// read of the same sectors returns them — the two directions agree.
    #[test]
    fn a_write_request_stores_the_guests_bytes() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let written: Vec<u8> = (0..512u32).map(|byte| (byte % 199) as u8).collect();
        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_OUT, 2));
        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.set_data_len(512);
        chain.write_data(&written);
        chain.publish();
        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_OK);
        assert_eq!(
            vq.used_elem(0),
            (0, 1),
            "a write reports only the status byte as written back to the guest"
        );

        // Read the same sector back through the device.
        block.queue = vq.create_queue();
        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_IN, 2));
        chain.set_data_len(512);
        chain.publish();
        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_OK);
        assert_eq!(chain.read_data(512), written);
    }

    /// A flush request is acknowledged (the device fsyncs the volume).
    #[test]
    fn a_flush_request_is_acknowledged() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_FLUSH, 0));
        // A flush has no data descriptor: the status descriptor follows the
        // header directly.
        chain.set_header_next(2);
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_OK);
        assert_eq!(vq.used_elem(0), (0, 1));
    }

    /// A request past the end of the volume never reaches the file: the parser
    /// rejects it (upstream's `InvalidOffset` check, before any I/O), and a
    /// chain that does not parse is consumed without a status byte — which the
    /// driver reads as a failed request, since it never wrote a status there
    /// itself.
    #[test]
    fn a_request_past_the_end_of_the_volume_never_reaches_the_volume() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        // The volume is 8 sectors; a 512-byte read at sector 8 is past its end.
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_IN, 8));
        chain.set_data_len(512);
        chain.write_data(&[0xff; 512]);
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(
            vq.used_elem(0),
            (0, 0),
            "the chain is consumed without a status byte and without data"
        );
        assert_eq!(
            chain.read_data(512),
            vec![0xff; 512],
            "the driver's buffer is untouched: the read never happened"
        );
    }

    /// An unknown request type is answered `VIRTIO_BLK_S_UNSUPP` — the device
    /// telling the driver "not implemented" — and the descriptor is still
    /// consumed, so the driver can make progress.
    #[test]
    fn an_unknown_request_type_is_answered_unsupported() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(42, 0));
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_UNSUPP);
        assert_eq!(vq.used_elem(0), (0, 1));
    }

    /// A discard request is refused: the device never offers
    /// `VIRTIO_BLK_F_DISCARD`, and a driver that sends one anyway gets
    /// `VIRTIO_BLK_S_UNSUPP` rather than a silently ignored trim.
    #[test]
    fn a_discard_request_is_refused() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_DISCARD, 0));
        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.set_data_len(16);
        // A well-formed discard range (sector 0, one sector, no flags): the
        // request parses, and only then is it refused as unsupported.
        let mut range = [0u8; 16];
        range[8..12].copy_from_slice(&1u32.to_le_bytes());
        chain.write_data(&range);
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_UNSUPP);
    }

    /// A chain that does not parse is consumed with a zero-length used entry
    /// and *no* status byte written: the descriptor is not the device's to
    /// interpret, and leaving it in the ring would wedge the driver.
    #[test]
    fn a_malformed_chain_is_consumed_without_a_status() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        // A write-only *header* descriptor is not a request at all.
        chain.set_header_flags(VIRTQ_DESC_F_WRITE);
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(vq.used_idx(), 1, "the chain is consumed");
        assert_eq!(vq.used_elem(0), (0, 0), "and nothing was written back");
    }

    /// `GET_ID` answers with zeroes: the host's file metadata (its backing
    /// device's id, the filesystem's UUID) must not reach a guest.
    #[test]
    fn a_get_id_request_returns_no_host_data() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_GET_ID, 0));
        chain.set_data_len(VIRTIO_BLK_ID_BYTES);
        chain.write_data(&[0xaa; VIRTIO_BLK_ID_BYTES as usize]);
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_OK);
        assert_eq!(
            chain.read_data(VIRTIO_BLK_ID_BYTES as usize),
            vec![0u8; VIRTIO_BLK_ID_BYTES as usize]
        );
    }

    /// A data buffer that is not (entirely) in guest memory is an I/O error:
    /// not a VMM fault, and not a partial transfer the driver could mistake for
    /// success.
    #[test]
    fn a_data_buffer_outside_guest_memory_is_an_io_error() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_IN, 0));
        chain.set_data_len(512);
        // The last byte of guest memory: a 512-byte read cannot fit.
        chain.set_data_addr(mem.last_addr().0);
        chain.publish();

        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_IOERR);
    }

    /// The device resets to the state before activation: the queue is empty,
    /// the acknowledged features are gone and the rings are released. A
    /// notification that arrives afterwards is ignored rather than walking a
    /// ring the reset dropped.
    #[test]
    fn a_reset_releases_the_guest_rings() {
        let (_file, mut block) = volume(&[0u8; 4096]);
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);
        block.set_acked_features(AVAIL_FEATURES);
        assert!(block.is_activated());

        block.reset();

        assert!(!block.is_activated());
        assert_eq!(block.acked_features(), 0);
        assert_eq!(block.queue, Queue::new(BLOCK_QUEUE_SIZE));
        block.process_queue().expect("an inactive device ignores the queue");
    }

    /// What the device writes is in the image file itself — which is what the
    /// next VM booted on the same copy reads back.
    #[test]
    fn what_the_device_writes_is_in_the_image_file() {
        let file = TempFile::new().expect("a temporary file");
        file.as_file().set_len(4096).expect("a sized volume");
        let mut block = VirtioBlock::new(file.as_path()).expect("a device over the volume");
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        block.queue = vq.create_queue();
        activate(&mut block, &mem);

        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_OUT, 1));
        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.set_data_len(512);
        chain.write_data(&[7u8; 512]);
        chain.publish();
        block.process_queue().expect("a well-formed ring drains");
        assert_eq!(chain.status(), VIRTIO_BLK_S_OK);

        let mut stored = [0u8; 512];
        file.as_file()
            .read_exact_at(&mut stored, 512)
            .expect("a read of the volume");
        assert_eq!(stored, [7u8; 512]);
        assert_eq!(
            file.as_file().metadata().expect("metadata").len(),
            4096,
            "the volume's size is the file's size"
        );
    }
}
