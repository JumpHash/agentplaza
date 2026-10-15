// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! The interrupt relay of the Hypervisor.framework backend: device interrupts arrive as `EventFd`
//! signals (what KVM would take as irqfds) and are raised as SPIs of the userspace GICv3. Pure: it
//! builds wherever `crate::pal` does, so its tests run on Linux; `hvf::boot` runs it on a thread
//! per VM.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::pal::AsRawFd;
use crate::pal::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use crate::pal::eventfd::EventFd;

/// Where the relay raises interrupts: the VM, held weakly by the caller.
pub trait SpiSink {
    /// Raises SPI `spi`. `false` when the VM is gone (the relay then ends).
    fn raise(&self, spi: u32) -> bool;
    /// Whether the VM still exists (checked when a wait times out).
    fn alive(&self) -> bool;
    /// Stops the VM: the relay cannot do its job.
    fn stop(&self);
}

/// One device interrupt: the event its transport signals, and its SPI.
pub type IrqSource = (EventFd, u32);

/// How long one wait lasts before the relay re-checks the exit flag and the VM.
const WAIT_MS: i32 = 200;

/// Polls every source and raises its SPI on `sink`, until `exit_signalled` is set (with `exit_evt`
/// written), the VM is gone, or polling fails (which stops the VM).
pub fn run(
    sink: &impl SpiSink,
    exit_evt: &EventFd,
    exit_signalled: &AtomicBool,
    sources: Vec<IrqSource>,
) {
    const EXIT: u64 = u64::MAX;
    let epoll = match Epoll::new() {
        Ok(epoll) => epoll,
        Err(err) => {
            eprintln!("hvf: cannot create the interrupt relay's poll set: {err}; stopping the VM");
            sink.stop();
            return;
        }
    };
    let mut registered = true;
    for (index, (evt, _)) in sources.iter().enumerate() {
        registered &= epoll
            .ctl(
                ControlOperation::Add,
                evt.as_raw_fd(),
                EpollEvent::new(EventSet::IN, index as u64),
            )
            .is_ok();
    }
    registered &= epoll
        .ctl(
            ControlOperation::Add,
            exit_evt.as_raw_fd(),
            EpollEvent::new(EventSet::IN, EXIT),
        )
        .is_ok();
    if !registered {
        eprintln!("hvf: cannot register the device interrupts; stopping the VM");
        sink.stop();
        return;
    }
    let mut events = vec![EpollEvent::default(); sources.len() + 1];
    while !exit_signalled.load(Ordering::SeqCst) {
        let ready = match epoll.wait(WAIT_MS, &mut events) {
            Ok(ready) => ready,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("hvf: the interrupt relay failed: {err}; stopping the VM");
                sink.stop();
                return;
            }
        };
        // A VM released without its exit signal (its last reference dropped):
        // nothing is left to interrupt.
        if ready == 0 && !sink.alive() {
            return;
        }
        for event in &events[..ready] {
            let data = event.data();
            if data == EXIT {
                continue;
            }
            if let Some((evt, spi)) = sources.get(data as usize)
                && evt.read().is_ok()
                && !sink.raise(*spi)
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use super::{IrqSource, SpiSink, run};
    use crate::pal::eventfd::{EFD_NONBLOCK, EventFd};

    /// Records what the relay raised, and what it did with the VM.
    struct Recorder {
        raised: Mutex<Vec<u32>>,
        alive: AtomicBool,
        stopped: AtomicBool,
        refuse: AtomicBool,
    }

    impl Recorder {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                raised: Mutex::new(Vec::new()),
                alive: AtomicBool::new(true),
                stopped: AtomicBool::new(false),
                refuse: AtomicBool::new(false),
            })
        }

        fn raised(&self) -> Vec<u32> {
            self.raised.lock().unwrap().clone()
        }
    }

    impl SpiSink for Recorder {
        fn raise(&self, spi: u32) -> bool {
            self.raised.lock().unwrap().push(spi);
            !self.refuse.load(Ordering::SeqCst)
        }

        fn alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        fn stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    /// Runs the relay on a thread, with its own clone of the exit event.
    fn spawn_relay(
        rec: &Arc<Recorder>,
        exit: &EventFd,
        flag: &Arc<AtomicBool>,
        sources: Vec<IrqSource>,
    ) -> JoinHandle<()> {
        let rec = Arc::clone(rec);
        let exit = exit.try_clone().unwrap();
        let flag = Arc::clone(flag);
        thread::spawn(move || run(&*rec, &exit, &flag, sources))
    }

    /// Joins the relay, failing the test if it has not ended within two seconds.
    fn join_within_two_seconds(handle: JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = handle.join();
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(2))
            .expect("the relay did not end");
    }

    /// Polls `cond` in 10 ms steps for up to two seconds.
    fn wait_until(cond: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !cond() {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn the_relay_raises_spis() {
        // a_signalled_source_raises_its_spi: a signalled source raises its SPI,
        // and the exit signal ends the relay without stopping the VM.
        {
            let rec = Recorder::new();
            let exit = EventFd::new(EFD_NONBLOCK).unwrap();
            let flag = Arc::new(AtomicBool::new(false));
            let quiet = EventFd::new(EFD_NONBLOCK).unwrap();
            let busy = EventFd::new(EFD_NONBLOCK).unwrap();
            let sources = vec![
                (quiet.try_clone().unwrap(), 1),
                (busy.try_clone().unwrap(), 8),
            ];
            let handle = spawn_relay(&rec, &exit, &flag, sources);

            busy.write(1).unwrap();
            assert!(
                wait_until(|| rec.raised() == [8]),
                "the relay did not raise SPI 8"
            );

            flag.store(true, Ordering::SeqCst);
            exit.write(1).unwrap();
            join_within_two_seconds(handle);
            assert!(!rec.stopped.load(Ordering::SeqCst));
        }
        // signals_before_a_read_coalesce_into_one_raise: signals written before
        // the relay reads coalesce into one raise.
        {
            let rec = Recorder::new();
            let exit = EventFd::new(EFD_NONBLOCK).unwrap();
            let flag = Arc::new(AtomicBool::new(false));
            let irq = EventFd::new(EFD_NONBLOCK).unwrap();
            for _ in 0..3 {
                irq.write(1).unwrap();
            }
            let handle = spawn_relay(&rec, &exit, &flag, vec![(irq.try_clone().unwrap(), 5)]);

            assert!(wait_until(|| !rec.raised().is_empty()));
            thread::sleep(Duration::from_millis(300));
            assert_eq!(rec.raised(), [5]);

            flag.store(true, Ordering::SeqCst);
            exit.write(1).unwrap();
            join_within_two_seconds(handle);
        }
        // a_refused_raise_ends_the_relay: a refused raise ends the relay, and the
        // raise was attempted.
        {
            let rec = Recorder::new();
            rec.refuse.store(true, Ordering::SeqCst);
            let exit = EventFd::new(EFD_NONBLOCK).unwrap();
            let flag = Arc::new(AtomicBool::new(false));
            let irq = EventFd::new(EFD_NONBLOCK).unwrap();
            let handle = spawn_relay(&rec, &exit, &flag, vec![(irq.try_clone().unwrap(), 3)]);

            irq.write(1).unwrap();
            join_within_two_seconds(handle);
            assert_eq!(rec.raised(), [3]);
        }
    }

    #[test]
    fn the_relay_ends_on_exit_or_release() {
        // the_exit_signal_ends_the_relay: the exit signal ends the relay with no raise and no stop.
        {
            let rec = Recorder::new();
            let exit = EventFd::new(EFD_NONBLOCK).unwrap();
            let flag = Arc::new(AtomicBool::new(false));
            let handle = spawn_relay(&rec, &exit, &flag, Vec::new());

            thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
            exit.write(1).unwrap();
            join_within_two_seconds(handle);
            assert!(rec.raised().is_empty());
            assert!(!rec.stopped.load(Ordering::SeqCst));
        }
        // a_released_vm_ends_the_relay: a released VM ends the relay with no raise and no stop.
        {
            let rec = Recorder::new();
            rec.alive.store(false, Ordering::SeqCst);
            let exit = EventFd::new(EFD_NONBLOCK).unwrap();
            let flag = Arc::new(AtomicBool::new(false));
            let handle = spawn_relay(&rec, &exit, &flag, Vec::new());

            join_within_two_seconds(handle);
            assert!(rec.raised().is_empty());
            assert!(!rec.stopped.load(Ordering::SeqCst));
        }
    }
}
