// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/vstate/vcpu.rs (trimmed: pause/resume,
// save-state and cpu-config-dump events, gdb support, seccomp filters and
// the response channel are dropped — the run loop only handles `Finish`;
// the per-VM exit watcher that upstream runs on the main thread's event
// loop lives in `builder`); the arch re-export is cfg-selected because this
// tree's `arch/mod.rs` declares the per-arch modules rather than re-exporting
// their items.

use std::os::fd::AsRawFd;
use std::sync::atomic::{Ordering, fence};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Barrier};
use std::{io, thread};

use kvm_bindings::{KVM_SYSTEM_EVENT_RESET, KVM_SYSTEM_EVENT_SHUTDOWN};
use kvm_ioctls::VcpuExit;
use libc::{c_int, c_void, siginfo_t};
use vmm_sys_util::errno;
use vmm_sys_util::eventfd::EventFd;
use vmm_sys_util::signal::{Killable, SIGRTMIN};

// Upstream re-exports the arch-specific `KvmVcpu` through `crate::arch`;
// select the current arch's module here.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use crate::arch::x86_64::vcpu::{KvmVcpu, KvmVcpuConfigureError, KvmVcpuError, Peripherals};
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub use crate::arch::aarch64::vcpu::{KvmVcpu, KvmVcpuConfigureError, KvmVcpuError, Peripherals};
use crate::vstate::vm::KvmVm;

/// Signal number (SIGRTMIN) used to kick Vcpus.
pub const VCPU_RTSIG_OFFSET: i32 = 0;

