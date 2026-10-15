// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker's vCPUs synchronize through a `Barrier`
// and the KVM run loop's event channel; Hypervisor.framework has neither, so
// the protocol a halted or powered-off vCPU waits on, and the rule that keeps
// a wake from being lost, are written here. No upstream code is derived.

//! Waiting and waking a vCPU: the condition-variable protocol a vCPU uses
//! while it is powered off or idling, and the deadline that bounds an idle.
//!
//! # Why the notifier takes the lock
//!
//! A vCPU parks on its own [`WaitSlot`] in two places: while it is powered off
//! waiting for a PSCI `CPU_ON`, and while it idles after `WFI`/`WFE`/
//! `CPU_SUSPEND` waiting for an interrupt line to be asserted, its virtual
//! timer to come due, or the VM to be stopped. Both loops check the condition
//! and then wait; a notification that lands in the window between the two is
//! lost, and the thread then sleeps until some *other* event happens to wake
//! it. When the notification was a stop request, nothing else ever comes:
//! `VmHandle::kill` reports a timeout, `VmHandle::wait` blocks forever, and the
//! VM's guest memory and hypervisor resources are never released.
//!
//! [`WaitSlot::wake`] therefore takes the slot's lock before it notifies. A
//! notifier cannot run while a waiter holds the lock, so a notification is
//! either observed by the waiter's own check (the state it is about is set
//! before the notification) or delivered to a waiter that is already inside
//! `wait`, which has released the lock. There is no window in which it can be
//! dropped.
//!
//! # Lock order
//!
//! A waiter holds its slot's lock while it asks the interrupt controller
//! whether an interrupt is pending for it, so the controller's lock is taken
//! *inside* the slot's. A path that changes the controller must therefore
//! release it before waking anybody - see [`wake_asserted`].
//!
//! Neither [`WaitSlot::wake`] nor [`wake_asserted`] may be called while the
//! slot's own lock is held: they take it.
//!
//! This module has no FFI and no `crate::arch` dependency, so it compiles and
//! is unit-tested on any target.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The number of vCPUs [`wake_asserted`]'s bitmap can name: bit `i` wakes the
/// `i`-th slot. Each of the two backends runs at most this many vCPUs per VM.
pub const MAX_WAKE_BITS: usize = 64;

/// `CNTV_CTL_EL0.ENABLE`: the virtual timer is armed.
pub const CNTV_CTL_ENABLE: u64 = 1 << 0;
/// `CNTV_CTL_EL0.IMASK`: the timer's interrupt is masked.
pub const CNTV_CTL_IMASK: u64 = 1 << 1;
/// `CNTV_CTL_EL0.ISTATUS`: the timer's condition has fired.
pub const CNTV_CTL_ISTATUS: u64 = 1 << 2;

/// The state of one vCPU, guarded by its slot's mutex.
#[derive(Debug, Default)]
struct SlotState {
    /// Whether the vCPU is executing: false while it waits for a PSCI
    /// `CPU_ON`.
    running: bool,
    /// The entry point and `X0` a `CPU_ON` handed it, for the owning thread
    /// to apply.
    start: Option<(u64, u64)>,
}

/// What a powered-off vCPU was woken for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// The vCPU is powered on. (It was already on when its thread last
    /// checked, so there is no new entry point to apply.)
    Running,
    /// The vCPU was powered on at `entry`, with `context_id` in `X0`.
    At {
        /// The address the vCPU starts executing at.
        entry: u64,
        /// The value the vCPU finds in `X0`.
        context_id: u64,
    },
    /// The VM was asked to stop before the vCPU was powered on.
    Stopped,
}

/// One vCPU's wake slot: whether the vCPU is powered on, and how it is woken.
#[derive(Debug)]
pub struct WaitSlot {
    state: Mutex<SlotState>,
    condvar: Condvar,
}

