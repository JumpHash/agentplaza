// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: Firecracker's builder boots through KVM, and this is
// the equivalent wiring for Hypervisor.framework. It reuses the shared
// aarch64 arch module for the memory map, the kernel loader and the device
// tree, exactly as the KVM builder does.

//! Building and booting one microVM on Hypervisor.framework.
//!
//! The order mirrors the KVM backend: allocate guest memory, create the VM and
//! map the memory into it, load the kernel (and the optional initrd), build
//! the device tree, wire the devices, and start the vCPU threads. What differs
//! is who owns which mapping: Hypervisor.framework has no in-kernel interrupt
//! controller here, so the controller and the console live in this process.

use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use linux_loader::loader::Cmdline;
use vm_memory::{Address, Bytes};

use crate::arch::aarch64::layout::{CMDLINE_MAX_SIZE, MMIO_LEN, SERIAL_MEM_START};
use crate::arch::aarch64::{
    FdtError, GicFdt, InitrdConfig, InitrdError, SerialFdtInfo, arch_memory_regions, create_fdt,
    get_fdt_addr, load_kernel,
};
use crate::hvf::gic::{self, GICV3_DIST_BASE, GICV3_DIST_SIZE, GICV3_REDIST_BASE};
use crate::hvf::sys::HvfError;
use crate::hvf::uart::{ConsoleLine, SerialDevice};
use crate::hvf::vcpu::{BootParams, MAX_VCPUS, SharedVm, run_vcpu};
use crate::hvf::vm::HvfVm;
use crate::spawner::{SpawnError, VmRuntime, VmSpec};
use crate::vstate::memory::{
    GuestAddress, GuestMemoryMmap, GuestRegionMmapExt, MemoryError, anonymous, mib_to_bytes,
};

