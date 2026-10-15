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

use crate::pal::AsRawFd;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::pal::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use crate::pal::eventfd::EventFd;

use super::VsockBackend;
use super::device::{EVQ_INDEX, RXQ_INDEX, TXQ_INDEX, Vsock};
use super::metrics::{IncMetric, METRICS};
use crate::control::DeviceGate;

/// Poll-set ids. The queue ids are `RXQ`/`TXQ`/`EVQ`; the backend fd is
/// registered with [`BACKEND`], the device's activation event with [`ACTIVATE`]
/// and the VM's exit event with [`EXIT`].
const ACTIVATE: u64 = 0;
const RXQ: u64 = 1;
const TXQ: u64 = 2;
const EVQ: u64 = 3;
const BACKEND: u64 = 4;
const EXIT: u64 = 5;

/// How long the event thread's `epoll_wait` blocks before re-checking the
/// shared exit flag, in milliseconds.
///
/// The exit event fd is only a wakeup hint: the exit watcher is its sole
/// reader, so a count the watcher consumed before this thread revalidated the
/// fd (level-triggered `epoll_wait` re-polls the eventfd) is silently lost,
/// and `-1` would then block forever. The flag checked after every wait is the
/// source of truth; this bound is what guarantees the flag is seen even when
/// the wakeup is lost. It bounds exit latency, not throughput: while events
/// arrive the wait returns immediately.
const EXIT_POLL_TIMEOUT_MS: i32 = 200;

/// Spawns the event thread for a VM's vsock device.
///
/// `exit_evt` is a clone of the VM's shared exit event: when a vCPU stops the
/// VM (or the spawner's stop control does), the eventfd becomes readable and
/// this thread exits *without* consuming it - the VM's exit watcher is the
/// only reader, and a second reader would steal its wakeup.
///
/// `exit_signalled` is the shared flag every exit path sets before it writes
/// that event. Because a shared wakeup can be consumed by the watcher before
/// this thread revalidates the fd, the flag - not the event fd - is the source
/// of truth: it is checked on every iteration of the run loop, so a lost
/// wakeup costs at most one [`EXIT_POLL_TIMEOUT_MS`].
///
/// `gate` is the VM's device gate ([`DeviceGate`]). The thread holds its read
/// half for each processing pass — from the moment the epoll wait returns until
/// the events are handled — and never while it waits. A snapshot holds the write
/// half from before it saves the device state until the memory is written, so
/// no packet can land between the two; the thread then blocks on the gate, not
/// on the device lock, and resumes when the snapshot is done.
pub fn start_event_thread<B>(
    device: Arc<Mutex<Vsock<B>>>,
    exit_evt: EventFd,
    exit_signalled: Arc<AtomicBool>,
    gate: DeviceGate,
    vm_id: u32,
) -> std::io::Result<std::thread::JoinHandle<()>>
where
    B: VsockBackend + Debug + 'static,
{
    std::thread::Builder::new()
        .name(format!("apz_vm{vm_id}_vsock"))
        .spawn(move || {
            // The jail goes on before this thread's epoll loop: the muxer's
            // socket and epoll are set up after this point, and they are on
            // the allowlist. A real install error is fail-closed; a platform
            // with no seccomp at all is reported and tolerated (see
            // `crate::jail`).
            #[cfg(target_os = "linux")]
            if let Err(err) = crate::jail::Jail::arm_thread() {
                eprintln!("vsock: cannot arm the seccomp jail: {err}");
                return;
            }
            run(device, exit_evt, exit_signalled, gate)
        })
}

fn lock<B>(device: &Arc<Mutex<Vsock<B>>>) -> std::sync::MutexGuard<'_, Vsock<B>> {
    device.lock().unwrap_or_else(PoisonError::into_inner)
}

fn register(epoll: &Epoll, fd: crate::pal::RawFd, data: u64, evset: EventSet) {
    if let Err(err) = epoll.ctl(ControlOperation::Add, fd, EpollEvent::new(evset, data)) {
        eprintln!("vsock: failed to register fd {fd} in the event thread: {err}");
    }
}