impl WaitSlot {
    /// Creates a slot for a vCPU that is (`running`) or is not powered on.
    pub fn new(running: bool) -> Self {
        Self {
            state: Mutex::new(SlotState {
                running,
                start: None,
            }),
            condvar: Condvar::new(),
        }
    }

    /// Wakes the vCPU: every thread waiting on this slot, including the vCPU
    /// thread itself if it is in [`WaitSlot::halt`] or
    /// [`WaitSlot::wait_for_start`].
    ///
    /// Interrupting a vCPU that is inside its hypervisor's run call is the
    /// caller's business: only it knows the framework handle.
    pub fn wake(&self) {
        // The lock is what makes the notification reliable: a waiter between
        // its check and its `wait` still holds the lock, so the notification
        // cannot pass it by (see the module documentation).
        let _state = lock(&self.state);
        self.condvar.notify_all();
    }

    /// Whether the vCPU is powered on.
    pub fn is_running(&self) -> bool {
        lock(&self.state).running
    }

    /// Powers the vCPU on at `entry` with `context_id` in `X0`, and wakes it.
    ///
    /// Returns whether this call is what powered it on. A `CPU_ON` of a vCPU
    /// that is already on is the caller's business to report (PSCI answers
    /// `ALREADY_ON`), and the check and the state write have to happen
    /// together: two concurrent `CPU_ON`s of the same vCPU would otherwise
    /// both see it off and both start it, with different entry points.
    pub fn start(&self, entry: u64, context_id: u64) -> bool {
        let newly = {
            let mut state = lock(&self.state);
            let newly = !state.running;
            if newly {
                state.running = true;
                state.start = Some((entry, context_id));
            }
            newly
        };
        // `wake` takes the lock, so it cannot be called under it.
        if newly {
            self.wake();
        }
        newly
    }

    /// Powers the vCPU off (a PSCI `CPU_OFF`).
    pub fn park(&self) {
        lock(&self.state).running = false;
    }

