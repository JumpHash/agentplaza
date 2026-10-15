// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/vstate/vm.rs (trimmed: snapshots,
// dirty bitmaps, userfaultfd, memory hotplug, MSI/MSI-X routing tables and
// the `Vm` newtype wrapper are dropped; `register_irq` only programs the
// irqfd because no userspace routing is installed without virtio; the
// arch re-exports are cfg-selected because this tree's `arch/mod.rs`
// declares the per-arch modules rather than re-exporting their items).

// Syscall boundary (see the allowlist in `lib.rs`): `KVM_SET_USER_MEMORY_REGION`
// passes a raw userspace pointer to the kernel, so the trait method that wraps
// it is `unsafe` and the caller must have validated the region.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use kvm_bindings::kvm_userspace_memory_region;
use kvm_ioctls::VmFd;
use vmm_sys_util::errno;
use vmm_sys_util::eventfd::EventFd;

// Upstream re-exports the arch-specific `KvmVm` through `crate::arch`; select
// the current arch's module here.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub use crate::arch::aarch64::vm::{KvmVm, KvmVmError};
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use crate::arch::x86_64::vm::{KvmVm, KvmVmError};
use crate::vstate::bus::{Bus, lock};
use crate::vstate::kvm::Kvm;
use crate::vstate::memory::{GuestMemoryMmap, GuestRegionMmap, GuestRegionMmapExt, MemoryError};
use crate::vstate::resources::ResourceAllocator;
use crate::vstate::vcpu::{
    StartGate, StartThreadedError, Vcpu, VcpuError, VcpuEvent, VcpuHandle, VcpuResponse,
};

