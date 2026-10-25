// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//! Boots an arm64 VM on the Windows Hypervisor Platform: the same guest as the Hypervisor.framework and
//! KVM arm64 backends. The kernel and the initrd are loaded into the arch layout's DRAM, the device tree is
//! written above them with the GICv3 node of the layout (`hvf::gic`'s addresses, which the hypervisor's
//! GIC uses too), vCPU 0 starts at the kernel entry with the device tree in X0, and the virtio transports
//! (vsock, the workspace, the data volumes) sit on the MMIO bus with their interrupts relayed to the GIC.
//!
//! Cold boot only: a snapshot restore is refused (the design doc's pieces list it as a later step).

// The partition calls are `unsafe` FFI (`super::vm`); the one here maps the guest memory, with its note.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};
use std::thread;

use vm_memory::{Address, Bytes, GuestAddress, GuestMemoryBackend, GuestMemoryRegion};

use super::abi;
use super::vcpu::{self, IrqLine, Vm, WeakVm};
use super::vm::Partition;
use crate::arch::aarch64::layout::{
    CMDLINE_MAX_SIZE, DATA_GSI_START, DATA_MMIO_START, MMIO_LEN, SERIAL_MEM_START, VSOCK_GSI,
    VSOCK_MMIO_START, WORKSPACE_GSI, WORKSPACE_MMIO_START,
};
use crate::arch::aarch64::{
    FdtError, GicFdt, InitrdConfig, InitrdError, SerialFdtInfo, arch_memory_regions, create_fdt,
    get_fdt_addr, get_kernel_start,
};
use crate::devices::virtio::VirtioError;
use crate::devices::virtio::mmio::VirtioMmioTransport;
use crate::devices::virtio::vsock::event_thread::{self, DeviceGate};
use crate::devices::virtio::vsock::mmio_device::VsockMmioDevice;
use crate::devices::virtio::vsock::unix::VsockUnixBackend;
use crate::devices::virtio::vsock::{Vsock, VsockError};
use crate::hvf::gic::{self, GICV3_DIST_BASE, GICV3_DIST_SIZE, GICV3_REDIST_BASE};
use crate::hvf::relay::IrqSource;
use crate::pal::eventfd::{EFD_NONBLOCK, EventFd};
use crate::spawner::{MAX_DATA_VOLUMES, SpawnError, VmRuntime, VmSpec};
use crate::vstate::bus::{Bus, BusDevice};
use crate::vstate::memory::{
    GuestMemoryMmap, GuestRegionMmapExt, MemoryError, anonymous, mib_to_bytes,
};

/// The vCPU count this backend runs: the GIC's redistributor window and the layout's device tree
/// are sized for it.
pub const MAX_VCPUS: u8 = 8;

