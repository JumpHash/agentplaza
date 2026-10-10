// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: Firecracker drives the userspace vsock device from
// its shared `event_manager` loop, which this tree does not have. This module
// is the replacement - one dedicated epoll thread per VM that owns a vsock
// device, mirroring upstream's `vsock/event_handler.rs` subscriber logic
// (`handle_rxq_event`/`handle_txq_event`/`handle_evq_event`/`notify_backend`)
// against a private epoll set instead of the event manager.

//! The per-VM vsock event thread.
//!
//! The device's virtqueue processing never runs on a vCPU thread: a guest
//! `QueueNotify` write is only turned into an eventfd write by
//! [`Vsock::notify_queue`], and this thread - the VM's only consumer of those
//! eventfds and of the muxer's nested epoll fd - does all ring processing,
//! then raises the device interrupt through the irq eventfd the mmio transport
//! owns (the same one registered as the layout's KVM `irqfd`).

use std::fmt::Debug;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex, PoisonError};

use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use vmm_sys_util::eventfd::EventFd;

use super::device::{EVQ_INDEX, RXQ_INDEX, TXQ_INDEX, Vsock};
use super::metrics::{IncMetric, METRICS};
use super::VsockBackend;

/// Poll-set ids. The queue ids are `RXQ`/`TXQ`/`EVQ`; the backend fd is
/// registered with [`BACKEND`], the device's activation event with [`ACTIVATE`]
/// and the VM's exit event with [`EXIT`].
const ACTIVATE: u64 = 0;
const RXQ: u64 = 1;
const TXQ: u64 = 2;
const EVQ: u64 = 3;
const BACKEND: u64 = 4;
const EXIT: u64 = 5;

/// Spawns the event thread for a VM's vsock device.
///
/// `exit_evt` is a clone of the VM's shared exit event: when a vCPU stops the
/// VM (or the spawner's stop control does), the eventfd becomes readable and
/// this thread exits *without* consuming it - the VM's exit watcher is the
/// only reader, and a second reader would steal its wakeup.
pub fn start_event_thread<B>(
    device: Arc<Mutex<Vsock<B>>>,
    exit_evt: EventFd,
    vm_id: u32,
) -> std::io::Result<std::thread::JoinHandle<()>>
where
    B: VsockBackend + Debug + 'static,
{
    std::thread::Builder::new()
        .name(format!("apz_vm{vm_id}_vsock"))
        .spawn(move || run(device, exit_evt))
}

fn lock<B>(device: &Arc<Mutex<Vsock<B>>>) -> std::sync::MutexGuard<'_, Vsock<B>> {
    device
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn register(epoll: &Epoll, fd: std::os::fd::RawFd, data: u64, evset: EventSet) {
    if let Err(err) = epoll.ctl(ControlOperation::Add, fd, EpollEvent::new(evset, data)) {
        eprintln!("vsock: failed to register fd {fd} in the event thread: {err}");
    }
}

fn unregister(epoll: &Epoll, fd: std::os::fd::RawFd) {
    if let Err(err) = epoll.ctl(ControlOperation::Delete, fd, EpollEvent::default()) {
        eprintln!("vsock: failed to unregister fd {fd} in the event thread: {err}");
    }
}

/// Consume a queue eventfd; nothing here may fail the thread, since a spurious
/// or lost wakeup only costs a later re-poll.
fn drain(event: &EventFd) {
    let _ = event.read();
}

/// RX queue event: the driver made new RX buffers available. Deliver any
/// pending host data into them.
fn handle_rxq<B: VsockBackend + Debug>(device: &mut Vsock<B>, evset: EventSet) -> Vec<usize> {
    if evset != EventSet::IN {
        eprintln!("vsock: rxq unexpected event {evset:?}");
        METRICS.rx_queue_event_fails.inc();
        return Vec::new();
    }

    if device.queue_events[RXQ_INDEX].read().is_err() {
        METRICS.rx_queue_event_fails.inc();
    } else if device.backend().has_pending_rx() && device.process_rx().unwrap_or(false) {
        return vec![RXQ_INDEX];
    }
    Vec::new()
}

/// TX queue event: the driver handed us packets to forward to the host.
fn handle_txq<B: VsockBackend + Debug>(device: &mut Vsock<B>, evset: EventSet) -> Vec<usize> {
    if evset != EventSet::IN {
        eprintln!("vsock: txq unexpected event {evset:?}");
        METRICS.tx_queue_event_fails.inc();
        return Vec::new();
    }

    let mut used = Vec::new();
    if device.queue_events[TXQ_INDEX].read().is_err() {
        METRICS.tx_queue_event_fails.inc();
    } else {
        if device.process_tx().unwrap_or(false) {
            used.push(TXQ_INDEX);
        }
        // The backend may have queued responses to what we just sent; fetch
        // them into RX buffers while we are here.
        if device.backend().has_pending_rx() && device.process_rx().unwrap_or(false) {
            used.push(RXQ_INDEX);
        }
    }
    used
}