/// Errors associated with the wrappers over KVM ioctls.
#[derive(Debug, thiserror::Error)]
pub enum VcpuError {
    /// Received error signaling kvm exit: {0}
    #[error("Received error signaling kvm exit: {0}")]
    FaultyKvmExit(String),
    /// Failed to signal vcpu: {0}
    #[error("Failed to signal vcpu: {0}")]
    SignalVcpu(vmm_sys_util::errno::Error),
    /// Unexpected kvm exit received: {0}
    #[error("Unexpected kvm exit received: {0}")]
    UnhandledKvmExit(String),
    /// Failed to run action on vcpu: {0}
    #[error("Failed to run action on vcpu: {0}")]
    VcpuResponse(KvmVcpuError),
    /// Failed to create the KVM vCPU: {0}
    #[error("Failed to create the KVM vCPU: {0}")]
    KvmVcpu(#[from] KvmVcpuError),
    /// Cannot spawn a new vCPU thread: {0}
    #[error("Cannot spawn a new vCPU thread: {0}")]
    VcpuSpawn(io::Error),
}

/// Error type for [`Vcpu::start_threaded`].
#[derive(Debug, thiserror::Error)]
pub enum StartThreadedError {
    /// Failed to spawn vCPU thread: {0}
    #[error("Failed to spawn vCPU thread: {0}")]
    Spawn(std::io::Error),
    /// Failed to clone kvm Vcpu fd: {0}
    #[error("Failed to clone kvm Vcpu fd: {0}")]
    CopyFd(CopyKvmFdError),
}

/// Error type for [`Vcpu::copy_kvm_vcpu_fd`].
#[derive(Debug, thiserror::Error)]
pub enum CopyKvmFdError {
    /// Error with libc dup of kvm Vcpu fd
    #[error("Error with libc dup of kvm Vcpu fd")]
    DupError(#[from] std::io::Error),
    /// Error creating the Vcpu from the duplicated Vcpu fd
    #[error("Error creating the Vcpu from the duplicated Vcpu fd")]
    CreateVcpuError(#[from] kvm_ioctls::Error),
}

/// A wrapper around creating and using a vcpu.
#[derive(Debug)]
pub struct Vcpu {
    /// Access to kvm-arch specific functionality.
    pub kvm_vcpu: KvmVcpu,

    /// File descriptor for vcpu to trigger exit event on vmm.
    exit_evt: EventFd,
    /// The receiving end of events channel owned by the vcpu side.
    event_receiver: Receiver<VcpuEvent>,
    /// The transmitting end of the events channel which will be given to the handler.
    event_sender: Option<Sender<VcpuEvent>>,
}

/// States of the vCPU thread's run loop.
///
/// Upstream also has a `Paused` state entered on `VcpuEvent::Pause` from the
/// main thread's control loop; this port has no pause/resume events, so the
/// loop runs until the vCPU exits or a `Finish` event arrives.
#[derive(Debug)]
enum VcpuRunState {
    /// The vCPU is executing guest code via `KVM_RUN`.
    Running,
    /// The vCPU thread's run loop has finished; the thread will exit.
    Finished,
}

impl Vcpu {
    /// Registers a signal handler which kicks the vcpu running on the current thread, if there is
    /// one.
    fn register_kick_signal_handler(&mut self) {
        extern "C" fn handle_signal(_: c_int, _: *mut siginfo_t, _: *mut c_void) {
            // We write to the immediate_exit from other thread, so make sure the read in the
            // KVM_RUN sees the up to date value
            fence(Ordering::Acquire);
        }
        // The upstream equivalent lives in the `utils` crate; the no-op
        // handler registration is inlined here (trimmed: no error logging
        // on failure — a failed registration only costs the kick latency).
        // SAFETY: `register_signal_handler` only installs the given handler
        // for the given signal number.
        let _ = vmm_sys_util::signal::register_signal_handler(
            SIGRTMIN() + VCPU_RTSIG_OFFSET,
            handle_signal,
        );
    }

    /// Constructs a new VCPU for `vm`.
    ///
    /// # Arguments
    ///
    /// * `index` - Represents the 0-based CPU index between [0, max vcpus).
    /// * `vm` - The vm to which this vcpu will get attached.
    /// * `exit_evt` - An `EventFd` that will be written into when this vcpu exits.
    pub fn new(index: u8, vm: &KvmVm, exit_evt: EventFd) -> Result<Self, VcpuError> {
        let (event_sender, event_receiver) = channel();
        let kvm_vcpu = KvmVcpu::new(index, vm)?;

        Ok(Vcpu {
            exit_evt,
            event_receiver,
            event_sender: Some(event_sender),
            kvm_vcpu,
        })
    }

    /// Sets a MMIO bus for this vcpu.
    pub fn set_mmio_bus(&mut self, mmio_bus: Arc<crate::vstate::bus::Bus>) {
        self.kvm_vcpu.peripherals.mmio_bus = Some(mmio_bus);
    }

    /// Obtains a copy of the VcpuFd
    pub fn copy_kvm_vcpu_fd(&self, vm: &KvmVm) -> Result<kvm_ioctls::VcpuFd, CopyKvmFdError> {
        // SAFETY: We own this fd so it is considered safe to clone
        let r = unsafe { libc::dup(self.kvm_vcpu.fd.as_raw_fd()) };
        if r < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: We assert this is a valid fd by checking the result from the dup
        unsafe { Ok(vm.fd().create_vcpu_from_rawfd(r)?) }
    }

    /// Moves the vcpu to its own thread and constructs a VcpuHandle.
    /// The handle can be used to control the remote vcpu.
    pub fn start_threaded(
        mut self,
        vm: &KvmVm,
        barrier: Arc<Barrier>,
    ) -> Result<VcpuHandle, StartThreadedError> {
        let event_sender = self.event_sender.take().expect("vCPU already started");
        let vcpu_fd = self
            .copy_kvm_vcpu_fd(vm)
            .map_err(StartThreadedError::CopyFd)?;
        let vcpu_thread = thread::Builder::new()
            .name(format!("apz_vcpu_{}", self.kvm_vcpu.index))
            .spawn(move || {
                self.register_kick_signal_handler();
                // Synchronization to make sure thread local data is initialized.
                barrier.wait();
                self.run();
            })
            .map_err(StartThreadedError::Spawn)?;

        Ok(VcpuHandle::new(event_sender, vcpu_fd, vcpu_thread))
    }

    /// Main loop of the vCPU thread.
    ///
    /// Runs the vCPU in KVM context in a loop. Handles KVM_EXITs then goes back in.
    /// Note that the state of the VCPU and associated VM must be setup first for this to do
    /// anything useful.
    pub fn run(&mut self) {
        // Start running the machine state in the `Running` state. (Upstream
        // starts `Paused` and waits for a resume event from the main thread.)
        let mut state = VcpuRunState::Running;
        loop {
            state = match state {
                VcpuRunState::Running => self.running(),
                VcpuRunState::Finished => break,
            };
        }
    }

    // This is the main loop of the `Running` state.
    fn running(&mut self) -> VcpuRunState {
        // This loop is here just for optimizing the emulation path.
        // No point in ticking the state machine if there are no external events.
        loop {
            match self.run_emulation() {
                // Emulation ran successfully, continue.
                Ok(VcpuEmulation::Handled) => (),
                // Emulation was interrupted, check external events.
                Ok(VcpuEmulation::Interrupted) => break,
                // The guest requested a SHUTDOWN or RESET.
                Ok(VcpuEmulation::Stopped) => return self.exit(),
                // Emulation errors lead to vCPU exit.
                Err(_) => return self.exit(),
            }
        }

        // Break this emulation loop on any transition request/external event.
        match self.event_receiver.try_recv() {
            Ok(VcpuEvent::Finish) => VcpuRunState::Finished,
            // Unhandled exit of the other end: the handle was dropped
            // without a Finish event; the thread cannot be controlled
            // anymore, so stop running the vCPU.
            Err(TryRecvError::Disconnected) => self.exit(),
            // No external event: continue running.
            Err(TryRecvError::Empty) => VcpuRunState::Running,
        }
    }

    // Transition to the exited state.
    //
    // Signals the VM's exit event (watched by the per-VM exit watcher, which
    // finishes the sibling vCPU threads) and finishes this thread's loop.
    fn exit(&mut self) -> VcpuRunState {
        if let Err(err) = self.exit_evt.write(1) {
            eprintln!("warning: failed signaling vcpu exit event: {err}");
        }
        VcpuRunState::Finished
    }

    /// Runs the vCPU in KVM context and handles the kvm exit reason.
    ///
    /// Returns error or enum specifying whether emulation was handled or interrupted.
    pub fn run_emulation(&mut self) -> Result<VcpuEmulation, VcpuError> {
        let emulation_result = self.kvm_vcpu.fd.run();
        match emulation_result {
            Err(err) if err.errno() == libc::EINTR => {
                self.kvm_vcpu.fd.set_kvm_immediate_exit(0);
                // Notify that this KVM_RUN was interrupted.
                Ok(VcpuEmulation::Interrupted)
            }
            _ => handle_kvm_exit(&mut self.kvm_vcpu.peripherals, emulation_result),
        }
    }
}

/// Handle the return value of a call to [`VcpuFd::run`] and update our emulation accordingly
fn handle_kvm_exit(
    peripherals: &mut Peripherals,
    emulation_result: Result<VcpuExit, errno::Error>,
) -> Result<VcpuEmulation, VcpuError> {
    match emulation_result {
        Ok(run) => match run {
            VcpuExit::MmioRead(addr, data) => {
                data.fill(0);
                if let Some(mmio_bus) = &peripherals.mmio_bus {
                    if let Err(err) = mmio_bus.read(addr, data) {
                        eprintln!(
                            "warning: invalid MMIO read @ {addr:#x}:{:#x}: {err}",
                            data.len()
                        );
                    }
                }
                Ok(VcpuEmulation::Handled)
            }
            VcpuExit::MmioWrite(addr, data) => {
                if let Some(mmio_bus) = &peripherals.mmio_bus {
                    if let Err(err) = mmio_bus.write(addr, data) {
                        eprintln!(
                            "warning: invalid MMIO write @ {addr:#x}:{:#x}: {err}",
                            data.len()
                        );
                    }
                }
                Ok(VcpuEmulation::Handled)
            }
            // Documentation specifies that below kvm exits are considered
            // errors.
            VcpuExit::FailEntry(hardware_entry_failure_reason, cpu) => {
                // Hardware entry failure.
                eprintln!(
                    "warning: received KVM_EXIT_FAIL_ENTRY signal: {hardware_entry_failure_reason} on cpu {cpu}"
                );
                Err(VcpuError::FaultyKvmExit(format!(
                    "{:?}",
                    VcpuExit::FailEntry(hardware_entry_failure_reason, cpu)
                )))
            }
            VcpuExit::InternalError => {
                // Failure from the Linux KVM subsystem rather than from the hardware.
                eprintln!("warning: received KVM_EXIT_INTERNAL_ERROR signal");
                Err(VcpuError::FaultyKvmExit(format!(
                    "{:?}",
                    VcpuExit::InternalError
                )))
            }
            VcpuExit::SystemEvent(event_type, event_flags) => match event_type {
                KVM_SYSTEM_EVENT_RESET | KVM_SYSTEM_EVENT_SHUTDOWN => {
                    eprintln!(
                        "info: received KVM_SYSTEM_EVENT: type: {event_type}, event: {event_flags:?}"
                    );
                    Ok(VcpuEmulation::Stopped)
                }
                _ => {
                    eprintln!(
                        "warning: received KVM_SYSTEM_EVENT signal type: {event_type}, flag: {event_flags:?}"
                    );
                    Err(VcpuError::FaultyKvmExit(format!(
                        "{:?}",
                        VcpuExit::SystemEvent(event_type, event_flags)
                    )))
                }
            },
            arch_specific_reason => {
                // run specific architecture emulation.
                peripherals.run_arch_emulation(arch_specific_reason)
            }
        },
        Err(err) => match err.errno() {
            libc::EAGAIN => Ok(VcpuEmulation::Handled),
            libc::ENOSYS => Err(VcpuError::FaultyKvmExit(
                "Received ENOSYS error because KVM failed to emulate an instruction."
                    .to_string(),
            )),
            _ => Err(VcpuError::FaultyKvmExit(format!("{err}"))),
        },
    }
}

/// List of events that the Vcpu can receive.
///
/// Upstream also has `Pause`, `Resume`, `SaveState` and `DumpCpuConfig`
/// events for the main thread's control loop; this port only needs `Finish`.
#[derive(Debug, Clone)]
pub enum VcpuEvent {
    /// The vCPU thread will end when receiving this message.
    Finish,
}

/// Wrapper over Vcpu that hides the underlying interactions with the Vcpu thread.
#[derive(Debug)]
pub struct VcpuHandle {
    event_sender: Sender<VcpuEvent>,
    /// VcpuFd
    pub vcpu_fd: kvm_ioctls::VcpuFd,
    // Rust JoinHandles have to be wrapped in Option if you ever plan on 'join()'ing them.
    // We want to be able to join these threads in tests.
    vcpu_thread: Option<thread::JoinHandle<()>>,
}

/// Error type for [`VcpuHandle::send_event`].
#[derive(Debug, thiserror::Error)]
#[error("Failed to signal vCPU: {0}")]
pub struct VcpuSendEventError(pub vmm_sys_util::errno::Error);

impl VcpuHandle {
    /// Creates a new [`VcpuHandle`].
    ///
    /// # Arguments
    /// + `event_sender`: [`Sender`] to communicate [`VcpuEvent`] to control the vcpu.
    /// + `vcpu_fd`: duplicated vCPU fd, used to set `immediate_exit` from this side.
    /// + `vcpu_thread`: A [`JoinHandle`] for the vcpu thread.
    pub fn new(
        event_sender: Sender<VcpuEvent>,
        vcpu_fd: kvm_ioctls::VcpuFd,
        vcpu_thread: thread::JoinHandle<()>,
    ) -> Self {
        Self {
            event_sender,
            vcpu_fd,
            vcpu_thread: Some(vcpu_thread),
        }
    }

    /// Sends event to vCPU.
    ///
    /// Upstream expects the channel to be alive; a vCPU thread that already
    /// exited is not an error for this port's watchers (they finish every
    /// vCPU, including ones that already stopped on their own).
    pub fn send_event(&mut self, event: VcpuEvent) -> Result<(), VcpuSendEventError> {
        // The vCPU thread may already have exited; then the channel is closed
        // and there is nothing left to kick.
        if self.event_sender.send(event).is_err() {
            return Ok(());
        }
        // Kick the vcpu so it picks up the message.
        // Add a fence to ensure the write is visible to the vpu thread
        self.vcpu_fd.set_kvm_immediate_exit(1);
        fence(Ordering::Release);
        // Safe to unwrap since constructor make this 'Some'.
        match self
            .vcpu_thread
            .as_ref()
            .expect("vcpu handle keeps its thread")
            .kill(SIGRTMIN() + VCPU_RTSIG_OFFSET)
        {
            // The vCPU thread exited between the send and the signal: it is
            // already where we want it.
            Err(err) if err.errno() == libc::ESRCH => Ok(()),
            result => result.map_err(VcpuSendEventError),
        }
    }
}

// Wait for the Vcpu thread to finish execution
impl Drop for VcpuHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.vcpu_thread.take() {
            if thread.join().is_err() {
                eprintln!("warning: vCPU thread panicked during teardown");
            }
        }
    }
}

/// Vcpu emulation state.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VcpuEmulation {
    /// Handled.
    Handled,
    /// Interrupted.
    Interrupted,
    /// Stopped.
    Stopped,
}