    /// Blocks, for a vCPU that is not powered on, until it is started.
    ///
    /// `stop` reports whether the VM has been asked to stop. It is checked
    /// under the slot's lock, which is what makes the stop reliable: the state
    /// it reads is set before the wake that follows it, and the wake cannot
    /// run while this thread holds the lock.
    pub fn wait_for_start(&self, stop: impl Fn() -> bool) -> Start {
        let mut state = lock(&self.state);
        while !state.running {
            if stop() {
                return Start::Stopped;
            }
            state = self
                .condvar
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        match state.start.take() {
            Some((entry, context_id)) => Start::At { entry, context_id },
            None => Start::Running,
        }
    }

    /// Idles the vCPU until `ready` reports that it should run again, or until
    /// the deadline `sleep` elapses (`None` waits for a wake alone).
    ///
    /// `ready` runs under the slot's lock, so the reason for the wake cannot
    /// slip between the check and the sleep.
    pub fn halt(&self, sleep: Option<Duration>, ready: impl Fn() -> bool) {
        let mut state = lock(&self.state);
        loop {
            if ready() {
                break;
            }
            match sleep {
                None => {
                    state = self
                        .condvar
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(timeout) => {
                    let (guard, timed_out) = self
                        .condvar
                        .wait_timeout(state, timeout)
                        .unwrap_or_else(PoisonError::into_inner);
                    state = guard;
                    if timed_out.timed_out() {
                        // The deadline passed: the caller runs the vCPU so the
                        // hypervisor can report what has become due.
                        break;
                    }
                }
            }
        }
    }
}

/// Wakes the slots whose bit is set in `asserted`, calling `kick` for each so
/// that the caller can also interrupt a vCPU that is inside its hypervisor's
/// run call (which does not wait on the slot).
///
/// This is the second half of a change to the interrupt controller: the
/// controller's lock has to be released before it is called, because a waiting
/// vCPU holds its slot's lock and then takes the controller's.
///
/// At most [`MAX_WAKE_BITS`] slots can be named; a slot beyond that keeps its
/// bit unread and is not woken.
pub fn wake_asserted<'a>(
    slots: impl IntoIterator<Item = &'a WaitSlot>,
    asserted: u64,
    kick: impl Fn(usize),
) {
    for (index, slot) in slots.into_iter().enumerate() {
        if index >= MAX_WAKE_BITS {
            break;
        }
        if asserted >> index & 1 == 1 {
            slot.wake();
            kick(index);
        }
    }
}

/// How long a vCPU that halted itself may wait before it has to run again for
/// its virtual timer, or `None` to wait for a wake alone.
///
/// * `control` is `CNTV_CTL_EL0` and `compare` `CNTV_CVAL_EL0`: a timer that is
///   disabled or masked never wakes the vCPU, and one that is already due has
///   to be reported by the hypervisor (a run is what reports it), so the
///   deadline is zero.
/// * `deliverable` says whether the timer's interrupt could reach the guest
///   right now. A due timer whose interrupt cannot (the guest has masked it
///   through `ICC_PMR_EL1`, has not enabled Group 1 in `ICC_IGRPEN1_EL1`, or
///   has not enabled the PPI) must not produce a zero deadline: the caller
///   would run the vCPU, the guest's still-armed `WFI` would trap again, and
///   the loop would spin a host core without executing one guest instruction.
///   Waiting instead is correct, because the write that unmasks the interrupt
///   is a change to the controller, and such a change wakes the vCPU.
/// * `frequency` is the counter frequency; it is only asked for when a
///   deadline has to be converted from ticks to time.
pub fn idle_deadline(
    control: u64,
    compare: u64,
    now: u64,
    deliverable: bool,
    frequency: impl FnOnce() -> u64,
) -> Option<Duration> {
    if control & CNTV_CTL_ENABLE == 0 || control & CNTV_CTL_IMASK != 0 {
        return None;
    }
    if compare <= now {
        return deliverable.then_some(Duration::ZERO);
    }
    let ticks = u128::from(compare - now);
    let nanos = ticks * 1_000_000_000 / u128::from(frequency().max(1));
    // A guest can programme the timer arbitrarily far ahead; the deadline is
    // clamped rather than allowed to overflow.
    Some(Duration::from_nanos(
        u64::try_from(nanos).unwrap_or(u64::MAX),
    ))
}

/// Locks a mutex, recovering from a peer thread's panic the way the rest of
/// this tree does: one panicking thread must not take the whole VM down.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;

    use super::*;

    /// How long a test waits for a wake that has to arrive, and how long it
    /// insists that one has *not* arrived.
    const WAIT: Duration = Duration::from_secs(10);
    const NOT_YET: Duration = Duration::from_millis(250);

    /// Runs `body` on another thread, and reports when it has finished.
    fn background(body: impl FnOnce() + Send + 'static) -> (Receiver<()>, thread::JoinHandle<()>) {
        let (done, finished) = channel();
        let handle = thread::spawn(move || {
            body();
            let _ = done.send(());
        });
        (finished, handle)
    }

    /// Waits for a background thread to finish, failing the test instead of
    /// hanging forever when the wake it needs never arrives.
    fn expect_wake(finished: &Receiver<()>, what: &str) {
        if finished.recv_timeout(WAIT).is_err() {
            panic!("{what} did not wake the vCPU");
        }
    }

    #[test]
    fn a_wake_is_not_lost_while_the_waiter_decides_to_wait() {
        // The window this closes: the waiter is between its predicate check
        // and its `wait`, still holding the slot's lock. A notification that
        // is not serialized with that lock lands here and is gone, and the VM
        // then never stops: `kill` reports a timeout while `wait` blocks.
        let slot = Arc::new(WaitSlot::new(false));
        // Hold the lock for the whole window — taken before the waker starts, so the wake cannot
        // slip in first — then release it: the wake is deferred, not lost.
        let guard = lock(&slot.state);
        let (finished, waker) = {
            let slot = Arc::clone(&slot);
            background(move || slot.wake())
        };
        assert!(
            finished.recv_timeout(NOT_YET).is_err(),
            "a wake must not run while the slot's lock is held"
        );
        drop(guard);
        expect_wake(&finished, "releasing the slot's lock");
        waker.join().unwrap();
    }