/// How long `boot` waits for the vCPUs to come up before reporting that the
/// VM did not start. Creating a vCPU is a call into the framework, so this is
/// a failure detector, not a scheduling policy.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors from `boot`.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// The snapshot could not be read or does not fit the spec.
    #[error("cannot restore the snapshot: {0}")]
    Snapshot(String),
    /// No vCPUs were requested
    #[error("no vCPUs were requested")]
    NoVcpus,
    /// {requested} vCPUs were requested, but this backend runs at most {max}
    #[error("{requested} vCPUs were requested, but this backend runs at most {max}")]
    TooManyVcpus {
        /// The number of vCPUs the caller asked for.
        requested: u8,
        /// The number this backend can run.
        max: usize,
    },
    /// Cannot open the kernel image {path}: {source}
    #[error("cannot open the kernel image {path}: {source}")]
    Kernel {
        /// Path of the kernel image that could not be opened.
        path: PathBuf,
        /// The error the open failed with.
        source: io::Error,
    },
    /// Cannot open the initrd image {path}: {source}
    #[error("cannot open the initrd image {path}: {source}")]
    Initrd {
        /// Path of the initrd image that could not be opened.
        path: PathBuf,
        /// The error the open failed with.
        source: io::Error,
    },
    /// Failed to allocate guest memory: {0}
    #[error("failed to allocate guest memory: {0}")]
    Memory(#[from] MemoryError),
    /// Failed to assemble guest memory: {0}
    #[error("failed to assemble guest memory: {0}")]
    GuestMemory(#[from] vm_memory::region::GuestRegionCollectionError),
    /// Failed to load the initrd: {0}
    #[error("failed to load the initrd: {0}")]
    InitrdLoad(#[from] InitrdError),
    /// Failed to load the kernel: {0}
    #[error("failed to load the kernel: {0}")]
    Loader(#[from] crate::arch::aarch64::ConfigurationError),
    /// Failed to build the kernel command line: {0}
    #[error("failed to build the kernel command line: {0}")]
    CommandLine(#[from] linux_loader::cmdline::Error),
    /// Failed to create the device tree: {0}
    #[error("failed to create the device tree: {0}")]
    Fdt(#[from] FdtError),
    /// Failed to write the device tree into guest memory: {0}
    #[error("failed to write the device tree into guest memory: {0}")]
    FdtWrite(#[from] vm_memory::GuestMemoryError),
    /// Failed to create the VM: {0}
    #[error("failed to create the VM: {0}")]
    Vm(#[from] HvfError),
    /// Failed to create a VM thread: {0}
    #[error("failed to create a VM thread: {0}")]
    Thread(io::Error),
    /// The VM asked for a vsock device, which this backend cannot provide
    #[error(
        "this VM asked for a vsock device, which the Apple Silicon backend cannot provide: \
         Hypervisor.framework has no vhost-vsock interface, and this port has no network device \
         to fall back to"
    )]
    UnsupportedVsock,
    /// The VM asked for a workspace volume, which this backend cannot provide
    #[error(
        "this VM asked for a workspace volume, which the Apple Silicon backend cannot provide: \
         its virtio-mmio transports have no device model on Hypervisor.framework (no block \
         device is implemented for that backend)"
    )]
    UnsupportedWorkspace,
    /// Shared data volumes are virtio-mmio block devices too, which this backend does not have.
    #[error(
        "this VM asked for shared data volumes, which Hypervisor.framework VMs cannot carry yet \
         (no block device is implemented for that backend)"
    )]
    UnsupportedVolumes,
    /// The VM did not start: {0}
    #[error("the VM did not start: {0}")]
    Startup(String),
}

/// Boots the VM described by `spec` and returns once its vCPUs are running.
pub fn boot(spec: &VmSpec, id: u32, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    match &spec.restore {
        Some(dir) => restore_inner(spec, dir, id, &runtime),
        None => boot_inner(spec, id, &runtime),
    }
    .map_err(SpawnError::backend)
}

/// Refuses what this backend cannot build, before anything is; returns the vCPU count.
fn check_spec(spec: &VmSpec) -> Result<usize, BootError> {
    // A vsock device is what a VM with a job needs (it is the only way to reach
    // the orchestrator), so a spec that asks for one must not boot a VM that
    // cannot have it: this backend has no vhost-vsock (Hypervisor.framework
    // exposes no such interface) and this port has no network device to fall
    // back to.
    if spec.vsock.is_some() {
        return Err(BootError::UnsupportedVsock);
    }
    // A workspace volume is a virtio-mmio block device, and this backend has no
    // virtio-mmio device model at all: refusing the spec keeps a caller from
    // booting a guest that would wait forever for a device that never appears.
    if spec.workspace.is_some() {
        return Err(BootError::UnsupportedWorkspace);
    }
    if !spec.volumes.is_empty() {
        return Err(BootError::UnsupportedVolumes);
    }

    let vcpu_count = usize::from(spec.vcpus);
    if vcpu_count == 0 {
        return Err(BootError::NoVcpus);
    }
    if vcpu_count > MAX_VCPUS {
        return Err(BootError::TooManyVcpus {
            requested: spec.vcpus,
            max: MAX_VCPUS,
        });
    }
    Ok(vcpu_count)
}

/// The guest memory, not yet mapped.
fn guest_memory(spec: &VmSpec) -> Result<GuestMemoryMmap, BootError> {
    // Guest memory: the aarch64 layout's DRAM regions (one for any size this
    // port boots, two only past the 64-bit MMIO hole). The regions are the
    // process's own pages, which is what `hv_vm_map` maps.
    let regions = arch_memory_regions(mib_to_bytes(spec.memory_mib));
    let raw = anonymous(&regions)?;
    let memory = GuestMemoryMmap::from_regions(
        raw.into_iter()
            .enumerate()
            .map(|(slot, region)| {
                GuestRegionMmapExt::dram_from_mmap_region(region, u32::try_from(slot).unwrap_or(0))
            })
            .collect(),
    )?;
    Ok(memory)
}

fn boot_inner(spec: &VmSpec, id: u32, runtime: &Arc<VmRuntime>) -> Result<(), BootError> {
    let vcpu_count = check_spec(spec)?;
    let memory = guest_memory(spec)?;

    // The VM, and the guest memory mapped into its address space. A device
    // access is a load or store to an address this leaves unmapped: that is
    // what makes it exit to this process.
    let mut vm = HvfVm::create()?;
    vm.map_guest_memory(&memory)?;

    // Kernel, optional initrd, command line.
    let kernel = File::open(&spec.kernel).map_err(|source| BootError::Kernel {
        path: spec.kernel.clone(),
        source,
    })?;
    let entry_point = load_kernel(&kernel, &memory)?;
    let initrd = match &spec.initrd {
        Some(path) => {
            let file = File::open(path).map_err(|source| BootError::Initrd {
                path: path.clone(),
                source,
            })?;
            Some(InitrdConfig::from_file(&memory, file)?)
        }
        None => None,
    };
    let cmdline = Cmdline::try_from(
        super::guest_cmdline(&spec.cmdline).as_str(),
        CMDLINE_MAX_SIZE,
    )?;
    let cmdline = cmdline.as_cstring()?;

    // The device tree, written above the guest's memory where the arm64 boot
    // protocol expects it (the same address the KVM backend uses).
    let fdt = create_fdt(
        &memory,
        (0..vcpu_count as u64).collect(),
        cmdline,
        &SerialFdtInfo {
            addr: SERIAL_MEM_START,
            len: MMIO_LEN,
            gsi: gic::UART_SPI,
        },
        // This backend has no virtio-mmio devices at all (no vsock device, and
        // no block device either), so no transport is described.
        &[],
        &GicFdtNode::new(vcpu_count),
        &initrd,
    )?;
    let fdt_addr = get_fdt_addr(&memory);
    memory.write_slice(&fdt, GuestAddress(fdt_addr))?;

    // The state the vCPUs share. It owns the VM and its memory: the framework
    // maps these host pages into the guest, so they have to outlive every
    // vCPU, and the VM has to be destroyed before the memory is unmapped.
    let shared = SharedVm::new(
        vm,
        memory,
        Arc::clone(runtime),
        vcpu_count,
        BootParams {
            entry: entry_point.entry_addr.raw_value(),
            fdt: fdt_addr,
        },
        None,
    );
    run(spec, id, runtime, shared, None)
}

/// Restores the VM from the snapshot in `dir`: the guest memory read back before it is mapped, the
/// interrupt controller, the UART and the vCPUs' power states put back, and each vCPU's registers
/// written by its own thread before it first runs. The guest's virtual counter goes on from the
/// snapshot's.
fn restore_inner(
    spec: &VmSpec,
    dir: &std::path::Path,
    id: u32,
    runtime: &Arc<VmRuntime>,
) -> Result<(), BootError> {
    let vcpu_count = check_spec(spec)?;
    let snapshot = crate::hvf::snapshot_state::read_state(dir)
        .map_err(|err| BootError::Snapshot(err.to_string()))?;
    if snapshot.vcpus.len() != vcpu_count || snapshot.meta.memory_mib != spec.memory_mib {
        return Err(BootError::Snapshot(format!(
            "the spec ({vcpu_count} vCPUs, {} MiB) is not the snapshot's machine ({} vCPUs, {} MiB)",
            spec.memory_mib,
            snapshot.vcpus.len(),
            snapshot.meta.memory_mib
        )));
    }
    let gic = gic::Gic::from_state(&snapshot.gic, vcpu_count).map_err(BootError::Snapshot)?;
    let memory = guest_memory(spec)?;
    crate::hvf::snapshot::fill_memory(&memory, dir, &snapshot).map_err(BootError::Snapshot)?;
    let mut vm = HvfVm::create()?;
    vm.map_guest_memory(&memory)?;
    let restore = crate::hvf::vcpu::Restore {
        vtimer_offset: crate::hvf::snapshot_state::vtimer_offset_for(
            crate::hvf::vcpu::host_now(),
            snapshot.guest_counter,
        ),
        running: snapshot.running.clone(),
        gic,
        vcpus: snapshot.vcpus.clone(),
    };
    // The entry point and device tree are the boot's; a restored vCPU goes on from its registers.
    let shared = SharedVm::new(
        vm,
        memory,
        Arc::clone(runtime),
        vcpu_count,
        BootParams { entry: 0, fdt: 0 },
        Some(restore),
    );
    run(spec, id, runtime, shared, Some(&snapshot.serial))
}

/// The rest of a boot or a restore, once the shared VM state exists: the console (new, or with the
/// registers a snapshot kept), the input, stop and snapshot controls, and the vCPU threads.
fn run(
    spec: &VmSpec,
    id: u32,
    runtime: &Arc<VmRuntime>,
    shared: Arc<SharedVm>,
    serial: Option<&crate::vstate::snapshot::SerialRegisters>,
) -> Result<(), BootError> {
    let vcpu_count = usize::from(spec.vcpus);
    let console: Arc<dyn ConsoleLine> = shared.clone();
    shared.install_serial(match serial {
        Some(registers) => {
            SerialDevice::from_registers(registers, Arc::clone(&runtime.serial), &console)
                .map_err(BootError::Snapshot)?
        }
        None => SerialDevice::new(Arc::clone(&runtime.serial), &console),
    });

    // The console's input side, the counterpart of the capture above: a
    // caller's bytes go to the UART, which queues them and raises the
    // console's SPI. Installed right after the console itself (so no input can
    // arrive before there is a console to take it) and holding the VM weakly -
    // a strong reference would keep the VM, its vCPU threads and its guest
    // memory alive for as long as the spawner's handle exists.
    let input_vm = Arc::downgrade(&shared);
    runtime.set_input_hook(move |data| match input_vm.upgrade() {
        Some(shared) => shared.serial_input(data),
        None => Err("the VM has stopped".to_string()),
    });

    // The stop control goes in before the vCPUs start, so that a VM which
    // fails to come up can still be stopped (and so `VmHandle::kill` works
    // from the moment `spawn` returns). It holds a weak reference: the VM and
    // its memory must be released as soon as its last vCPU has finished, not
    // kept alive until the handle that asked it to stop is dropped.
    let stop = Arc::downgrade(&shared);
    runtime.set_stop_control(move || {
        if let Some(shared) = stop.upgrade() {
            shared.stop();
        }
    });

    // The snapshot control, holding the VM weakly as the stop control does.
    runtime.set_vm_control(Arc::new(crate::hvf::snapshot::HvfControl {
        vm: Arc::downgrade(&shared),
        vcpus: spec.vcpus,
        memory_mib: spec.memory_mib,
        busy: std::sync::Mutex::new(()),
        paused: std::sync::atomic::AtomicBool::new(false),
    }));

    for index in 0..vcpu_count {
        let shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name(format!("apz_vm{id}_vcpu{index}"))
            .spawn(move || run_vcpu(&shared, index))
            .map_err(BootError::Thread)?;
        runtime.add_thread(handle);
    }

    // Report a backend that could not start its vCPUs instead of pretending
    // the VM is up. The vCPUs have to be stopped *and joined* before the error
    // is returned: they own the VM and its guest memory, and this caller never
    // gets a handle that could stop them, so a thread left behind would keep
    // running until the process exits.
    if let Some(failure) = shared.wait_until_started(START_TIMEOUT) {
        shared.stop();
        return Err(match runtime.join_threads() {
            Ok(()) => BootError::Startup(failure),
            Err(err) => BootError::Startup(format!(
                "{failure}, and stopping the vCPUs left behind failed: {err}"
            )),
        });
    }
    Ok(())
}

/// The interrupt controller as the device tree describes it: an `arm,gic-v3`
/// with a distributor, one redistributor per vCPU, and the maintenance
/// interrupt. The addresses are the ones this backend's userspace controller
/// answers at; the guest finds them in the device tree.
struct GicFdtNode {
    /// The `reg` cells: distributor base and size, then redistributor base and
    /// size.
    properties: [u64; 4],
}

impl GicFdtNode {
    fn new(vcpu_count: usize) -> Self {
        let (_, redist_size) = gic::redist_window(vcpu_count);
        Self {
            properties: [
                GICV3_DIST_BASE,
                GICV3_DIST_SIZE,
                GICV3_REDIST_BASE,
                redist_size,
            ],
        }
    }
}

impl GicFdt for GicFdtNode {
    fn compatible(&self) -> &str {
        "arm,gic-v3"
    }

    fn device_properties(&self) -> &[u64] {
        &self.properties
    }

    fn fdt_maint_irq(&self) -> u32 {
        gic::MAINTENANCE_PPI
    }

    fn msi_properties(&self) -> Option<&[u64]> {
        // No ITS: the only interrupt source is the serial console's SPI, and
        // wire interrupts do not need a message-based controller.
        None
    }
}
