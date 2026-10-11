// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the balloon's queue processing, config space and
// free-page reporting. The transport's register protocol is the MMIO
// transport's; this file is the device behind it (the block device's shape,
// without a separate adapter: the transport's `QueueConfig`s are held here).

//! The virtio-balloon device on the MMIO transport.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use vm_memory::GuestMemoryBackend;
use vmm_sys_util::eventfd::EventFd;

use super::{
    BALLOON_QUEUE_NUM, BALLOON_QUEUE_SIZE, BalloonError, BalloonStats, DEFLATE_QUEUE,
    INFLATE_QUEUE, PAGE_SHIFT, PAGE_SIZE, REPORTING_QUEUE, STAT_AVAILABLE_MEMORY,
    STAT_FREE_MEMORY, STAT_TOTAL_MEMORY, STATS_QUEUE, VIRTIO_BALLOON_F_DEFLATE_ON_OOM,
    VIRTIO_BALLOON_F_REPORTING, VIRTIO_BALLOON_F_STATS_VQ, VIRTIO_ID_BALLOON,
};
use crate::devices::virtio::VirtioError;
use crate::devices::virtio::mmio::{
    QueueConfig, VIRTIO_MMIO_INT_CONFIG, VIRTIO_MMIO_INT_VRING, VirtioDevice,
};
use crate::devices::virtio::queue::{DescriptorChain, Queue};
use crate::devices::virtio::vhost_device::ack_feature_page;
use crate::vstate::memory::{Bytes, GuestAddress, GuestMemoryMmap};

/// `VIRTIO_F_VERSION_1` (feature bit 32): the device conforms to virtio 1.0.
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// Offset of `actual` in the configuration space; `num_pages` is the 4 bytes before it.
const CONFIG_ACTUAL_OFFSET: usize = 4;

/// Largest driver buffer the device reads. A guest cannot make it allocate more.
const MAX_BUFFER_BYTES: usize = 64 * 1024;

/// One stats entry on the wire: a packed `{ tag: u16, val: u64 }`.
const STAT_ENTRY_SIZE: usize = 10;

/// Where the device gives memory back to the host.
///
/// The default is [`MadviseDiscard`]; tests inject a recorder.
pub trait Discard: Send {
    /// Discards the host memory `[host, host + len)`, which backs guest pages.
    fn discard(&mut self, host: *mut u8, len: usize) -> io::Result<()>;
}

/// Discards with `madvise(MADV_DONTNEED)` (see [`crate::sys::discard`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct MadviseDiscard;

impl Discard for MadviseDiscard {
    fn discard(&mut self, host: *mut u8, len: usize) -> io::Result<()> {
        crate::sys::discard(host, len)
    }
}

/// A virtio-balloon device.
pub struct VirtioBalloon {
    /// The host's target size, in pages (`num_pages`, written by the host).
    num_pages: u32,
    /// The size the guest has given back, in pages (`actual`, written by the guest).
    actual: u32,
    /// The driver-visible register set of each queue.
    queue_regs: [QueueConfig; BALLOON_QUEUE_NUM],
    /// The device-side rings, initialized on activation.
    rings: [Queue; BALLOON_QUEUE_NUM],
    /// The features the driver acknowledged.
    acked_features: u64,
    /// The latest value the guest reported for each stats tag.
    stats: BTreeMap<u16, u64>,
    /// How memory is given back to the host.
    discard: Box<dyn Discard>,
    /// Guest memory, set on activation.
    mem: Option<GuestMemoryMmap>,
    /// The device's interrupt, set on activation.
    irq: Option<EventFd>,
    /// The transport's interrupt status, where a config change is recorded
    /// (`VIRTIO_MMIO_INT_CONFIG`); set by [`VirtioBalloon::set_interrupt_status`].
    interrupt_status: Option<Arc<AtomicU32>>,
}

impl VirtioBalloon {
    /// A balloon that gives memory back with `madvise(MADV_DONTNEED)`.
    pub fn new() -> Self {
        Self::with_discard(Box::new(MadviseDiscard))
    }

    /// A balloon that gives memory back through `discard`.
    pub fn with_discard(discard: Box<dyn Discard>) -> Self {
        Self {
            num_pages: 0,
            actual: 0,
            queue_regs: [QueueConfig::new(u32::from(BALLOON_QUEUE_SIZE)); BALLOON_QUEUE_NUM],
            rings: std::array::from_fn(|_| Queue::new(BALLOON_QUEUE_SIZE)),
            acked_features: 0,
            stats: BTreeMap::new(),
            discard,
            mem: None,
            irq: None,
            interrupt_status: None,
        }
    }