    #[test]
    fn a_powered_off_vcpu_waits_for_its_start_or_a_stop() {
        // a_powered_off_vcpu_wakes_when_it_is_started: starting the vCPU wakes it at the
        // given entry, and a second start reports that it did not start the vCPU.
        {
            let slot = Arc::new(WaitSlot::new(false));
            let (finished, started) = {
                let slot = Arc::clone(&slot);
                background(move || {
                    assert_eq!(
                        slot.wait_for_start(|| false),
                        Start::At {
                            entry: 0x4000_0000,
                            context_id: 0x1234,
                        }
                    );
                })
            };

            assert!(slot.start(0x4000_0000, 0x1234), "the vCPU was off");
            expect_wake(&finished, "starting the vCPU");
            started.join().unwrap();

            // A second start reports that it did not start the vCPU, so PSCI can
            // answer `ALREADY_ON`; a parked vCPU waits again.
            assert!(!slot.start(0x8000_0000, 0), "the vCPU was already on");
            slot.park();
            assert!(!slot.is_running());
            assert!(slot.start(0x8000_0000, 7));
            assert_eq!(
                slot.wait_for_start(|| false),
                Start::At {
                    entry: 0x8000_0000,
                    context_id: 7,
                }
            );
        }
        // a_powered_off_vcpu_that_is_never_started_observes_the_stop: a stop reaches a
        // waiter that was never started.
        {
            let slot = Arc::new(WaitSlot::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let (finished, waiter) = {
                let slot = Arc::clone(&slot);
                let stop = Arc::clone(&stop);
                background(move || {
                    assert_eq!(
                        slot.wait_for_start(|| stop.load(Ordering::SeqCst)),
                        Start::Stopped
                    );
                })
            };

            // The stop arrives after the waiter has parked, and only a wake ends
            // the wait.
            assert!(finished.recv_timeout(NOT_YET).is_err());
            stop.store(true, Ordering::SeqCst);
            slot.wake();
            expect_wake(&finished, "stopping the VM");
            waiter.join().unwrap();
        }
    }

    #[test]
    fn a_halted_vcpu_wakes_on_a_pending_interrupt_or_a_deadline() {
        // a_halted_vcpu_wakes_when_the_reason_for_halting_is_gone: a halted vCPU wakes
        // once the interrupt it waits for is pending.
        {
            let slot = Arc::new(WaitSlot::new(true));
            let pending = Arc::new(AtomicBool::new(false));
            let (finished, halted) = {
                let slot = Arc::clone(&slot);
                let pending = Arc::clone(&pending);
                background(move || slot.halt(None, || pending.load(Ordering::SeqCst)))
            };

            assert!(
                finished.recv_timeout(NOT_YET).is_err(),
                "nothing is pending"
            );
            pending.store(true, Ordering::SeqCst);
            slot.wake();
            expect_wake(&finished, "an interrupt the vCPU waits for");
            halted.join().unwrap();
        }
        // a_halted_vcpu_wakes_when_its_deadline_elapses: a halted vCPU wakes when its
        // deadline elapses.
        {
            let slot = Arc::new(WaitSlot::new(true));
            let (finished, halted) = {
                let slot = Arc::clone(&slot);
                background(move || slot.halt(Some(Duration::from_millis(20)), || false))
            };
            expect_wake(&finished, "the deadline");
            halted.join().unwrap();
        }
    }

    #[test]
    fn a_wake_names_the_slots_the_bitmap_asserts() {
        // Each vCPU parks with its own release flag, and the wake's selection
        // is observed through the kick it performs - which is synchronous, so
        // this test does not depend on timing.
        let slots: Vec<Arc<WaitSlot>> = (0..3).map(|_| Arc::new(WaitSlot::new(true))).collect();
        let released: Vec<Arc<AtomicBool>> =
            (0..3).map(|_| Arc::new(AtomicBool::new(false))).collect();
        let checks: Vec<Arc<AtomicUsize>> = (0..3).map(|_| Arc::new(AtomicUsize::new(0))).collect();
        let (woken, woken_rx) = channel();
        let halters: Vec<_> = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let slot = Arc::clone(slot);
                let released = Arc::clone(&released[index]);
                let checks = Arc::clone(&checks[index]);
                let woken = woken.clone();
                thread::spawn(move || {
                    // No deadline: only a wake ends this wait.
                    slot.halt(None, || {
                        checks.fetch_add(1, Ordering::SeqCst);
                        released.load(Ordering::SeqCst)
                    });
                    let _ = woken.send(index);
                })
            })
            .collect();