fn unregister(epoll: &Epoll, fd: crate::pal::RawFd) {
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

fn run<B>(
    device: Arc<Mutex<Vsock<B>>>,
    exit_evt: EventFd,
    exit_signalled: Arc<AtomicBool>,
    gate: DeviceGate,
) where
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
        // The exit flag is the source of truth, checked before and after every
        // wait: the exit watcher is the exit event fd's only reader, so a
        // wakeup meant for this thread can be consumed before it revalidates
        // the fd (a level-triggered epoll then reports nothing), and `-1`
        // would strand the thread - and with it the muxer, its bound UDS path
        // and its copy of the guest memory - until someone wrote the fd again.
        // The bounded timeout is the backstop that makes a lost wakeup cost at
        // most one iteration, whether this thread is inside or outside the
        // wait when the exit lands.
        if exit_signalled.load(Ordering::SeqCst) {
            return;
        }
        let count = match epoll.wait(EXIT_POLL_TIMEOUT_MS, &mut events) {
            Ok(count) => count,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("vsock: event thread poll failed: {err}");
                return;
            }
        };
        // Re-check before acting on the events: an exit that landed while this
        // thread was returning from the wait must not be processed as a queue
        // event.
        if exit_signalled.load(Ordering::SeqCst) {
            return;
        }

        // One processing pass holds the device gate's read half, so a snapshot
        // that holds the write half sees either all of a pass or none of it.
        // The guard lives to the end of this iteration; the wait above is not
        // under it.
        let _pass = gate.read().unwrap_or_else(PoisonError::into_inner);
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
                    // The device state is not this thread's to cache: the
                    // transport's reset path (`VsockMmioDevice::reset` →
                    // `Vsock::reset`, a driver status write of 0) clears
                    // `activated`/`mem` without telling this thread, while the
                    // queue event fds keep the poll registrations they got at
                    // ACTIVATE. A kick written just before such a reset
                    // therefore wakes this thread against an inactive device.
                    // Upstream's event handler re-checked the live
                    // `is_activated()` on every event; do the same here — drain
                    // the queue events (so their counts do not accumulate) and
                    // skip ring processing, which would otherwise hit
                    // `process_rx`/`process_tx`'s inactive-device guard and
                    // panic this thread (killing the VM's vsock for the rest of
                    // its life). The check and the dispatch below hold the same
                    // device lock the reset path needs, so no reset can land
                    // between them.
                    if !dev.is_activated() {
                        for queue_event in dev.queue_events() {
                            drain(queue_event);
                        }
                        continue;
                    }
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

#[cfg(test)]
mod tests {
    use crate::pal::RawFd;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::devices::virtio::vsock::packet::{VsockPacketRx, VsockPacketTx};
    use crate::devices::virtio::vsock::{VsockChannel, VsockEpollListener, VsockError};
    use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

    /// A backend with no traffic: enough to build the device the event thread
    /// polls, and to observe the thread's exit behaviour without any guest.
    #[derive(Debug)]
    struct IdleBackend {
        /// The fd the event thread registers as the backend's. A real, quiet
        /// event fd (never written) rather than fd 0: a test that activates
        /// the device makes the thread poll this fd, and fd 0 — a pipe or
        /// `/dev/null` under the test harness — would be permanently readable
        /// and spin the event loop.
        poll: EventFd,
    }

    impl Default for IdleBackend {
        fn default() -> Self {
            Self {
                poll: EventFd::new(crate::pal::eventfd::EFD_NONBLOCK)
                    .expect("an idle backend event fd"),
            }
        }
    }

    impl VsockChannel for IdleBackend {
        fn recv_pkt(&mut self, _pkt: &mut VsockPacketRx) -> Result<(), VsockError> {
            Err(VsockError::NoData)
        }

        fn send_pkt(&mut self, _pkt: &VsockPacketTx) {}

        fn has_pending_rx(&self) -> bool {
            false
        }
    }

    impl VsockEpollListener for IdleBackend {
        fn get_polled_evset(&self) -> EventSet {
            EventSet::IN
        }

        fn notify(&mut self, _evset: EventSet) {}
    }

    impl VsockBackend for IdleBackend {
        fn activate(&mut self) -> Result<(), VsockError> {
            Ok(())
        }

        fn reset(&mut self) {}
    }

    impl AsRawFd for IdleBackend {
        fn as_raw_fd(&self) -> RawFd {
            self.poll.as_raw_fd()
        }
    }

    /// A device with three empty queues and an idle backend, never activated.
    fn idle_device(mem: &GuestMemoryMmap) -> Arc<Mutex<Vsock<IdleBackend>>> {
        Arc::new(Mutex::new(
            Vsock::with_queues(3, IdleBackend::default(), three_empty_queues(mem))
                .expect("an idle vsock device"),
        ))
    }