/// Event queue event: the driver acknowledged a TRANSPORT_RESET.
fn handle_evq<B: VsockBackend + Debug>(device: &mut Vsock<B>, evset: EventSet) -> Vec<usize> {
    if evset != EventSet::IN {
        eprintln!("vsock: evq unexpected event {evset:?}");
        METRICS.ev_queue_event_fails.inc();
        return Vec::new();
    }

    if device.queue_events[EVQ_INDEX].read().is_err() {
        METRICS.ev_queue_event_fails.inc();
    }

    // The guest's evq kick is the TRANSPORT_RESET ack; clear the gate and drain
    // any RX the backend buffered while it was up.
    device.pending_event_ack = false;
    if device.backend().has_pending_rx() && device.process_rx().unwrap_or(false) {
        return vec![RXQ_INDEX];
    }
    Vec::new()
}

/// Backend event: a host-side fd became readable (data, or a host connection).
fn handle_backend<B: VsockBackend + Debug>(device: &mut Vsock<B>, evset: EventSet) -> Vec<usize> {
    let mut used = Vec::new();
    device.backend_mut().notify(evset);
    if device.process_tx().unwrap_or(false) {
        used.push(TXQ_INDEX);
    }
    if device.backend().has_pending_rx() && device.process_rx().unwrap_or(false) {
        used.push(RXQ_INDEX);
    }
    used
}

fn run<B>(device: Arc<Mutex<Vsock<B>>>, exit_evt: EventFd)
where
    B: VsockBackend + Debug,
{
    let epoll = match Epoll::new() {
        Ok(epoll) => epoll,
        Err(err) => {
            eprintln!("vsock: cannot create the event thread's epoll fd: {err}");
            return;
        }
    };

    // The exit event is polled for the thread's whole life; the activation
    // event only until the driver activates the device (at which point the
    // queue/backend fds take its place, mirroring upstream's init/activate
    // split).
    register(&epoll, exit_evt.as_raw_fd(), EXIT, EventSet::IN);
    let activate_fd = {
        let dev = lock(&device);
        let evt = dev.activate_evt();
        register(&epoll, evt.as_raw_fd(), ACTIVATE, EventSet::IN);
        evt.as_raw_fd()
    };

    let mut activated = false;
    let mut events = [EpollEvent::new(EventSet::empty(), 0); 8];

    loop {
        let count = match epoll.wait(-1, &mut events) {
            Ok(count) => count,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("vsock: event thread poll failed: {err}");
                return;
            }
        };

        for event in &events[..count] {
            match event.data() {
                EXIT => return,
                ACTIVATE => {
                    let dev = lock(&device);
                    drain(dev.activate_evt());
                    for (index, queue_event) in dev.queue_events().iter().enumerate() {
                        let data = match index {
                            RXQ_INDEX => RXQ,
                            TXQ_INDEX => TXQ,
                            EVQ_INDEX => EVQ,
                            _ => continue,
                        };
                        register(&epoll, queue_event.as_raw_fd(), data, EventSet::IN);
                    }
                    register(
                        &epoll,
                        dev.backend().as_raw_fd(),
                        BACKEND,
                        dev.backend().get_polled_evset(),
                    );
                    unregister(&epoll, activate_fd);
                    activated = true;
                }
                _ if !activated => {
                    // A queue notification before the driver activated the
                    // device: consume it, it can only be a hostile/buggy
                    // driver poking QueueNotify early.
                    let dev = lock(&device);
                    for queue_event in dev.queue_events() {
                        drain(queue_event);
                    }
                }
                source => {
                    let mut dev = lock(&device);
                    let used = match source {
                        RXQ => handle_rxq(&mut dev, event.event_set()),
                        TXQ => handle_txq(&mut dev, event.event_set()),
                        EVQ => handle_evq(&mut dev, event.event_set()),
                        BACKEND => handle_backend(&mut dev, event.event_set()),
                        _ => Vec::new(),
                    };
                    if !used.is_empty() && dev.signal_used_queue(0).is_err() {
                        eprintln!("vsock: failed to raise the device interrupt");
                    }
                }
            }
        }
    }
}