        // Wait until every vCPU has checked its predicate at least once, i.e.
        // is parked (or is on its way into the wait).
        let deadline = Instant::now() + WAIT;
        while checks
            .iter()
            .any(|checks| checks.load(Ordering::SeqCst) == 0)
        {
            assert!(Instant::now() < deadline, "the vCPUs did not park");
            thread::yield_now();
        }

        let kicks = Arc::new(Mutex::new(Vec::new()));
        let kicked = Arc::clone(&kicks);
        for index in [0, 2] {
            released[index].store(true, Ordering::SeqCst);
        }
        wake_asserted(slots.iter().map(Arc::as_ref), 0b101, move |index| {
            kicked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(index);
        });

        // The slots the bitmap names wake (in whichever order their threads
        // get scheduled); the one it does not is released explicitly below,
        // so nothing it reports can be the wake's doing.
        let mut reported = vec![
            woken_rx.recv_timeout(WAIT).unwrap(),
            woken_rx.recv_timeout(WAIT).unwrap(),
        ];
        reported.sort_unstable();
        assert_eq!(reported, vec![0, 2]);
        assert_eq!(*kicks.lock().unwrap(), vec![0, 2]);

        released[1].store(true, Ordering::SeqCst);
        slots[1].wake();
        for halter in halters {
            halter.join().unwrap();
        }
    }

    #[test]
    fn a_due_timer_that_cannot_reach_the_guest_is_not_a_spin() {
        const FREQ: u64 = 24_000_000;
        // Armed and unmasked, as a guest programmes it for its next tick.
        let armed = CNTV_CTL_ENABLE;
        let now = 1_000;

        // Already due, with its interrupt deliverable: the vCPU runs so the
        // hypervisor reports the timer.
        assert_eq!(
            idle_deadline(armed, now, now, true, || FREQ),
            Some(Duration::ZERO)
        );
        // The same timer with its interrupt unable to reach the guest (the
        // reset `ICC_PMR_EL1` is 0, `ICC_IGRPEN1_EL1` is disabled, or the PPI
        // is not enabled): running the vCPU would execute no guest
        // instruction, so it waits for the controller change that unmasks it.
        assert_eq!(idle_deadline(armed, now, now, false, || FREQ), None);

        // Not due yet: a deadline of the difference in ticks (one second).
        assert_eq!(
            idle_deadline(armed, now + FREQ, now, true, || FREQ),
            Some(Duration::from_secs(1))
        );
        // A disabled or masked timer never wakes the vCPU.
        assert_eq!(idle_deadline(0, now, now, true, || FREQ), None);
        assert_eq!(
            idle_deadline(CNTV_CTL_ENABLE | CNTV_CTL_IMASK, now, now, true, || FREQ),
            None
        );
        // A timer programmed arbitrarily far ahead is clamped, not overflowed.
        assert_eq!(
            idle_deadline(armed, u64::MAX, now, true, || FREQ),
            Some(Duration::from_nanos(u64::MAX))
        );
    }
}
