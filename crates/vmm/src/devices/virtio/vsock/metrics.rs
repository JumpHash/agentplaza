// Copyright 2023 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/devices/virtio/vsock/metrics.rs
// (trimmed: this port has no `logger` module and no metric flushing, so the
// `SharedIncMetric` plumbing collapses into a single `AtomicU64` counter. The
// field list and the `METRICS` static stay, so every upstream call site
// remains a one-liner.)

//! Vsock device metrics, reduced to no-op atomics.
//!
//! Firecracker feeds these through `logger::METRICS` and serializes them to
//! JSON on demand. This port has neither: the counters are incremented at the
//! same call sites and read by the port's own tests, but never flushed.

use std::sync::atomic::{AtomicU64, Ordering};

/// A monotonically increasing counter (`SharedIncMetric` in Firecracker).
#[derive(Debug, Default)]
pub struct SharedIncMetric(AtomicU64);

impl SharedIncMetric {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }
}

/// The increment/read surface the vsock code uses on every metric.
pub trait IncMetric {
    /// Increment by one.
    fn inc(&self);
    /// Add `value` to the counter.
    fn add(&self, value: u64);
    /// Current value.
    fn count(&self) -> u64;
}

impl IncMetric for SharedIncMetric {
    fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn add(&self, value: u64) {
        self.0.fetch_add(value, Ordering::Relaxed);
    }

    fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// The vsock bytes this process has moved, as `(rx, tx)`: rx is what the hub sent the guest, tx what the guest sent
/// the hub.
///
/// The counters are process-wide. The node runs each VM in its own process, so for it they are the VM's own; VMs
/// that share a process share these.
pub fn byte_totals() -> (u64, u64) {
    (
        METRICS.rx_bytes_count.count(),
        METRICS.tx_bytes_count.count(),
    )
}

/// Aggregate metrics of all vsock connections/actions (one vsock device per
/// VM, so no per-device split).
pub(super) static METRICS: VsockDeviceMetrics = VsockDeviceMetrics::new();

/// Vsock-related metrics. Field-for-field upstream's; the device frontend only
/// touches `activate_fails`, `cfg_fails` and `ev_queue_event_fails`, the rest
/// are for the muxer backend that lands with the next slice.
#[allow(dead_code)] // most fields are written by the muxer backend, not yet landed
#[derive(Debug)]
pub(super) struct VsockDeviceMetrics {
    pub activate_fails: SharedIncMetric,
    pub cfg_fails: SharedIncMetric,
    pub rx_queue_event_fails: SharedIncMetric,
    pub tx_queue_event_fails: SharedIncMetric,
    pub ev_queue_event_fails: SharedIncMetric,
    pub muxer_event_fails: SharedIncMetric,
    pub conn_event_fails: SharedIncMetric,
    pub rx_queue_event_count: SharedIncMetric,
    pub tx_queue_event_count: SharedIncMetric,
    pub rx_bytes_count: SharedIncMetric,
    pub tx_bytes_count: SharedIncMetric,
    pub rx_packets_count: SharedIncMetric,
    pub tx_packets_count: SharedIncMetric,
    pub conns_added: SharedIncMetric,
    pub conns_killed: SharedIncMetric,
    pub conns_removed: SharedIncMetric,
    pub killq_resync: SharedIncMetric,
    pub tx_flush_fails: SharedIncMetric,
    pub tx_write_fails: SharedIncMetric,
    pub rx_read_fails: SharedIncMetric,
}

impl VsockDeviceMetrics {
    const fn new() -> Self {
        Self {
            activate_fails: SharedIncMetric::new(),
            cfg_fails: SharedIncMetric::new(),
            rx_queue_event_fails: SharedIncMetric::new(),
            tx_queue_event_fails: SharedIncMetric::new(),
            ev_queue_event_fails: SharedIncMetric::new(),
            muxer_event_fails: SharedIncMetric::new(),
            conn_event_fails: SharedIncMetric::new(),
            rx_queue_event_count: SharedIncMetric::new(),
            tx_queue_event_count: SharedIncMetric::new(),
            rx_bytes_count: SharedIncMetric::new(),
            tx_bytes_count: SharedIncMetric::new(),
            rx_packets_count: SharedIncMetric::new(),
            tx_packets_count: SharedIncMetric::new(),
            conns_added: SharedIncMetric::new(),
            conns_killed: SharedIncMetric::new(),
            conns_removed: SharedIncMetric::new(),
            killq_resync: SharedIncMetric::new(),
            tx_flush_fails: SharedIncMetric::new(),
            tx_write_fails: SharedIncMetric::new(),
            rx_read_fails: SharedIncMetric::new(),
        }
    }
}