    /// The same device, activated against `mem` (as the transport does when the
    /// driver writes the last status byte): the event thread then registers the
    /// three queue event fds and the backend fd and processes rings.
    fn activated_device(mem: &GuestMemoryMmap) -> Arc<Mutex<Vsock<IdleBackend>>> {
        let mut device = Vsock::with_queues(3, IdleBackend::default(), three_empty_queues(mem))
            .expect("an idle vsock device");
        let irq = EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).expect("an interrupt event fd");
        device
            .activate((*mem).clone(), irq)
            .expect("the idle device activates");
        Arc::new(Mutex::new(device))
    }

    /// Three empty queues (RX, TX, EV) laid out in `mem`.
    fn three_empty_queues(mem: &GuestMemoryMmap) -> Vec<crate::devices::virtio::queue::Queue> {
        [
            VirtQueue::new(GuestAddress(0), mem, 16),
            VirtQueue::new(GuestAddress(0x2000), mem, 16),
            VirtQueue::new(GuestAddress(0x4000), mem, 16),
        ]
        .iter()
        .map(VirtQueue::create_queue)
        .collect()
    }

    /// Joins `handle`, returning `false` if it is still running after
    /// `timeout` (rather than blocking the test forever on a stuck thread).
    fn joined_within(handle: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        handle.join().is_ok()
    }

    /// F1 regression: a lost exit wakeup must not strand the thread.
    ///
    /// This is the exit watcher winning the race exactly as the review
    /// describes: the exit path published the shared flag and notified the
    /// event fd, but the watcher - the fd's only reader - consumed the count
    /// before this thread revalidated it, so a level-triggered `epoll_wait`
    /// reports nothing and the thread sees no exit event. Modelling the lost
    /// count as "the fd is never written again", the thread must still observe
    /// the exit through the flag within one poll timeout. With the pre-F1 code
    /// (no flag, `epoll.wait(-1)`) it blocks forever.
    #[test]
    fn exit_is_observed_when_the_event_fd_wakeup_is_lost() {
        let mem = default_mem();
        let device = idle_device(&mem);
        let exit_evt = EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).unwrap();
        let exit_signalled = Arc::new(AtomicBool::new(false));
        let handle = start_event_thread(
            device,
            exit_evt.try_clone().unwrap(),
            Arc::clone(&exit_signalled),
            DeviceGate::default(),
            0,
        )
        .expect("the event thread starts");

        // Let the thread reach its wait, then run the exit path. Only the flag
        // is set: the event fd's count was already consumed (by the watcher).
        std::thread::sleep(Duration::from_millis(50));
        exit_signalled.store(true, Ordering::SeqCst);

        assert!(
            joined_within(handle, Duration::from_secs(2)),
            "the event thread did not observe the exit after its event-fd wakeup was consumed"
        );
    }

    /// The event fd wakeup still ends the thread on its own: the normal path,
    /// where this thread sees the readable fd before the watcher consumes it.
    #[test]
    fn the_event_fd_wakeup_ends_the_thread() {
        let mem = default_mem();
        let device = idle_device(&mem);
        let exit_evt = EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).unwrap();
        let exit_signalled = Arc::new(AtomicBool::new(false));
        let handle = start_event_thread(
            device,
            exit_evt.try_clone().unwrap(),
            Arc::clone(&exit_signalled),
            DeviceGate::default(),
            0,
        )
        .expect("the event thread starts");

        // Both halves of the exit path: the flag the watcher publishes, and
        // the event fd write that wakes the thread's own poll.
        std::thread::sleep(Duration::from_millis(50));
        exit_signalled.store(true, Ordering::SeqCst);
        exit_evt.write(1).unwrap();

        assert!(joined_within(handle, Duration::from_secs(2)));
    }

    /// P1 regression: a device reset that lands between a queue kick and the
    /// event thread's wakeup must not panic the thread.
    ///
    /// The transport's reset path (`VsockMmioDevice::reset` → `Vsock::reset`,
    /// a driver status write of 0) clears the device's `activated`/`mem` state
    /// without notifying this thread, while the queue event fds keep their poll
    /// registrations. A guest that kicks RX/TX and then resets the transport
    /// therefore wakes the thread with a live event fd and a dead device;
    /// upstream's event handler re-checked the device's live `is_activated()`
    /// on every event, and this test pins that behaviour: the stale event is
    /// drained, no `process_rx`/`process_tx` runs, and the thread keeps
    /// running. Pre-fix, the thread panicked here (`process_tx`'s `mem` guard),
    /// which killed the VM's vsock for the rest of its life and turned even a
    /// clean ACPI S5 exit into `SpawnError::ThreadPanic`.
    #[test]
    fn a_stale_queue_kick_after_a_device_reset_does_not_panic_the_event_thread() {
        let mem = default_mem();
        let device = activated_device(&mem);
        let exit_evt = EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).unwrap();
        let exit_signalled = Arc::new(AtomicBool::new(false));
        let handle = start_event_thread(
            Arc::clone(&device),
            exit_evt.try_clone().unwrap(),
            Arc::clone(&exit_signalled),
            DeviceGate::default(),
            0,
        )
        .expect("the event thread starts");

        // Let the thread consume the activation event and settle into the
        // source arm (the queue event fds are registered by then).
        std::thread::sleep(Duration::from_millis(100));

        // The reset, off the vCPU thread: the device goes inactive, and the
        // thread is not told. Its queue event fds stay in the poll set.
        let kick = {
            let mut dev = lock(&device);
            dev.reset();
            assert!(!dev.is_activated(), "the reset deactivates the device");
            dev.queue_events()[TXQ_INDEX]
                .try_clone()
                .expect("the tx queue event fd clones")
        };

        // The guest's stale kick: a TX queue notification the driver wrote just
        // before resetting the transport.
        kick.write(1).unwrap();

        // The thread must survive it (pre-fix it panics in `process_tx`) and
        // must have drained the event fd so the count does not accumulate.
        std::thread::sleep(Duration::from_millis(250));
        assert!(
            !handle.is_finished(),
            "the event thread died on a queue kick that raced a device reset"
        );
        assert!(
            kick.read().is_err(),
            "the stale queue event fd was left pending instead of being drained"
        );

        // ... and it must still exit cleanly once the VM stops.
        exit_signalled.store(true, Ordering::SeqCst);
        assert!(
            joined_within(handle, Duration::from_secs(2)),
            "the event thread did not exit cleanly after the reset window"
        );
    }
}