    /// Hands the balloon the transport's shared interrupt status
    /// ([`VirtioMmioTransport::interrupt_status_handle`]), so a configuration
    /// change is reported in the status the guest reads.
    pub fn set_interrupt_status(&mut self, status: Arc<AtomicU32>) {
        self.interrupt_status = Some(status);
    }

    /// Sets the host's target size (in pages) and tells the guest its configuration changed.
    pub fn set_target_pages(&mut self, pages: u32) {
        self.num_pages = pages;
        // The status is recorded before the interrupt is raised, so the guest
        // reads CONFIG when its handler runs.
        if let Some(status) = &self.interrupt_status {
            status.fetch_or(VIRTIO_MMIO_INT_CONFIG, Ordering::SeqCst);
        }
        if let Some(irq) = &self.irq
            && let Err(err) = irq.write(u64::from(VIRTIO_MMIO_INT_CONFIG))
        {
            eprintln!("warning: virtio-balloon: cannot raise the config interrupt: {err}");
        }
    }

    /// The host's target size, in pages.
    pub fn target_pages(&self) -> u32 {
        self.num_pages
    }

    /// The size the guest has given back, in pages.
    pub fn actual_pages(&self) -> u32 {
        self.actual
    }

    /// The guest's memory counters, as last reported on the stats queue.
    pub fn stats(&self) -> BalloonStats {
        let mib = |tag: u16| self.stats.get(&tag).map(|bytes| *bytes >> 20);
        BalloonStats {
            free_mib: mib(STAT_FREE_MEMORY),
            total_mib: mib(STAT_TOTAL_MEMORY),
            available_mib: mib(STAT_AVAILABLE_MEMORY),
            actual_mib: (u64::from(self.actual) << PAGE_SHIFT) >> 20,
        }
    }

    /// Handles a guest write to the configuration space. Only `actual` is the
    /// guest's to write; a write to `num_pages` is dropped.
    pub fn write_config(&mut self, offset: u64, data: &[u8]) {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let mut actual = self.actual.to_le_bytes();
        for (index, byte) in data.iter().enumerate() {
            let pos = start.saturating_add(index);
            if let Some(slot) = pos
                .checked_sub(CONFIG_ACTUAL_OFFSET)
                .and_then(|rel| actual.get_mut(rel))
            {
                *slot = *byte;
            }
        }
        self.actual = u32::from_le_bytes(actual);
    }

    /// The whole configuration space: `num_pages` then `actual`, little-endian.
    fn config_bytes(&self) -> [u8; 2 * CONFIG_ACTUAL_OFFSET] {
        let mut config = [0u8; 2 * CONFIG_ACTUAL_OFFSET];
        config[..CONFIG_ACTUAL_OFFSET].copy_from_slice(&self.num_pages.to_le_bytes());
        config[CONFIG_ACTUAL_OFFSET..].copy_from_slice(&self.actual.to_le_bytes());
        config
    }
}

impl VirtioDevice for VirtioBalloon {
    fn device_type(&self) -> u32 {
        VIRTIO_ID_BALLOON
    }

    fn features(&self) -> u64 {
        VIRTIO_F_VERSION_1
            | VIRTIO_BALLOON_F_STATS_VQ
            | VIRTIO_BALLOON_F_DEFLATE_ON_OOM
            | VIRTIO_BALLOON_F_REPORTING
    }

    fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError> {
        self.acked_features =
            ack_feature_page(self.features(), self.acked_features, page, value)?;
        Ok(())
    }

    fn queues(&self) -> &[QueueConfig] {
        &self.queue_regs
    }

    fn queues_mut(&mut self) -> &mut [QueueConfig] {
        &mut self.queue_regs
    }

    fn activate(&mut self, mem: &GuestMemoryMmap, irq: &EventFd) -> Result<(), VirtioError> {
        for (ring, regs) in self.rings.iter_mut().zip(&self.queue_regs) {
            // The transport has already validated the driver's queue size.
            ring.size = regs.size as u16;
            ring.ready = regs.ready;
            ring.desc_table_address = GuestAddress(regs.desc);
            ring.avail_ring_address = GuestAddress(regs.avail);
            ring.used_ring_address = GuestAddress(regs.used);
            ring.initialize(mem).map_err(BalloonError::from)?;
        }
        self.irq = Some(irq.try_clone().map_err(BalloonError::from)?);
        self.mem = Some(mem.clone());
        Ok(())
    }

