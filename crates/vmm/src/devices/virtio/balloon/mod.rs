// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: a virtio-balloon device (virtio 1.x §5.5, device id 5)
// on the MMIO transport. Its queues are served synchronously on the vCPU thread
// that notifies them, as the block device's are; the only host effect is
// `madvise(MADV_DONTNEED)` over the pages the guest gives back.

//! The virtio-balloon device: the host sets a memory target, the guest gives
//! pages back by inflating, and the host discards their backing memory.
//!
//! Four queues: inflate (the guest hands over page frame numbers it does not
//! use), deflate (the guest takes pages back; they fault in again on use),
//! stats (the guest reports its memory counters) and free-page reporting (the
//! guest reports free ranges the host may discard).

pub mod device;

use std::io;

use crate::devices::virtio::queue::{InvalidAvailIdx, QueueError};

pub use device::{Discard, MadviseDiscard, SharedBalloon, VirtioBalloon};

/// `VIRTIO_ID_BALLOON`: the virtio type id of a balloon device.
pub const VIRTIO_ID_BALLOON: u32 = 5;

/// `VIRTIO_BALLOON_F_STATS_VQ` (feature bit 1): the stats queue exists.
pub const VIRTIO_BALLOON_F_STATS_VQ: u64 = 1 << 1;
/// `VIRTIO_BALLOON_F_DEFLATE_ON_OOM` (feature bit 2): the guest may deflate under memory pressure.
pub const VIRTIO_BALLOON_F_DEFLATE_ON_OOM: u64 = 1 << 2;
/// `VIRTIO_BALLOON_F_REPORTING` (feature bit 5): the free-page reporting queue exists.
pub const VIRTIO_BALLOON_F_REPORTING: u64 = 1 << 5;

/// Number of virtqueues: inflate, deflate, stats, reporting.
pub const BALLOON_QUEUE_NUM: usize = 4;
/// Queue index of the inflate queue.
pub const INFLATE_QUEUE: usize = 0;
/// Queue index of the deflate queue.
pub const DEFLATE_QUEUE: usize = 1;
/// Queue index of the stats queue.
pub const STATS_QUEUE: usize = 2;
/// Queue index of the free-page reporting queue.
pub const REPORTING_QUEUE: usize = 3;
/// Ring size of every balloon queue.
pub const BALLOON_QUEUE_SIZE: u16 = 256;

/// Size of a guest page, and its shift: the balloon counts memory in 4 KiB pages.
pub const PAGE_SHIFT: u32 = 12;
/// Size of a guest page in bytes.
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

/// Stats tag: memory swapped in (bytes).
pub const STAT_SWAP_IN: u16 = 0;
/// Stats tag: memory swapped out (bytes).
pub const STAT_SWAP_OUT: u16 = 1;
/// Stats tag: major page faults.
pub const STAT_MAJOR_FAULTS: u16 = 2;
/// Stats tag: minor page faults.
pub const STAT_MINOR_FAULTS: u16 = 3;
/// Stats tag: free memory (bytes).
pub const STAT_FREE_MEMORY: u16 = 4;
/// Stats tag: total memory (bytes).
pub const STAT_TOTAL_MEMORY: u16 = 5;
/// Stats tag: memory available without swapping (bytes).
pub const STAT_AVAILABLE_MEMORY: u16 = 6;
/// Stats tag: disk caches (bytes).
pub const STAT_DISK_CACHES: u16 = 7;

/// The guest's memory counters, in MiB. A counter the guest has not reported is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BalloonStats {
    /// Free memory.
    pub free_mib: Option<u64>,
    /// Total memory.
    pub total_mib: Option<u64>,
    /// Memory available without swapping.
    pub available_mib: Option<u64>,
    /// Memory the guest has given back to the host (its `actual` size).
    pub actual_mib: u64,
}

/// A balloon queue or interrupt failed.
#[derive(Debug, thiserror::Error)]
pub enum BalloonError {
    /// A queue could not be initialized from the driver's registers: {0}
    #[error("cannot initialize a balloon queue: {0}")]
    Queue(#[from] QueueError),
    /// The driver asked for more buffers than its ring holds: {0}
    #[error("balloon queue: {0}")]
    AvailIdx(#[from] InvalidAvailIdx),
    /// The device interrupt could not be created: {0}
    #[error("cannot create the balloon interrupt: {0}")]
    Irq(#[from] io::Error),
    /// A snapshot carries {got} queue positions; the balloon has {expected} (none before activation)
    #[error("snapshot has {got} queue positions; the balloon has {expected}")]
    SnapshotQueueCount {
        /// The queue positions the device expects.
        expected: usize,
        /// The queue positions the snapshot carries.
        got: usize,
    },
    /// The balloon's snapshot state is malformed: {0}
    #[error("malformed balloon snapshot state: {0}")]
    SnapshotState(#[from] serde_json::Error),
}
