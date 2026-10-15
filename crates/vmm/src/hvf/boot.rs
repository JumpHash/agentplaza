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
//!
//! The virtio devices (the vsock device, the workspace volume and the shared
//! data volumes) are the KVM backend's, on the same virtio-mmio transports at
//! the same windows and GSIs (`arch::aarch64::layout`). What replaces KVM's
//! irqfds is an *interrupt relay*, as on WHP: the devices signal `EventFd`s,
//! and a relay thread polls them and raises the matching SPI of the userspace
//! GICv3, which wakes the vCPU it is routed to.

use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;
use std::time::Duration;

use linux_loader::loader::Cmdline;
use vm_memory::{Address, Bytes};

use crate::arch::aarch64::layout::{
    CMDLINE_MAX_SIZE, DATA_GSI_START, DATA_MMIO_START, MMIO_LEN, SERIAL_MEM_START, VSOCK_GSI,
    VSOCK_MMIO_START, WORKSPACE_GSI, WORKSPACE_MMIO_START,
};
use crate::arch::aarch64::{
    FdtError, GicFdt, InitrdConfig, InitrdError, SerialFdtInfo, VirtioMmioFdtInfo,
    arch_memory_regions, create_fdt, get_fdt_addr, load_kernel,
};
use crate::devices::virtio::VirtioError;
use crate::devices::virtio::mmio::{MmioDeviceState, VirtioMmioTransport};
use crate::devices::virtio::vsock::event_thread::{self, DeviceGate};
use crate::devices::virtio::vsock::mmio_device::VsockMmioDevice;
use crate::devices::virtio::vsock::unix::VsockUnixBackend;
use crate::devices::virtio::vsock::{Vsock, VsockError};
use crate::hvf::gic::{self, GICV3_DIST_BASE, GICV3_DIST_SIZE, GICV3_REDIST_BASE};
use crate::hvf::snapshot_state::HvfSnapshot;
use crate::hvf::sys::HvfError;
use crate::hvf::uart::{ConsoleLine, SerialDevice};
use crate::hvf::vcpu::{BootParams, ExitSignal, MAX_VCPUS, SharedVm, run_vcpu};
use crate::hvf::vm::HvfVm;
use crate::pal::AsRawFd;
use crate::pal::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use crate::pal::eventfd::{EFD_NONBLOCK, EventFd};
use crate::spawner::{MAX_DATA_VOLUMES, SpawnError, VmRuntime, VmSpec};
use crate::vstate::memory::{
    GuestAddress, GuestMemoryMmap, GuestRegionMmapExt, MemoryError, anonymous, mib_to_bytes,
};

/// How long `boot` waits for the vCPUs to come up before reporting that the
/// VM did not start. Creating a vCPU is a call into the framework, so this is
/// a failure detector, not a scheduling policy.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `spi` is one of the virtio transports' SPIs (the layout's GSIs).
const fn is_virtio_spi(spi: u32) -> bool {
    spi == VSOCK_GSI
        || spi == WORKSPACE_GSI
        || (spi >= DATA_GSI_START && spi - DATA_GSI_START < MAX_DATA_VOLUMES as u32)
}