    fn write_config(&mut self, offset: u64, data: &[u8]) {
        VirtioBalloon::write_config(self, offset, data);
    }

    fn read_config(&mut self, offset: u64, data: &mut [u8]) {
        let config = self.config_bytes();
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        for (index, byte) in data.iter_mut().enumerate() {
            *byte = config.get(start.saturating_add(index)).copied().unwrap_or(0);
        }
    }

    fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError> {
        // The index is guest-controlled: a queue the device does not have is dropped.
        let Ok(index) = usize::try_from(index) else {
            return Ok(());
        };
        if index >= BALLOON_QUEUE_NUM {
            return Ok(());
        }
        let Self {
            rings,
            mem,
            discard,
            stats,
            irq,
            ..
        } = self;
        let Some(mem) = mem.as_ref() else {
            return Ok(());
        };
        let used = serve(&mut rings[index], index, mem, &mut **discard, stats)?;
        if used
            && rings[index].prepare_kick()
            && let Some(irq) = irq.as_ref()
            && let Err(err) = irq.write(u64::from(VIRTIO_MMIO_INT_VRING))
        {
            eprintln!("warning: virtio-balloon: cannot raise the device interrupt: {err}");
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.rings = std::array::from_fn(|_| Queue::new(BALLOON_QUEUE_SIZE));
        for regs in &mut self.queue_regs {
            regs.reset();
        }
        self.acked_features = 0;
        self.actual = 0;
        self.stats.clear();
        self.mem = None;
        self.irq = None;
    }
}

/// Serves queue `index`. Returns whether any buffer was used.
fn serve(
    queue: &mut Queue,
    index: usize,
    mem: &GuestMemoryMmap,
    discard: &mut dyn Discard,
    stats: &mut BTreeMap<u16, u64>,
) -> Result<bool, BalloonError> {
    match index {
        INFLATE_QUEUE => drain(queue, |chain| inflate(chain, mem, discard)),
        DEFLATE_QUEUE => drain(queue, |_| {}),
        STATS_QUEUE => drain(queue, |chain| record_stats(chain, mem, stats)),
        REPORTING_QUEUE => drain(queue, |chain| report_free(chain, mem, discard)),
        _ => Ok(false),
    }
}

/// Pops every available buffer, hands each chain to `handle`, and returns it to
/// the driver. The length is 0: the device writes nothing into a balloon buffer.
fn drain(queue: &mut Queue, mut handle: impl FnMut(&DescriptorChain)) -> Result<bool, BalloonError> {
    let mut used_any = false;
    while let Some(chain) = queue.pop()? {
        handle(&chain);
        used_any = true;
        if let Err(err) = queue.add_used(chain.index, 0) {
            eprintln!(
                "warning: virtio-balloon: cannot publish descriptor {} in the used ring: {err}",
                chain.index
            );
        }
    }
    queue.advance_used_ring_idx();
    Ok(used_any)
}

/// Each readable descriptor holds little-endian `u32` page frame numbers the
/// guest has given up. Their backing memory is discarded.
fn inflate(chain: &DescriptorChain, mem: &GuestMemoryMmap, discard: &mut dyn Discard) {
    visit_chain(chain, |desc| {
        if desc.is_write_only() {
            return;
        }
        let Some(bytes) = read_buffer(desc, mem) else {
            return;
        };
        let pfns = bytes
            .chunks_exact(4)
            .filter_map(|c| <[u8; 4]>::try_from(c).ok())
            .map(|c| u64::from(u32::from_le_bytes(c)));
        discard_pages(mem, discard, pfns);
    });
}

/// Each descriptor is a free guest-physical range. Only whole pages inside it are discarded.
fn report_free(chain: &DescriptorChain, mem: &GuestMemoryMmap, discard: &mut dyn Discard) {
    visit_chain(chain, |desc| {
        let page = PAGE_SIZE as u64;
        let first = desc.addr.0.div_ceil(page);
        let end = desc.addr.0.saturating_add(u64::from(desc.len)) / page;
        if first < end {
            discard_pages(mem, discard, first..end);
        }
    });
}

/// Each readable descriptor holds `{ tag: u16, val: u64 }` entries. The latest value of each tag is kept.
fn record_stats(chain: &DescriptorChain, mem: &GuestMemoryMmap, stats: &mut BTreeMap<u16, u64>) {
    visit_chain(chain, |desc| {
        if desc.is_write_only() {
            return;
        }
        let Some(bytes) = read_buffer(desc, mem) else {
            return;
        };
        for entry in bytes.chunks_exact(STAT_ENTRY_SIZE) {
            if let Some((tag, val)) = parse_stat(entry) {
                stats.insert(tag, val);
            }
        }
    });
}

/// Parses one `{ tag: u16, val: u64 }` entry.
fn parse_stat(entry: &[u8]) -> Option<(u16, u64)> {
    let tag = <[u8; 2]>::try_from(entry.get(0..2)?).ok()?;
    let val = <[u8; 8]>::try_from(entry.get(2..10)?).ok()?;
    Some((u16::from_le_bytes(tag), u64::from_le_bytes(val)))
}

/// Calls `visit` for each descriptor of `chain`, head first.
fn visit_chain(chain: &DescriptorChain, mut visit: impl FnMut(&DescriptorChain)) {
    visit(chain);
    let mut next = chain.next_descriptor();
    while let Some(desc) = next {
        visit(&desc);
        next = desc.next_descriptor();
    }
}

/// The bytes of a descriptor, or `None` if it is too large or not in guest memory.
fn read_buffer(desc: &DescriptorChain, mem: &GuestMemoryMmap) -> Option<Vec<u8>> {
    let len = usize::try_from(desc.len).ok()?;
    if len > MAX_BUFFER_BYTES {
        eprintln!("warning: virtio-balloon: rejecting a {len}-byte buffer");
        return None;
    }
    let mut bytes = vec![0u8; len];
    mem.read_slice(&mut bytes, desc.addr).ok()?;
    Some(bytes)
}

/// Discards the host memory behind the page frames `pfns`. Frames that are
/// adjacent in host memory are discarded with one call; a frame outside guest
/// memory is skipped.
fn discard_pages(mem: &GuestMemoryMmap, discard: &mut dyn Discard, pfns: impl Iterator<Item = u64>) {
    // The run being built: (host address, length in bytes).
    let mut run: Option<(usize, usize)> = None;
    for pfn in pfns {
        let Ok(host) = mem.get_host_address(GuestAddress(pfn << PAGE_SHIFT)) else {
            flush_run(&mut run, discard);
            continue;
        };
        let host = host as usize;
        let extends = matches!(run, Some((start, len)) if start + len == host);
        if extends {
            if let Some((_, len)) = run.as_mut() {
                *len += PAGE_SIZE;
            }
        } else {
            flush_run(&mut run, discard);
            run = Some((host, PAGE_SIZE));
        }
    }
    flush_run(&mut run, discard);
}

/// Discards the pending run, if any.
fn flush_run(run: &mut Option<(usize, usize)>, discard: &mut dyn Discard) {
    if let Some((host, len)) = run.take()
        && let Err(err) = discard.discard(host as *mut u8, len)
    {
        eprintln!("warning: virtio-balloon: cannot discard {len} bytes of guest memory: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::balloon::STAT_SWAP_IN;
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};

    /// Records each discard instead of performing it.
    #[derive(Default)]
    struct Recorder {
        calls: Vec<(usize, usize)>,
    }

    impl Discard for Recorder {
        fn discard(&mut self, host: *mut u8, len: usize) -> io::Result<()> {
            self.calls.push((host as usize, len));
            Ok(())
        }
    }

    /// Guest address of the test buffers; the rings sit at guest address 0.
    const BUFFER: u64 = 0x8000;

    /// Writes `data` at [`BUFFER`] and publishes it as descriptor `head`.
    fn publish_buffer(mem: &GuestMemoryMmap, vq: &VirtQueue, head: u16, data: &[u8]) {
        mem.write_slice(data, GuestAddress(BUFFER)).unwrap();
        vq.set_desc(head, BUFFER, data.len() as u32, 0, 0);
        vq.publish(head);
    }

    #[test]
    fn balloon_config_space_reports_target_and_takes_actual_from_the_guest() {
        let mut balloon = VirtioBalloon::new();
        balloon.set_target_pages(0x0012_3456);

        let mut target = [0u8; 4];
        balloon.read_config(0, &mut target);
        assert_eq!(u32::from_le_bytes(target), 0x0012_3456);

        balloon.write_config(4, &500u32.to_le_bytes());
        assert_eq!(balloon.actual_pages(), 500);
        let mut actual = [0u8; 4];
        balloon.read_config(4, &mut actual);
        assert_eq!(u32::from_le_bytes(actual), 500);

        // The host owns the target: a guest write to it is dropped.
        balloon.write_config(0, &7u32.to_le_bytes());
        assert_eq!(balloon.target_pages(), 0x0012_3456);
    }

    #[test]
    fn balloon_target_change_sets_the_config_interrupt_in_the_shared_status() {
        let status = Arc::new(AtomicU32::new(0));
        let mut balloon = VirtioBalloon::new();
        balloon.set_interrupt_status(Arc::clone(&status));

        balloon.set_target_pages(42);
        assert_eq!(status.load(Ordering::SeqCst), VIRTIO_MMIO_INT_CONFIG);

        // The guest's configuration write goes through the trait to `actual`.
        VirtioDevice::write_config(&mut balloon, 4, &7u32.to_le_bytes());
        assert_eq!(balloon.actual_pages(), 7);
    }

    #[test]
    fn balloon_inflate_discards_the_pfn_runs_coalesced() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let mut queue = vq.create_queue();

        // Frames 4..=7 are adjacent in host memory; frame 9 is not.
        let pfns: [u32; 5] = [4, 5, 6, 7, 9];
        let bytes: Vec<u8> = pfns.iter().flat_map(|pfn| pfn.to_le_bytes()).collect();
        publish_buffer(&mem, &vq, 0, &bytes);

        let mut recorder = Recorder::default();
        serve(&mut queue, INFLATE_QUEUE, &mem, &mut recorder, &mut BTreeMap::new()).unwrap();

        let host = |pfn: u64| mem.get_host_address(GuestAddress(pfn << PAGE_SHIFT)).unwrap() as usize;
        assert_eq!(
            recorder.calls,
            vec![(host(4), 4 * PAGE_SIZE), (host(9), PAGE_SIZE)]
        );
        assert_eq!(vq.used_idx(), 1);
        assert_eq!(vq.used_elem(0), (0, 0));
    }

    #[test]
    fn balloon_deflate_returns_the_buffers_without_discarding_anything() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let mut queue = vq.create_queue();

        publish_buffer(&mem, &vq, 0, &[0u8; 8]);
        publish_buffer(&mem, &vq, 1, &[0u8; 4]);

        let mut recorder = Recorder::default();
        serve(&mut queue, DEFLATE_QUEUE, &mem, &mut recorder, &mut BTreeMap::new()).unwrap();

        assert!(recorder.calls.is_empty());
        assert_eq!(vq.used_idx(), 2);
        assert_eq!(vq.used_elem(0), (0, 0));
        assert_eq!(vq.used_elem(1), (1, 0));
    }