/// Error type for [`KvmVm::start_vcpus`].
#[derive(Debug, thiserror::Error)]
pub enum StartVcpusError {
    /// Vcpu handle error: {0}
    #[error("Vcpu handle error: {0}")]
    VcpuHandle(#[from] StartThreadedError),
}

/// Architecture independent parts of a VM.
#[derive(Debug)]
pub struct VmCommon {
    /// The KVM file descriptor used to access this KvmVm.
    pub fd: VmFd,
    max_memslots: u32,
    /// The guest memory of this KvmVm.
    pub guest_memory: GuestMemoryMmap,
    next_kvm_slot: AtomicU32,
    /// Allocator for VM resources
    pub resource_allocator: Mutex<ResourceAllocator>,
    /// MMIO bus
    pub mmio_bus: Arc<Bus>,
    /// The global KVM state (fd + capabilities).
    pub kvm: Kvm,
    /// Handles to vCPU threads.
    pub vcpus_handles: Mutex<Vec<VcpuHandle>>,
    /// Event fd written to by vCPUs on exit.
    pub vcpus_exit_evt: EventFd,
    /// Whether the VM's exit has been signalled, published by every path that
    /// writes [`vcpus_exit_evt`](Self::vcpus_exit_evt) **before** it notifies
    /// the event fd.
    ///
    /// The event fd is a counter with a single reader — the VM's exit watcher
    /// — and that reader is deliberately the only one: a second `read` would
    /// steal its wakeup. But a second *observer*, the vsock event thread, may
    /// share the fd for wakeup and must still see the exit: a level-triggered
    /// `epoll_wait` revalidates readiness through the fd's poll callback, so a
    /// count the watcher consumed first is simply lost to the observer. This
    /// flag is that observer's source of truth. It is the same rule the vCPU
    /// stop path follows: set the state you are notifying about before you
    /// notify.
    pub exit_signalled: Arc<AtomicBool>,
    /// Set by a vCPU whose exit was the guest asking for a *reset* rather than
    /// a shutdown; read by the VM's exit watcher, which reports it to the
    /// spawner. (The exit event fd cannot carry the difference: it is a
    /// counter several writers share.)
    pub guest_reset: Arc<AtomicBool>,
    /// Whether the VM's vCPUs have been asked to finish.
    ///
    /// The `Finish` event alone is not enough to stop a vCPU: a vCPU inside
    /// `KVM_RUN` only notices the message once it is kicked back to user
    /// space, and one kick can be lost - it lands while the vCPU is already in
    /// user space, or before the thread has entered its run loop at all - after
    /// which the vCPU re-enters the guest and the run loop, which only consults
    /// the event channel when `KVM_RUN` is interrupted, never looks again. The
    /// request is therefore published as a flag *before* the kick, and every
    /// vCPU checks it on each way out of `KVM_RUN`: whatever the exit reason
    /// was, the next one observes the request. It is the same rule the
    /// Hypervisor.framework backend's wake slots follow (`hvf/wait.rs`): set
    /// the state you are notifying about before you notify.
    pub vcpu_finish_requested: Arc<AtomicBool>,
}

/// Errors associated with the wrappers over KVM ioctls.
#[derive(Debug, thiserror::Error)]
pub enum VmError {
    /// Cannot set the memory regions: {0}
    #[error("Cannot set the memory regions: {0}")]
    SetUserMemoryRegion(kvm_ioctls::Error),
    /// Failed to create VM: {0}
    #[error("Failed to create VM: {0}")]
    CreateVm(kvm_ioctls::Error),
    /// {0}
    #[error("{0}")]
    Arch(#[from] KvmVmError),
    /// Error during eventfd operations: {0}
    #[error("Error during eventfd operations: {0}")]
    EventFd(std::io::Error),
    /// Failed to create vcpu: {0}
    #[error("Failed to create vcpu: {0}")]
    CreateVcpu(VcpuError),
    /// The number of configured slots is bigger than the maximum reported by KVM: {0}
    #[error("The number of configured slots is bigger than the maximum reported by KVM: {0}")]
    NotEnoughMemorySlots(u32),
    /// Failed to add a memory region: {0}
    #[error("Failed to add a memory region: {0}")]
    InsertRegion(#[from] vm_memory::GuestRegionCollectionError),
    /// MemoryError error: {0}
    #[error("MemoryError error: {0}")]
    MemoryError(#[from] MemoryError),
    /// vCPU {vcpu} failed to {action}: {reason}
    #[error("vCPU {vcpu} failed to {action}: {reason}")]
    VcpuControl {
        /// Index of the vCPU, in the order the VM started them.
        vcpu: usize,
        /// The request that failed, for example "pause".
        action: &'static str,
        /// Why: a timeout, an exited thread, a refusal or a failure reported by the vCPU.
        reason: String,
    },
}

/// How long a vCPU may take to answer a pause, resume or save request. A
/// vCPU answers at its next `KVM_RUN` exit (the kick), or at once when it is
/// already paused, so this only elapses when a vCPU is stuck.
const VCPU_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Contains KvmVm functions that are usable across CPU architectures
impl KvmVm {
    /// Create a KVM VM
    pub fn create_common(kvm: Kvm) -> Result<VmCommon, VmError> {
        // It is known that KVM_CREATE_VM occasionally fails with EINTR on heavily loaded machines
        // with many VMs.
        //
        // The behavior itself that KVM_CREATE_VM can return EINTR is intentional. This is because
        // the KVM_CREATE_VM path includes mm_take_all_locks() that is CPU intensive and all CPU
        // intensive syscalls should check for pending signals and return EINTR immediately to allow
        // userland to remain interactive.
        // https://lists.nongnu.org/archive/html/qemu-devel/2014-01/msg01740.html
        //
        // However, it is empirically confirmed that, even though there is no pending signal,
        // KVM_CREATE_VM returns EINTR.
        // https://lore.kernel.org/qemu-devel/8735e0s1zw.wl-maz@kernel.org/
        //
        // To mitigate it, QEMU does an infinite retry on EINTR that greatly improves reliabiliy:
        // - https://github.com/qemu/qemu/commit/94ccff133820552a85c0fb95e33a539e0b90a75
        // - https://github.com/qemu/qemu/commit/bbde13cd14ad4eec18529ce0bf5876058464e124
        //
        // Similarly, we do retries up to 5 times. Although Firecracker clients are also able to
        // retry, they have to start Firecracker from scratch. Doing retries in Firecracker makes
        // recovery faster and improves reliability.
        const MAX_ATTEMPTS: u32 = 5;
        let mut attempt = 1;
        let fd = loop {
            match kvm.fd.create_vm() {
                Ok(fd) => break fd,
                Err(e) if e.errno() == libc::EINTR && attempt < MAX_ATTEMPTS => {
                    // Exponential backoff (1us, 2us, 4us, and 8us => 15us in total)
                    std::thread::sleep(std::time::Duration::from_micros(2u64.pow(attempt - 1)));
                }
                Err(e) => return Err(VmError::CreateVm(e)),
            }

            attempt += 1;
        };

        let vcpus_exit_evt = EventFd::new(libc::EFD_NONBLOCK).map_err(VmError::EventFd)?;

        Ok(VmCommon {
            fd,
            max_memslots: kvm.max_nr_memslots(),
            guest_memory: GuestMemoryMmap::default(),
            next_kvm_slot: AtomicU32::new(0),
            resource_allocator: Mutex::new(ResourceAllocator::new()),
            mmio_bus: Arc::new(Bus::new()),
            kvm,
            vcpus_handles: Mutex::new(Vec::new()),
            vcpus_exit_evt,
            exit_signalled: Arc::new(AtomicBool::new(false)),
            guest_reset: Arc::new(AtomicBool::new(false)),
            vcpu_finish_requested: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Creates the specified number of [`Vcpu`]s.
    ///
    /// Each vCPU gets a clone of the `vcpus_exit_evt` EventFd stored on this KvmVm.
    pub fn create_vcpus(&mut self, vcpu_count: u8) -> Result<Vec<Vcpu>, VmError> {
        self.arch_pre_create_vcpus(vcpu_count)?;

        let mut vcpus = Vec::with_capacity(vcpu_count as usize);
        for cpu_idx in 0..vcpu_count {
            let exit_evt = self
                .vcpus_exit_evt()
                .try_clone()
                .map_err(VmError::EventFd)?;
            let vcpu = Vcpu::new(cpu_idx, self, exit_evt).map_err(VmError::CreateVcpu)?;
            vcpus.push(vcpu);
        }

        self.arch_post_create_vcpus(vcpu_count)?;

        Ok(vcpus)
    }

    /// Returns a reference to the [`Kvm`] instance.
    pub fn kvm(&self) -> &Kvm {
        &self.common.kvm
    }

    /// Returns a reference to the vCPU exit [`EventFd`].
    pub fn vcpus_exit_evt(&self) -> &EventFd {
        &self.common.vcpus_exit_evt
    }

    /// The VM's "exit signalled" flag, set before every write to
    /// [`vcpus_exit_evt`](Self::vcpus_exit_evt) (see
    /// [`VmCommon::exit_signalled`]).
    pub fn exit_signalled(&self) -> &Arc<AtomicBool> {
        &self.common.exit_signalled
    }

    /// Returns a locked reference to the vCPU handles.
    pub fn vcpus_handles(&self) -> MutexGuard<'_, Vec<VcpuHandle>> {
        lock(&self.common.vcpus_handles)
    }

    /// Whether the guest asked for a *reset* rather than a shutdown.
    ///
    /// The exit watcher reads this once the VM's exit event fires, and reports
    /// it to the spawner; it is false for a VM that was stopped from outside.
    pub fn guest_reset(&self) -> bool {
        self.common.guest_reset.load(Ordering::SeqCst)
    }

    /// Starts the microVM vCPUs.
    ///
    /// Spawns a thread per vCPU and releases them together once every thread
    /// is up. (Upstream also sets the controlling terminal to raw mode here
    /// for the serial console; this port has no serial input, so that is
    /// trimmed, along with the seccomp filter parameter.)
    ///
    /// A vCPU thread that cannot be spawned does not leave the earlier ones
    /// running: the gate is released in abort mode, so every thread that did
    /// start returns without ever entering `KVM_RUN`, and dropping their
    /// handles joins them. The caller is about to free the VM and its guest
    /// memory, and a vCPU left running would keep translating through memory
    /// that is no longer its own.
    pub fn start_vcpus(self: &Arc<Self>, mut vcpus: Vec<Vcpu>) -> Result<(), StartVcpusError> {
        let gate = Arc::new(StartGate::new());
        let mut started = Vec::with_capacity(vcpus.len());

        for mut vcpu in vcpus.drain(..) {
            vcpu.set_mmio_bus(self.common.mmio_bus.clone());
            #[cfg(target_arch = "x86_64")]
            vcpu.kvm_vcpu.set_pio_bus(self.pio_bus.clone());

            match vcpu.start_threaded(self, Arc::clone(&gate)) {
                Ok(handle) => started.push(handle),
                Err(err) => {
                    gate.abort();
                    drop(started);
                    return Err(StartVcpusError::VcpuHandle(err));
                }
            }
        }

        self.vcpus_handles().extend(started);
        gate.open();
        Ok(())
    }

    /// Sends finish events to all vCPU threads and joins them.
    ///
    /// The request is published in `vcpu_finish_requested` **before** the
    /// events are sent (see that field's documentation): the vCPU threads
    /// check it on every exit from `KVM_RUN`, so a vCPU that is not (or not
    /// yet) reachable through its handle's channel - one still parked on the
    /// start gate, or one whose `Finish` message was lost to a kick that
    /// arrived while it was in user space - stops all the same instead of
    /// running on after this function returns.
    pub fn shutdown_vcpus(&self) {
        self.common
            .vcpu_finish_requested
            .store(true, Ordering::SeqCst);
        let mut handles = self.vcpus_handles();
        for (idx, handle) in handles.iter_mut().enumerate() {
            if let Err(err) = handle.send_event(VcpuEvent::Finish) {
                eprintln!(
                    "warning: failed to send VcpuEvent::Finish to vCPU {}: {}",
                    idx, err
                );
            }
        }
        // Join the vCPU threads by running VcpuHandle::drop().
        handles.clear();
    }

    /// Pauses every vCPU: each leaves `KVM_RUN` and blocks until it is resumed
    /// or the VM finishes. Fails, naming the vCPU, if one does not answer in
    /// time. On failure the vCPUs that did pause stay paused; the caller
    /// resumes or finishes them.
    pub fn pause_vcpus(&self) -> Result<(), VmError> {
        for (vcpu, response) in self
            .control_vcpus(VcpuEvent::Pause, "pause")?
            .into_iter()
            .enumerate()
        {
            match response {
                VcpuResponse::Paused => {}
                other => return Err(vcpu_failure(vcpu, "pause", other)),
            }
        }
        Ok(())
    }

    /// Resumes every paused vCPU. Fails, naming the vCPU, if one does not
    /// answer in time.
    pub fn resume_vcpus(&self) -> Result<(), VmError> {
        for (vcpu, response) in self
            .control_vcpus(VcpuEvent::Resume, "resume")?
            .into_iter()
            .enumerate()
        {
            match response {
                VcpuResponse::Resumed => {}
                other => return Err(vcpu_failure(vcpu, "resume", other)),
            }
        }
        Ok(())
    }

    /// Saves the state of every vCPU, in index order. The vCPUs must be paused:
    /// a running vCPU refuses, and that refusal is the error returned.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    pub fn save_vcpu_states(&self) -> Result<Vec<crate::vstate::snapshot::VcpuState>, VmError> {
        self.control_vcpus(VcpuEvent::SaveState, "save state")?
            .into_iter()
            .enumerate()
            .map(|(vcpu, response)| match response {
                VcpuResponse::SavedState(state) => Ok(*state),
                other => Err(vcpu_failure(vcpu, "save state", other)),
            })
            .collect()
    }

    /// Sends `event` to every vCPU, then waits for each one's answer, in index
    /// order.
    ///
    /// Every vCPU gets the request before any answer is awaited, so they act
    /// at about the same time. The handles lock is held for the whole exchange,
    /// so a concurrent shutdown cannot interleave with it.
    fn control_vcpus(
        &self,
        event: VcpuEvent,
        action: &'static str,
    ) -> Result<Vec<VcpuResponse>, VmError> {
        let mut handles = self.vcpus_handles();
        for (vcpu, handle) in handles.iter_mut().enumerate() {
            handle
                .send_event(event.clone())
                .map_err(|err| vcpu_error(vcpu, action, err.to_string()))?;
        }
        handles
            .iter()
            .enumerate()
            .map(|(vcpu, handle)| {
                await_response(
                    vcpu,
                    action,
                    handle.response_receiver(),
                    VCPU_RESPONSE_TIMEOUT,
                )
            })
            .collect()
    }

    /// Reserves the next `slot_cnt` contiguous kvm slot ids and returns the first one
    pub fn next_kvm_slot(&self, slot_cnt: u32) -> Option<u32> {
        let next = self
            .common
            .next_kvm_slot
            .fetch_add(slot_cnt, Ordering::Relaxed);
        if self.common.max_memslots <= next {
            None
        } else {
            Some(next)
        }
    }

    pub(crate) fn set_user_memory_region(
        &self,
        region: kvm_userspace_memory_region,
    ) -> Result<(), VmError> {
        // SAFETY: Safe because the fd is a valid KVM file descriptor.
        unsafe {
            self.fd()
                .set_user_memory_region(region)
                .map_err(VmError::SetUserMemoryRegion)
        }
    }

    fn register_memory_region(&mut self, region: Arc<GuestRegionMmapExt>) -> Result<(), VmError> {
        let new_guest_memory = self
            .common
            .guest_memory
            .insert_region(Arc::clone(&region))?;

        self.set_user_memory_region(kvm_userspace_memory_region::from(region.as_ref()))?;

        self.common.guest_memory = new_guest_memory;

        Ok(())
    }

    /// Register a list of new memory regions to this [`KvmVm`].
    pub fn register_dram_memory_regions(
        &mut self,
        regions: Vec<GuestRegionMmap>,
    ) -> Result<(), VmError> {
        for region in regions {
            let next_slot = self
                .next_kvm_slot(1)
                .ok_or(VmError::NotEnoughMemorySlots(self.common.max_memslots))?;

            let arcd_region =
                Arc::new(GuestRegionMmapExt::dram_from_mmap_region(region, next_slot));

            self.register_memory_region(arcd_region)?
        }

        Ok(())
    }

    /// Gets a reference to the kvm file descriptor owned by this VM.
    pub fn fd(&self) -> &VmFd {
        &self.common.fd
    }

    /// Gets a reference to this [`KvmVm`]'s [`GuestMemoryMmap`] object
    pub fn guest_memory(&self) -> &GuestMemoryMmap {
        &self.common.guest_memory
    }

    /// Gets a mutable reference to this [`KvmVm`]'s [`ResourceAllocator`] object
    pub fn resource_allocator(&self) -> MutexGuard<'_, ResourceAllocator> {
        lock(&self.common.resource_allocator)
    }

    /// Register a device IRQ
    ///
    /// Upstream additionally records a routing entry for the later
    /// `KVM_SET_GSI_ROUTING` programming (needed once virtio/MSI devices
    /// exist); this port relies on the in-kernel default irqchip routing, so
    /// only the irqfd is programmed.
    pub fn register_irq(&self, fd: &EventFd, gsi: u32) -> Result<(), errno::Error> {
        self.common.fd.register_irqfd(fd, gsi)?;
        Ok(())
    }
}

fn vcpu_error(vcpu: usize, action: &'static str, reason: String) -> VmError {
    VmError::VcpuControl {
        vcpu,
        action,
        reason,
    }
}

/// Waits for one vCPU's answer. A vCPU whose thread has exited disconnects its
/// response channel, so that is reported as such rather than as a timeout.
fn await_response(
    vcpu: usize,
    action: &'static str,
    responses: &Receiver<VcpuResponse>,
    timeout: Duration,
) -> Result<VcpuResponse, VmError> {
    responses.recv_timeout(timeout).map_err(|err| match err {
        RecvTimeoutError::Timeout => {
            vcpu_error(vcpu, action, format!("did not answer within {timeout:?}"))
        }
        RecvTimeoutError::Disconnected => vcpu_error(vcpu, action, "its thread exited".to_string()),
    })
}

/// The error for a vCPU that answered a request with something other than its
/// success answer: the vCPU's own reason when it gave one.
fn vcpu_failure(vcpu: usize, action: &'static str, response: VcpuResponse) -> VmError {
    let reason = match response {
        VcpuResponse::Error(reason) | VcpuResponse::NotAllowed(reason) => reason,
        other => format!("answered with an unexpected response: {other:?}"),
    };
    vcpu_error(vcpu, action, reason)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn a_vcpu_answer_is_read_from_its_own_channel() {
        let (sender, receiver) = channel();
        sender.send(VcpuResponse::Paused).unwrap();
        let response = await_response(2, "pause", &receiver, Duration::from_millis(10)).unwrap();
        assert!(matches!(response, VcpuResponse::Paused), "got {response:?}");
    }

    #[test]
    fn a_silent_vcpu_times_out_and_is_named() {
        // The sender is kept alive so the wait ends by timeout, not disconnect.
        let (_sender, receiver) = channel::<VcpuResponse>();
        let err = await_response(2, "pause", &receiver, Duration::from_millis(10)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("vCPU 2") && msg.contains("pause"), "{msg}");
        assert!(msg.contains("did not answer"), "{msg}");
    }

    #[test]
    fn a_vcpu_whose_thread_exited_is_named_and_not_a_timeout() {
        let (sender, receiver) = channel::<VcpuResponse>();
        drop(sender);
        let err = await_response(1, "save state", &receiver, Duration::from_secs(5)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("vCPU 1") && msg.contains("exited"), "{msg}");
    }

    #[test]
    fn a_refusal_or_failure_from_a_vcpu_carries_its_reason() {
        let refused = vcpu_failure(
            0,
            "save state",
            VcpuResponse::NotAllowed("save/restore unavailable while running".into()),
        );
        let msg = refused.to_string();
        assert!(
            msg.contains("vCPU 0") && msg.contains("while running"),
            "{msg}"
        );

        let failed = vcpu_failure(3, "save state", VcpuResponse::Error("boom".into()));
        let msg = failed.to_string();
        assert!(msg.contains("vCPU 3") && msg.contains("boom"), "{msg}");
    }

    #[test]
    fn an_answer_of_the_wrong_kind_is_an_error_naming_the_vcpu() {
        let err = vcpu_failure(1, "pause", VcpuResponse::Resumed);
        let msg = err.to_string();
        assert!(
            msg.contains("vCPU 1") && msg.contains("unexpected"),
            "{msg}"
        );
    }
}