// The console and the virtio transports share the controller's SPIs: each
// needs its own, and every one must be an SPI the controller implements.
const _: () = {
    let spis = (gic::NUM_INTIDS - gic::NUM_PRIVATE_INTIDS) as u32;
    assert!(!is_virtio_spi(gic::UART_SPI));
    assert!(DATA_GSI_START + MAX_DATA_VOLUMES as u32 <= spis);
};

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
    /// The data volumes are not ones this VM can carry.
    #[error("{0}")]
    Volumes(String),
    /// A device's event could not be created.
    #[error("failed to create a device: {0}")]
    Device(io::Error),
    /// A virtio transport could not be put on the MMIO bus.
    #[error("failed to register a device on the MMIO bus: {0}")]
    Bus(#[from] crate::vstate::bus::BusError),
    /// A virtio device could not be created or restored.
    #[error("failed to create a virtio device: {0}")]
    Virtio(#[from] VirtioError),
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
fn check_spec(spec: &VmSpec, id: u32) -> Result<usize, BootError> {
    crate::spawner::check_volumes(&spec.volumes).map_err(BootError::Volumes)?;
    if spec.balloon.is_some() {
        eprintln!(
            "hvf: VM {id}: the balloon is Linux-only; booting with all {} MiB usable",
            spec.memory_mib
        );
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
    let vcpu_count = check_spec(spec, id)?;
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
        // The vsock device, the workspace and the data volumes, in the
        // layout's slots: the same nodes the KVM backend writes.
        &transports_of(spec),
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
        ExitSignal::new().map_err(BootError::Device)?,
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
    let vcpu_count = check_spec(spec, id)?;
    let snapshot = crate::hvf::snapshot_state::read_state(dir)
        .map_err(|err| BootError::Snapshot(err.to_string()))?;
    if snapshot.vcpus.len() != vcpu_count
        || snapshot.meta.memory_mib != spec.memory_mib
        || snapshot.vsock.is_some() != spec.vsock.is_some()
        || snapshot.workspace.is_some() != spec.workspace.is_some()
        || snapshot.volumes.len() != spec.volumes.len()
    {
        return Err(BootError::Snapshot(format!(
            "the spec ({vcpu_count} vCPUs, {} MiB, vsock {}, workspace {}, {} data volumes) is not \
             the snapshot's machine ({} vCPUs, {} MiB, vsock {}, workspace {}, {} data volumes)",
            spec.memory_mib,
            spec.vsock.is_some(),
            spec.workspace.is_some(),
            spec.volumes.len(),
            snapshot.vcpus.len(),
            snapshot.meta.memory_mib,
            snapshot.vsock.is_some(),
            snapshot.workspace.is_some(),
            snapshot.volumes.len()
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
        ExitSignal::new().map_err(BootError::Device)?,
    );
    run(spec, id, runtime, shared, Some(&snapshot))
}

/// The rest of a boot or a restore, once the shared VM state exists: the console and the virtio
/// devices (new, or rebuilt from the state a snapshot kept), the input, stop and snapshot controls,
/// and the device and vCPU threads.
fn run(
    spec: &VmSpec,
    id: u32,
    runtime: &Arc<VmRuntime>,
    shared: Arc<SharedVm>,
    restored: Option<&HvfSnapshot>,
) -> Result<(), BootError> {
    let vcpu_count = usize::from(spec.vcpus);
    let console: Arc<dyn ConsoleLine> = shared.clone();
    shared.install_serial(match restored {
        Some(snapshot) => {
            SerialDevice::from_registers(&snapshot.serial, Arc::clone(&runtime.serial), &console)
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

    // The virtio devices, their threads and the interrupt relay. A failure
    // stops the device threads already started before it is reported: no vCPU
    // runs yet, and nothing else would ever tell them the VM is gone.
    // The vsock event thread takes the gate for reading around its work; a
    // snapshot holds it for writing, so no packet lands between the device
    // state and the memory dump.
    let gate: DeviceGate = Arc::new(RwLock::new(()));
    let transports = match attach_devices(spec, runtime, &shared, &gate, restored) {
        Ok(transports) => transports,
        Err(err) => {
            shared.stop();
            return Err(match runtime.join_threads() {
                Ok(()) => err,
                Err(join) => BootError::Startup(format!(
                    "{err}, and stopping the device threads left behind failed: {join}"
                )),
            });
        }
    };

    // The snapshot control, holding the VM and its devices weakly as the stop
    // control does.
    runtime.set_vm_control(Arc::new(crate::hvf::snapshot::HvfControl {
        vm: Arc::downgrade(&shared),
        vsock: transports.vsock.as_ref().map(Arc::downgrade),
        workspace: transports.workspace.as_ref().map(Arc::downgrade),
        volumes: transports.volumes.iter().map(Arc::downgrade).collect(),
        gate,
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

/// The virtio-mmio transports a VM of `spec` carries, as its device tree
/// describes them: the vsock device, the workspace volume and the shared data
/// volumes, in the aarch64 layout's slots and GSIs (the KVM backend's).
fn transports_of(spec: &VmSpec) -> Vec<VirtioMmioFdtInfo> {
    let mut transports = Vec::new();
    if spec.vsock.is_some() {
        transports.push(VirtioMmioFdtInfo {
            addr: VSOCK_MMIO_START,
            len: MMIO_LEN,
            gsi: VSOCK_GSI,
        });
    }
    if spec.workspace.is_some() {
        transports.push(VirtioMmioFdtInfo {
            addr: WORKSPACE_MMIO_START,
            len: MMIO_LEN,
            gsi: WORKSPACE_GSI,
        });
    }
    for index in 0..spec.volumes.len() {
        transports.push(VirtioMmioFdtInfo {
            addr: DATA_MMIO_START + index as u64 * MMIO_LEN,
            len: MMIO_LEN,
            gsi: DATA_GSI_START + index as u32,
        });
    }
    transports
}

/// The VM's virtio transports, held strongly by the VM itself (see
/// `SharedVm::attach_transport`); these handles are for the snapshot control.
struct Transports {
    vsock: Option<Arc<Mutex<VirtioMmioTransport>>>,
    workspace: Option<Arc<Mutex<VirtioMmioTransport>>>,
    volumes: Vec<Arc<Mutex<VirtioMmioTransport>>>,
}

/// One device interrupt: the event the transport signals, and the SPI the
/// relay raises for it.
type IrqSource = (EventFd, u32);

/// Builds the virtio devices of `spec` (rebuilt from `restored`'s state for a
/// restore), puts their transports on the VM's MMIO bus, and starts the vsock
/// event thread and the interrupt relay.
fn attach_devices(
    spec: &VmSpec,
    runtime: &Arc<VmRuntime>,
    shared: &Arc<SharedVm>,
    gate: &DeviceGate,
    restored: Option<&HvfSnapshot>,
) -> Result<Transports, BootError> {
    let mut irq_sources: Vec<IrqSource> = Vec::new();
    // SPIs to raise once the devices are up: a restored transport whose
    // interrupt the guest had not acknowledged yet. A pending SPI is in the
    // controller's state already unless the relay raised it after the
    // controller was saved; an extra edge is harmless (the driver reads the
    // transport's interrupt status first).
    let mut pending: Vec<u32> = Vec::new();

    // The block devices: the workspace volume and the shared data volumes.
    // Their requests are served on the vCPU that rings their doorbell, so they
    // need no thread of their own.
    let workspace = match &spec.workspace {
        Some(image) => {
            let state = restored.and_then(|s| s.workspace.as_ref());
            note_pending(&mut pending, state, WORKSPACE_GSI);
            Some(attach_block(
                shared,
                image,
                None,
                WORKSPACE_MMIO_START,
                WORKSPACE_GSI,
                state,
                &mut irq_sources,
            )?)
        }
        None => None,
    };
    let mut volumes = Vec::with_capacity(spec.volumes.len());
    for (index, volume) in spec.volumes.iter().enumerate() {
        let gsi = DATA_GSI_START + index as u32;
        let state = restored.and_then(|s| s.volumes.get(index));
        note_pending(&mut pending, state, gsi);
        volumes.push(attach_block(
            shared,
            &volume.image,
            Some(&volume.name),
            DATA_MMIO_START + index as u64 * MMIO_LEN,
            gsi,
            state,
            &mut irq_sources,
        )?);
    }

    // The vsock device: the userspace device and its muxer, which binds
    // `uds_path` (a taken path fails the boot here), and its ring-processing
    // event thread. It comes after the block devices, so a volume that cannot
    // be opened fails the boot before a socket is bound or a thread started.
    let vsock = match &spec.vsock {
        Some(config) => {
            let irq = new_irq(&mut irq_sources, VSOCK_GSI)?;
            let backend = VsockUnixBackend::new(
                config.guest_cid,
                config.uds_path.to_string_lossy().into_owned(),
            )
            .map_err(VsockError::from)
            .map_err(VirtioError::from)?;
            let device = Arc::new(Mutex::new(
                Vsock::new(config.guest_cid, backend).map_err(VirtioError::from)?,
            ));
            let exit = shared.exit_signal();
            let handle = event_thread::start_event_thread(
                Arc::clone(&device),
                exit.evt.try_clone().map_err(BootError::Device)?,
                Arc::clone(&exit.signalled),
                Arc::clone(gate),
                runtime.id,
            )
            .map_err(BootError::Thread)?;
            runtime.add_thread(handle);
            let device = Box::new(VsockMmioDevice::new(device));
            let state = restored.and_then(|s| s.vsock.as_ref());
            let transport = match state {
                Some(state) => {
                    let mut transport =
                        VirtioMmioTransport::restore(device, shared.memory().clone(), irq, state)?;
                    // The muxer's connections did not survive: the guest is told so, and re-reads
                    // its CID (a restore may give the VM another).
                    transport.on_restored()?;
                    transport
                }
                None => VirtioMmioTransport::new(device, shared.memory().clone(), irq),
            };
            note_pending(&mut pending, state, VSOCK_GSI);
            Some(attach(shared, transport, VSOCK_MMIO_START)?)
        }
        None => None,
    };

    // The interrupt relay, when there is anything to relay. It holds the VM
    // weakly, upgrading only to raise an SPI, so it never keeps a stopped VM
    // alive. If that upgrade is the last reference when the vCPUs finish, the
    // VM's teardown runs on the relay thread: that is safe, since every vCPU
    // has destroyed its own vCPU by then and `hv_vm_destroy` and the memory
    // unmap have no thread affinity.
    if !irq_sources.is_empty() {
        let exit = shared.exit_signal();
        let exit_evt = exit.evt.try_clone().map_err(BootError::Device)?;
        let exit_signalled = Arc::clone(&exit.signalled);
        let vm = Arc::downgrade(shared);
        let relay = thread::Builder::new()
            .name(format!("apz_vm{}_irq", runtime.id))
            .spawn(move || relay_interrupts(&vm, &exit_evt, &exit_signalled, irq_sources))
            .map_err(BootError::Thread)?;
        runtime.add_thread(relay);
    }
    for spi in pending {
        shared.raise_spi(spi);
    }

    Ok(Transports {
        vsock,
        workspace,
        volumes,
    })
}

/// A new interrupt event for the device on SPI `spi`, registered with the relay.
fn new_irq(irq_sources: &mut Vec<IrqSource>, spi: u32) -> Result<EventFd, BootError> {
    let irq = EventFd::new(EFD_NONBLOCK).map_err(BootError::Device)?;
    irq_sources.push((irq.try_clone().map_err(BootError::Device)?, spi));
    Ok(irq)
}

/// Records `spi` for raising when a restored transport had an unacknowledged interrupt.
fn note_pending(pending: &mut Vec<u32>, state: Option<&MmioDeviceState>, spi: u32) {
    if state.is_some_and(|state| state.transport.interrupt_status != 0) {
        pending.push(spi);
    }
}

/// Puts `transport` on the VM's MMIO bus at `base`.
fn attach(
    shared: &SharedVm,
    transport: VirtioMmioTransport,
    base: u64,
) -> Result<Arc<Mutex<VirtioMmioTransport>>, BootError> {
    let transport = Arc::new(Mutex::new(transport));
    shared.attach_transport(&transport, base, MMIO_LEN)?;
    Ok(transport)
}

/// A block device on the VM's MMIO bus at `base`, its interrupt relayed to SPI `spi`: the writable
/// workspace (`id` is `None`) or a read-only data volume named `id`. Rebuilt from `state` for a
/// restore.
fn attach_block(
    shared: &SharedVm,
    image: &std::path::Path,
    id: Option<&str>,
    base: u64,
    spi: u32,
    state: Option<&MmioDeviceState>,
    irq_sources: &mut Vec<IrqSource>,
) -> Result<Arc<Mutex<VirtioMmioTransport>>, BootError> {
    use crate::devices::virtio::block::device::VirtioBlock;
    use crate::devices::virtio::block::mmio_device::BlockMmioDevice;
    let irq = new_irq(irq_sources, spi)?;
    let device = match id {
        Some(name) => VirtioBlock::new_read_only(image, name),
        None => VirtioBlock::new(image),
    }
    .map_err(VirtioError::from)?;
    let device = Box::new(BlockMmioDevice::new(device));
    let transport = match state {
        Some(state) => VirtioMmioTransport::restore(device, shared.memory().clone(), irq, state)?,
        None => VirtioMmioTransport::new(device, shared.memory().clone(), irq),
    };
    attach(shared, transport, base)
}

/// Polls every device interrupt event and raises its SPI, until the VM stops.
fn relay_interrupts(
    vm: &Weak<SharedVm>,
    exit_evt: &EventFd,
    exit_signalled: &AtomicBool,
    sources: Vec<IrqSource>,
) {
    const EXIT: u64 = u64::MAX;
    let stop_vm = || {
        if let Some(vm) = vm.upgrade() {
            vm.stop();
        }
    };
    let epoll = match Epoll::new() {
        Ok(epoll) => epoll,
        Err(err) => {
            eprintln!("hvf: cannot create the interrupt relay's poll set: {err}; stopping the VM");
            stop_vm();
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
        stop_vm();
        return;
    }
    let mut events = vec![EpollEvent::default(); sources.len() + 1];
    while !exit_signalled.load(Ordering::SeqCst) {
        let ready = match epoll.wait(200, &mut events) {
            Ok(ready) => ready,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("hvf: the interrupt relay failed: {err}; stopping the VM");
                stop_vm();
                return;
            }
        };
        // A VM released without its exit signal (its last reference dropped):
        // nothing is left to interrupt.
        if ready == 0 && vm.strong_count() == 0 {
            return;
        }
        for event in &events[..ready] {
            let data = event.data();
            if data == EXIT {
                continue;
            }
            if let Some((evt, spi)) = sources.get(data as usize)
                && evt.read().is_ok()
            {
                match vm.upgrade() {
                    Some(vm) => vm.raise_spi(*spi),
                    None => return,
                }
            }
        }
    }
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
        // No ITS: the interrupt sources are the console's and the virtio
        // transports' SPIs, and wire interrupts do not need a message-based
        // controller.
        None
    }
}