/// Why an arm64 boot failed.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("the VM needs at least one vCPU")]
    NoVcpus,
    #[error("{requested} vCPUs were requested, but this backend runs at most {MAX_VCPUS}")]
    TooManyVcpus { requested: u8 },
    #[error("a snapshot restore is not written for the Windows arm64 backend yet")]
    Restore,
    #[error("the Windows arm64 backend takes at most {MAX_DATA_VOLUMES} data volumes")]
    TooManyVolumes,
    #[error("cannot open the kernel image {path}: {source}")]
    Kernel { path: PathBuf, source: io::Error },
    #[error("cannot open the initrd image {path}: {source}")]
    Initrd { path: PathBuf, source: io::Error },
    #[error("failed to allocate guest memory: {0}")]
    Memory(#[from] MemoryError),
    #[error("failed to assemble guest memory: {0}")]
    GuestMemory(#[from] vm_memory::region::GuestRegionCollectionError),
    #[error("failed to load the initrd: {0}")]
    InitrdLoad(#[from] InitrdError),
    #[error("the kernel is not an arm64 Image: {0}")]
    KernelImage(String),
    #[error("the kernel command line is not usable: {0}")]
    CommandLine(String),
    #[error("failed to create the device tree: {0}")]
    Fdt(#[from] FdtError),
    #[error("failed to write guest memory: {0}")]
    Write(#[from] vm_memory::GuestMemoryError),
    #[error("the hypervisor: {0}")]
    Whp(#[from] crate::whp::sys::WhpError),
    #[error("failed to create a device: {0}")]
    Device(io::Error),
    #[error("failed to create a VM thread: {0}")]
    Thread(io::Error),
    #[error("failed to register a device on the MMIO bus: {0}")]
    Bus(#[from] crate::vstate::bus::BusError),
    #[error("failed to create a virtio device: {0}")]
    Virtio(#[from] VirtioError),
    #[error("failed to start the vsock device: {0}")]
    Vsock(#[from] VsockError),
}

/// Boots the VM described by `spec` and returns once its vCPU threads run.
pub fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    crate::spawner::check_net(spec)?;
    boot_inner(spec, &runtime).map_err(SpawnError::backend)
}

fn boot_inner(spec: &VmSpec, runtime: &Arc<VmRuntime>) -> Result<(), BootError> {
    if spec.restore.is_some() {
        return Err(BootError::Restore);
    }
    if spec.vcpus == 0 {
        return Err(BootError::NoVcpus);
    }
    if spec.vcpus > MAX_VCPUS {
        return Err(BootError::TooManyVcpus {
            requested: spec.vcpus,
        });
    }
    if spec.volumes.len() > MAX_DATA_VOLUMES {
        return Err(BootError::TooManyVolumes);
    }
    let vcpus = u32::from(spec.vcpus);
    let memory = guest_memory(spec.memory_mib)?;

    // The partition: the GIC in the hypervisor, the memory mapped at the layout's addresses, one
    // processor per vCPU with its MPIDR and redistributor frame.
    let partition = Partition::create(vcpus, GICV3_DIST_BASE)?;
    for region in memory.iter() {
        // SAFETY: the regions belong to `memory`, which the `Vm` keeps alive until the partition is dropped
        // (the `Vm` drops its partition first).
        unsafe {
            partition.map_memory(
                region.as_ptr(),
                region.start_addr().raw_value(),
                region.len(),
            )?;
        }
    }
    for index in 0..vcpus {
        partition.create_vcpu(index)?;
        partition.set_registers(index, &abi::vcpu_registers(index))?;
    }

    // Kernel, initrd, command line, then the device tree above them.
    let entry = load_image(&spec.kernel, &memory)?;
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
    let cmdline = command_line(&crate::hvf::guest_cmdline(&spec.cmdline))?;
    let transports = crate::arch::aarch64::virtio_transports(
        spec.vsock.is_some(),
        spec.workspace.is_some(),
        spec.volumes.len(),
        spec.net.is_some(),
    );
    let fdt = create_fdt(
        &memory,
        (0..u64::from(vcpus)).collect(),
        cmdline,
        &SerialFdtInfo {
            addr: SERIAL_MEM_START,
            len: MMIO_LEN,
            gsi: gic::UART_SPI,
        },
        &transports,
        &GicFdtNode::new(vcpus),
        &initrd,
    )?;
    let fdt_addr = get_fdt_addr(&memory);
    memory.write_slice(&fdt, GuestAddress(fdt_addr))?;
    partition.set_registers(0, &abi::boot_registers(entry, fdt_addr))?;

    // The VM, its bus and its devices. A device thread or the relay that outlives a failed boot is
    // stopped by the `stop` below, which the error path runs by dropping the `Arc`.
    let vm = Arc::new(Vm {
        partition,
        memory,
        bus: Bus::new(),
        vcpus,
        lines: Mutex::new(Vec::new()),
        stopping: Arc::new(AtomicBool::new(false)),
        exit_evt: EventFd::new(EFD_NONBLOCK).map_err(BootError::Device)?,
        runtime: Arc::clone(runtime),
    });
    let irq_sources = attach_devices(spec, runtime, &vm)?;

    // The interrupt relay, when a device has an interrupt to relay. It holds the VM weakly.
    if !irq_sources.is_empty() {
        let exit_evt = vm.exit_evt.try_clone().map_err(BootError::Device)?;
        let stopping = Arc::clone(&vm.stopping);
        let weak = Arc::downgrade(&vm);
        let relay = thread::Builder::new()
            .name(format!("apz_vm{}_irq", runtime.id))
            .spawn(move || crate::hvf::relay::run(&WeakVm(weak), &exit_evt, &stopping, irq_sources))
            .map_err(BootError::Thread)?;
        runtime.add_thread(relay);
    }

    // The stop control holds the VM weakly: the runtime must not keep a stopped VM alive.
    let weak: Weak<Vm> = Arc::downgrade(&vm);
    runtime.set_stop_control(move || {
        if let Some(vm) = weak.upgrade() {
            vm.stop();
        }
    });

    // One thread per vCPU. The hypervisor starts the secondaries itself, on the guest's PSCI CPU_ON.
    for index in 0..vcpus {
        let vm = Arc::clone(&vm);
        let handle = thread::Builder::new()
            .name(format!("apz_vm{}_vcpu{index}", runtime.id))
            .spawn(move || vcpu::run_vcpu(&vm, index))
            .map_err(BootError::Thread)?;
        runtime.add_thread(handle);
    }
    Ok(())
}

/// Loads an arm64 `Image` the way the Linux arm64 boot protocol and linux-loader's PE loader do: the
/// header's magic (`ARM\x64` at 0x38) is checked, the image goes at the layout's kernel start plus its
/// `text_offset` (0x80000 when `image_size` is zero), and the entry is that address.
fn load_image(path: &std::path::Path, memory: &GuestMemoryMmap) -> Result<u64, BootError> {
    const MAGIC: u32 = 0x644d_5241;
    let image = std::fs::read(path).map_err(|source| BootError::Kernel {
        path: path.to_path_buf(),
        source,
    })?;
    if image.len() < 0x40 {
        return Err(BootError::KernelImage(
            "the file is shorter than the header".to_string(),
        ));
    }
    let magic = u32::from_le_bytes(image[0x38..0x3c].try_into().expect("four bytes"));
    if magic != MAGIC {
        return Err(BootError::KernelImage(format!(
            "magic {magic:#010x}, not ARM\\x64"
        )));
    }
    let image_size = u64::from_le_bytes(image[0x10..0x18].try_into().expect("eight bytes"));
    let text_offset = if image_size == 0 {
        0x8_0000
    } else {
        u64::from_le_bytes(image[0x08..0x10].try_into().expect("eight bytes"))
    };
    let load = get_kernel_start() + text_offset;
    memory.write_slice(&image, GuestAddress(load))?;
    Ok(load)
}

/// The kernel command line as the device tree takes it: no longer than the layout allows, no NUL byte.
fn command_line(text: &str) -> Result<CString, BootError> {
    if text.len() >= CMDLINE_MAX_SIZE {
        return Err(BootError::CommandLine(format!(
            "{} bytes, the layout allows {}",
            text.len(),
            CMDLINE_MAX_SIZE - 1
        )));
    }
    CString::new(text).map_err(|err| BootError::CommandLine(err.to_string()))
}

/// The transports' devices on the bus: the workspace, the data volumes, then the vsock device (its
/// muxer binds `uds_path`, and its ring thread starts), each with its interrupt line.
fn attach_devices(
    spec: &VmSpec,
    runtime: &Arc<VmRuntime>,
    vm: &Arc<Vm>,
) -> Result<Vec<IrqSource>, BootError> {
    let mut sources: Vec<IrqSource> = Vec::new();
    let gate: DeviceGate = Arc::new(std::sync::RwLock::new(()));

    if let Some(image) = &spec.workspace {
        attach_block(
            vm,
            image,
            None,
            WORKSPACE_MMIO_START,
            WORKSPACE_GSI,
            &mut sources,
        )?;
    }
    for (index, volume) in spec.volumes.iter().enumerate() {
        attach_block(
            vm,
            &volume.image,
            Some(&volume.name),
            DATA_MMIO_START + index as u64 * MMIO_LEN,
            DATA_GSI_START + index as u32,
            &mut sources,
        )?;
    }
    if let Some(config) = &spec.vsock {
        let irq = new_irq(&mut sources, VSOCK_GSI)?;
        let backend = VsockUnixBackend::new(
            config.guest_cid,
            config.uds_path.to_string_lossy().into_owned(),
        )
        .map_err(VsockError::from)
        .map_err(VirtioError::from)?;
        let device = Arc::new(Mutex::new(
            Vsock::new(config.guest_cid, backend).map_err(VirtioError::from)?,
        ));
        let handle = event_thread::start_event_thread(
            Arc::clone(&device),
            vm.exit_evt.try_clone().map_err(BootError::Device)?,
            Arc::clone(&vm.stopping),
            Arc::clone(&gate),
            runtime.id,
        )
        .map_err(BootError::Thread)?;
        runtime.add_thread(handle);
        let transport = VirtioMmioTransport::new(
            Box::new(VsockMmioDevice::new(device)),
            vm.memory.clone(),
            irq,
        );
        attach_transport(vm, transport, VSOCK_MMIO_START, VSOCK_GSI)?;
    }
    // The lab topology's network device (PLAN 22.6): a transport in the layout's net slot on its own SPI,
    // its interface on the segment's switch. A session VM has none (`check_net`).
    if let Some(attachment) = &spec.net {
        use crate::arch::aarch64::layout::{NET_GSI, NET_MMIO_START};
        let irq = new_irq(&mut sources, NET_GSI)?;
        let device = crate::devices::virtio::net::attach(
            &attachment.switch,
            &attachment.segment,
            attachment.mac,
        )
        .map_err(VirtioError::from)?;
        let transport = VirtioMmioTransport::new(Box::new(device), vm.memory.clone(), irq);
        attach_transport(vm, transport, NET_MMIO_START, NET_GSI)?;
    }
    Ok(sources)
}

/// A block device on the bus at `base`, its interrupt relayed to SPI `spi`.
fn attach_block(
    vm: &Arc<Vm>,
    image: &std::path::Path,
    id: Option<&str>,
    base: u64,
    spi: u32,
    sources: &mut Vec<IrqSource>,
) -> Result<(), BootError> {
    use crate::devices::virtio::block::device::VirtioBlock;
    use crate::devices::virtio::block::mmio_device::BlockMmioDevice;

    let irq = new_irq(sources, spi)?;
    let device = match id {
        Some(name) => VirtioBlock::new_read_only(image, name),
        None => VirtioBlock::new(image),
    }
    .map_err(VirtioError::from)?;
    let transport = VirtioMmioTransport::new(
        Box::new(BlockMmioDevice::new(device)),
        vm.memory.clone(),
        irq,
    );
    attach_transport(vm, transport, base, spi)
}

/// A new interrupt event for SPI `spi`, registered with the relay.
fn new_irq(sources: &mut Vec<IrqSource>, spi: u32) -> Result<EventFd, BootError> {
    let irq = EventFd::new(EFD_NONBLOCK).map_err(BootError::Device)?;
    sources.push((irq.try_clone().map_err(BootError::Device)?, spi));
    Ok(irq)
}

/// Puts a transport on the bus at `base` (one window), and records its line for the acknowledge path.
fn attach_transport(
    vm: &Arc<Vm>,
    transport: VirtioMmioTransport,
    base: u64,
    spi: u32,
) -> Result<(), BootError> {
    let transport = Arc::new(Mutex::new(transport));
    let device: Arc<Mutex<dyn BusDevice>> = transport;
    vcpu::attach(&vm.bus, device, base, MMIO_LEN)?;
    vm.add_line(IrqLine {
        base,
        len: MMIO_LEN,
        spi,
    });
    Ok(())
}

/// The guest's DRAM: the layout's regions for `memory_mib`, the process's own pages (what WHP maps).
fn guest_memory(memory_mib: usize) -> Result<GuestMemoryMmap, BootError> {
    let regions = arch_memory_regions(mib_to_bytes(memory_mib));
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

/// The GICv3 node of the device tree: the same distributor and redistributor windows the Hypervisor.framework
/// backend describes, which the hypervisor's GIC uses at the same addresses.
struct GicFdtNode {
    properties: [u64; 4],
}

impl GicFdtNode {
    fn new(vcpus: u32) -> Self {
        let (_, redist_size) = gic::redist_window(vcpus as usize);
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
        None
    }
}