    #[test]
    fn balloon_stats_queue_keeps_the_latest_value_per_tag_and_returns_the_buffer() {
        fn encode(entries: &[(u16, u64)]) -> Vec<u8> {
            let mut out = Vec::new();
            for (tag, val) in entries {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&val.to_le_bytes());
            }
            out
        }

        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let mut queue = vq.create_queue();
        let mut balloon = VirtioBalloon::new();

        publish_buffer(
            &mem,
            &vq,
            0,
            &encode(&[
                (STAT_FREE_MEMORY, 2 << 30),
                (STAT_TOTAL_MEMORY, 4 << 30),
                (STAT_AVAILABLE_MEMORY, 3 << 30),
                (STAT_SWAP_IN, 9),
            ]),
        );
        serve(&mut queue, STATS_QUEUE, &mem, &mut Recorder::default(), &mut balloon.stats)
            .unwrap();

        // A later report replaces the free-memory value and leaves the others.
        publish_buffer(&mem, &vq, 0, &encode(&[(STAT_FREE_MEMORY, 1 << 30)]));
        serve(&mut queue, STATS_QUEUE, &mem, &mut Recorder::default(), &mut balloon.stats)
            .unwrap();

        assert_eq!(vq.used_idx(), 2);
        assert_eq!(vq.used_elem(1), (0, 0));

        // 256 Ki pages given back is 1 GiB.
        balloon.write_config(4, &(256 * 1024u32).to_le_bytes());
        assert_eq!(
            balloon.stats(),
            BalloonStats {
                free_mib: Some(1024),
                total_mib: Some(4096),
                available_mib: Some(3072),
                actual_mib: 1024,
            }
        );
    }
}
