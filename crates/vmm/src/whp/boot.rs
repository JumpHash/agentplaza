// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the WHP counterpart of `builder::x86_64::boot`. The
// guest machine is the one the KVM x86_64 builder describes - the same memory
// layout, MPTable, ACPI tables, legacy ports and virtio-mmio transport - so a
// guest boots identically on both.

//! Building and booting one x86_64 microVM on the Windows Hypervisor Platform.
//!
//! The order mirrors the KVM builder: guest memory, the partition and its
//! mappings, the kernel, initrd and command line, the MPTable, ACPI tables and
//! zero page, the devices, then the vCPUs. What differs is who delivers
//! interrupts: the devices still signal `EventFd`s exactly as they do under
//! KVM, but instead of KVM irqfds an *interrupt relay* thread polls them and
//! raises the matching pin of the userspace I/O APIC, which asks WHP's local
//! APIC emulation for the interrupt.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use vm_memory::{Address, Bytes, GuestAddress, GuestMemoryBackend, GuestMemoryRegion};
use windows_sys::Win32::System::Hypervisor::*;

use super::boot_state::{BootRegisters, Segment, TableRegister, long_mode_boot};
use super::kernel::{self, GuestWriter, KernelError};
use super::sys::{WhpError, capability};
use super::vcpu::{ExitSignals, IOAPIC_WINDOW, Interrupts, IoApicMmio, MAX_VCPUS, WhpVm, run_vcpu};
use super::vm::Partition;
use super::zero_page::{ZeroPage, ZeroPageError, standard_e820};
use super::{MmioTransport, guest_cmdline};
use crate::arch::x86_64::layout::{
    CMDLINE_MAX_SIZE, CMDLINE_START, HIMEM_START, MMIO_LEN, PCI_MMCONFIG_SIZE, PCI_MMCONFIG_START,
    RSDP_ADDR, SYSTEM_MEM_SIZE, SYSTEM_MEM_START, VSOCK_GSI, VSOCK_MMIO_START, ZERO_PAGE_START,
};
use crate::arch::x86_64::{arch_memory_regions, mptable};
use crate::devices::legacy::{
    I8042Device, PM1_CNT_SIZE, Pm1CntDevice, SLEEP_CONTROL_PORT, SerialDevice, SerialOut,
};
use crate::devices::virtio::VirtioError;
use crate::devices::virtio::mmio::VirtioMmioTransport;
use crate::devices::virtio::vsock::Vsock;
use crate::devices::virtio::vsock::mmio_device::VsockMmioDevice;
use crate::devices::virtio::vsock::unix::VsockUnixBackend;
use crate::devices::virtio::vsock::{VsockError, event_thread};
use crate::pal::AsRawFd;
use crate::pal::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};
use crate::pal::eventfd::{EFD_NONBLOCK, EventFd};
use crate::spawner::{SpawnError, VmRuntime, VmSpec};
use crate::vstate::memory::{
    GuestMemoryMmap, GuestRegionMmapExt, MemoryError, anonymous, mib_to_bytes,
};
use crate::vstate::resources::ResourceAllocator;

/// Serial port 1 (`COM1`): base and window, and its ISA IRQ.
const SERIAL_PORT_ADDRESS: u64 = 0x3f8;
const SERIAL_PORT_SIZE: u64 = 0x8;
const COM1_IRQ: usize = 4;
/// The i8042 data window and its IRQ.
const I8042_DATA_PORT: u64 = 0x060;
const I8042_DATA_PORT_SIZE: u64 = 0x5;
const KBD_IRQ: usize = 1;

/// `WHV_INTERNAL_ACTIVITY_REGISTER.StartupSuspend`: an application processor
/// waits for INIT/SIPI, as on hardware.
const STARTUP_SUSPEND: u64 = 1;

/// Errors from [`boot`].
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// No vCPUs, or more than this backend runs.
    #[error("{0} vCPUs were requested; this backend runs 1 to {MAX_VCPUS}")]
    VcpuCount(u8),
    /// The VM asked for a workspace volume.
    #[error(
        "this VM asked for a workspace volume, which the Windows backend cannot provide yet \
         (the virtio-block device is Linux-only)"
    )]
    UnsupportedWorkspace,
    /// A kernel or initrd file could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// Guest memory could not be allocated.
    #[error("failed to allocate guest memory: {0}")]
    Memory(#[from] MemoryError),
    /// Guest memory could not be assembled.
    #[error("failed to assemble guest memory: {0}")]
    GuestMemory(#[from] vm_memory::region::GuestRegionCollectionError),
    /// A guest memory write failed.
    #[error("failed to write guest memory: {0}")]
    GuestWrite(#[from] vm_memory::GuestMemoryError),
    /// The WHP platform failed.
    #[error("{0}")]
    Whp(#[from] WhpError),
    /// The kernel image could not be loaded.
    #[error("failed to load the kernel: {0}")]
    Kernel(#[from] KernelError),
    /// The initrd does not fit below the 32-bit MMIO hole.
    #[error("the initrd ({0} bytes) does not fit in guest memory")]
    InitrdTooLarge(usize),
    /// The command line is too long.
    #[error("the kernel command line is {0} bytes; at most {CMDLINE_MAX_SIZE} fit")]
    CommandLine(usize),
    /// The zero page could not be built.
    #[error("failed to build the zero page: {0}")]
    ZeroPage(#[from] ZeroPageError),
    /// The MPTable could not be written.
    #[error("failed to write the MPTable: {0:?}")]
    MpTable(mptable::MptableError),
    /// The ACPI tables could not be written.
    #[error("failed to write the ACPI tables: {0}")]
    Acpi(#[from] crate::acpi::AcpiError),
    /// A device could not be created.
    #[error("failed to create a device: {0}")]
    Device(io::Error),
    /// A device could not be put on a bus.
    #[error("failed to register a device on a bus: {0}")]
    Bus(#[from] crate::vstate::bus::BusError),
    /// The virtio device could not be created.
    #[error("failed to create the vsock device: {0}")]
    Virtio(#[from] VirtioError),
    /// A VM thread could not be started.
    #[error("failed to start a VM thread: {0}")]
    Thread(io::Error),
}

/// Boots the VM described by `spec` and returns once its vCPUs are running.
pub fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    boot_inner(spec, &runtime).map_err(SpawnError::backend)
}

/// Guest RAM as the kernel loader writes it.
struct Ram<'a>(&'a GuestMemoryMmap);

impl GuestWriter for Ram<'_> {
    fn write_at(&mut self, gpa: u64, data: &[u8]) -> bool {
        self.0.write_slice(data, GuestAddress(gpa)).is_ok()
    }
}

fn read(path: &PathBuf) -> Result<Vec<u8>, BootError> {
    std::fs::read(path).map_err(|source| BootError::Read {
        path: path.clone(),
        source,
    })
}

fn boot_inner(spec: &VmSpec, runtime: &Arc<VmRuntime>) -> Result<(), BootError> {
    // A workspace volume is a writable virtio-block device; booting without it
    // would leave the guest waiting for a disk that never appears.
    if spec.workspace.is_some() {
        return Err(BootError::UnsupportedWorkspace);
    }
    if spec.balloon.is_some() {
        eprintln!(
            "whp: VM {}: the balloon is Linux-only; booting with all {} MiB usable",
            runtime.id, spec.memory_mib
        );
    }
    if spec.vcpus == 0 || spec.vcpus > MAX_VCPUS {
        return Err(BootError::VcpuCount(spec.vcpus));
    }
    let vcpu_count = u32::from(spec.vcpus);

    // Guest memory, laid out like the KVM backend's (RAM below the 32-bit MMIO
    // hole, the rest above 4 GiB), and the partition with it mapped in. A
    // device access is a load or store to an unmapped address: that is what
    // makes it exit to this process.
    let regions = arch_memory_regions(mib_to_bytes(spec.memory_mib));
    let memory = GuestMemoryMmap::from_regions(
        anonymous(&regions)?
            .into_iter()
            .enumerate()
            .map(|(slot, region)| {
                GuestRegionMmapExt::dram_from_mmap_region(region, u32::try_from(slot).unwrap_or(0))
            })
            .collect(),
    )?;
    let partition = Arc::new(Partition::create(vcpu_count)?);
    for region in memory.iter() {
        // SAFETY: the region is committed memory owned by `memory`, which the
        // VM state keeps alive until after the partition (and with it the
        // mapping) is gone: `WhpVm` owns both, and drops the partition's last
        // reference only when the vCPU threads have finished.
        #[allow(unsafe_code)]
        unsafe {
            partition.map_memory(
                region.as_ptr(),
                region.start_addr().raw_value(),
                region.len(),
            )?;
        }
    }

    // The kernel, the optional initrd at the top of low memory, and the
    // command line.
    let image = read(&spec.kernel)?;
    let loaded = kernel::load_kernel(&image, &mut Ram(&memory), HIMEM_START)?;
    drop(image);
    let initrd = match &spec.initrd {
        Some(path) => {
            let bytes = read(path)?;
            let lowmem = memory
                .iter()
                .next()
                .map(|region| region.len())
                .unwrap_or_default();
            let address = kernel::initrd_address(lowmem, bytes.len() as u64, 4096)
                .filter(|address| *address >= loaded.end)
                .ok_or(BootError::InitrdTooLarge(bytes.len()))?;
            memory.write_slice(&bytes, GuestAddress(address))?;
            Some((address, bytes.len()))
        }
        None => None,
    };
    let api = partition.api();
    let tsc_khz = capability::<u64>(api, WHvCapabilityCodeProcessorClockFrequency)
        .ok()
        .map(|hz| hz / 1000);
    let transports: Vec<MmioTransport> = spec
        .vsock
        .iter()
        .map(|_| MmioTransport {
            base: VSOCK_MMIO_START,
            len: MMIO_LEN,
            irq: VSOCK_GSI,
        })
        .collect();
    let cmdline = guest_cmdline(&spec.cmdline, &transports, tsc_khz);
    if cmdline.len() + 1 > CMDLINE_MAX_SIZE {
        return Err(BootError::CommandLine(cmdline.len()));
    }
    let mut cmdline_bytes = cmdline.into_bytes();
    cmdline_bytes.push(0);
    memory.write_slice(&cmdline_bytes, GuestAddress(CMDLINE_START))?;

    // The MPTable (the I/O APIC and the ISA routing), the hardware-reduced ACPI
    // tables, and the zero page - the KVM builder's, from the same layout.
    let mut resources = ResourceAllocator::new();
    mptable::setup_mptable(&memory, &mut resources, spec.vcpus).map_err(BootError::MpTable)?;
    crate::acpi::create_acpi_tables(&memory, &mut resources, spec.vcpus)?;
    let mut zero_page = ZeroPage::new(loaded.setup_header.as_deref())?;
    zero_page.set_command_line(CMDLINE_START as u32, cmdline_bytes.len() as u32);
    if let Some((address, size)) = initrd {
        zero_page.set_initrd(address as u32, size as u32);
    }
    zero_page.set_acpi_rsdp(RSDP_ADDR);
    let ram: Vec<(u64, u64)> = memory
        .iter()
        .map(|region| (region.start_addr().raw_value(), region.len()))
        .collect();
    for entry in standard_e820(
        &ram,
        HIMEM_START,
        SYSTEM_MEM_START,
        SYSTEM_MEM_SIZE,
        PCI_MMCONFIG_START,
        PCI_MMCONFIG_SIZE,
    ) {
        zero_page.add_e820(entry)?;
    }
    memory.write_slice(zero_page.as_bytes(), GuestAddress(ZERO_PAGE_START))?;

    // The boot processor enters the kernel in long mode; the others wait for
    // the INIT/SIPI the kernel sends them.
    let (registers, writes) = long_mode_boot(loaded.entry);
    for write in &writes {
        memory.write_slice(&write.bytes, GuestAddress(write.gpa))?;
    }
    for index in 0..vcpu_count {
        partition.create_vcpu(index)?;
    }
    set_boot_registers(&partition, &registers)?;
    for index in 1..vcpu_count {
        let value = WHV_REGISTER_VALUE {
            Reg64: STARTUP_SUSPEND,
        };
        if let Err(err) =
            partition.set_registers(index, &[WHvRegisterInternalActivityState], &[value])
        {
            eprintln!("whp: vCPU {index} could not be parked until INIT/SIPI: {err}");
        }
    }

    // The interrupt path and the shared VM state. The MPTable names the I/O
    // APIC `vcpus + 1`.
    let interrupts = Arc::new(Interrupts::new(Arc::clone(&partition), spec.vcpus + 1));
    let exit = ExitSignals::new().map_err(BootError::Device)?;
    let vm = Arc::new(WhpVm::new(
        Arc::clone(&partition),
        memory,
        Arc::clone(&interrupts),
        vcpu_count,
        exit,
    ));
    drop(partition);

    // The devices: COM1, the i8042 (its reset line), the PM1 control and reset
    // ports, the I/O APIC, and the vsock transport. The buses hold weak
    // references; the strong ones go to the exit watcher, the VM's last thread.
    let serial = Arc::new(Mutex::new(
        SerialDevice::new(SerialOut::shared(Arc::clone(&runtime.serial)))
            .map_err(BootError::Device)?,
    ));
    let i8042 = Arc::new(Mutex::new(
        I8042Device::new(
            vm.exit.evt.try_clone().map_err(BootError::Device)?,
            Arc::clone(&vm.exit.signalled),
        )
        .map_err(BootError::Device)?,
    ));
    let pm1 = Arc::new(Mutex::new(Pm1CntDevice::new(
        vm.exit.evt.try_clone().map_err(BootError::Device)?,
        Arc::clone(&vm.exit.guest_reset),
        Arc::clone(&vm.exit.signalled),
        Arc::clone(&vm.exit.finish_requested),
    )));
    let ioapic = Arc::new(Mutex::new(IoApicMmio(Arc::clone(&interrupts))));
    vm.pio
        .insert(serial.clone(), SERIAL_PORT_ADDRESS, SERIAL_PORT_SIZE)?;
    vm.pio
        .insert(i8042.clone(), I8042_DATA_PORT, I8042_DATA_PORT_SIZE)?;
    vm.pio
        .insert(pm1.clone(), SLEEP_CONTROL_PORT, PM1_CNT_SIZE)?;
    vm.mmio
        .insert(ioapic.clone(), IOAPIC_WINDOW.0, IOAPIC_WINDOW.1)?;

    // Interrupt sources the relay polls: (event, I/O APIC pin).
    let mut irq_sources: Vec<(EventFd, usize)> = Vec::new();
    {
        let serial = serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        irq_sources.push((
            EventFd::try_clone(serial.serial.interrupt_evt()).map_err(BootError::Device)?,
            COM1_IRQ,
        ));
    }
    {
        let i8042 = i8042
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        irq_sources.push((
            i8042
                .kbd_interrupt_evt
                .try_clone()
                .map_err(BootError::Device)?,
            KBD_IRQ,
        ));
    }

    // The vsock device: the userspace device and its muxer, which binds
    // `uds_path` (a taken path fails the boot here), its ring-processing event
    // thread, and the virtio-mmio transport the command line announces.
    let vsock = match &spec.vsock {
        Some(config) => {
            let irq = EventFd::new(EFD_NONBLOCK).map_err(BootError::Device)?;
            irq_sources.push((
                irq.try_clone().map_err(BootError::Device)?,
                VSOCK_GSI as usize,
            ));
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
                vm.exit.evt.try_clone().map_err(BootError::Device)?,
                Arc::clone(&vm.exit.signalled),
                // No snapshots on this backend yet: the gate is never taken for writing.
                event_thread::DeviceGate::default(),
                runtime.id,
            )
            .map_err(BootError::Thread)?;
            runtime.add_thread(handle);
            let transport = Arc::new(Mutex::new(VirtioMmioTransport::new(
                Box::new(VsockMmioDevice::new(device)),
                vm.memory.clone(),
                irq,
            )));
            vm.mmio
                .insert(transport.clone(), VSOCK_MMIO_START, MMIO_LEN)?;
            Some(transport)
        }
        None => None,
    };

    // Console input, held weakly: input after the VM stopped fails.
    let input = Arc::downgrade(&serial);
    runtime.set_input_hook(move |data| match input.upgrade() {
        Some(serial) => serial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .queue_input(data),
        None => Err("the VM has stopped".to_string()),
    });

    // The stop control: a host kill. Weak, so a stopped VM is released.
    let stop = Arc::downgrade(&vm);
    runtime.set_stop_control(move || {
        if let Some(vm) = stop.upgrade() {
            vm.exit.killed.store(true, Ordering::SeqCst);
            vm.exit.signal();
            vm.stop();
        }
    });

    // The interrupt relay.
    let relay_vm = Arc::clone(&vm);
    let relay = thread::Builder::new()
        .name(format!("apz_vm{}_irq", runtime.id))
        .spawn(move || relay_interrupts(&relay_vm, irq_sources))
        .map_err(BootError::Thread)?;
    runtime.add_thread(relay);

    // The exit watcher: waits for the exit event, records how the VM ended,
    // stops the vCPUs, and owns the devices until then.
    let watcher_vm = Arc::clone(&vm);
    let watcher_runtime = Arc::clone(runtime);
    let devices = (serial, i8042, pm1, ioapic, vsock);
    let watcher = thread::Builder::new()
        .name(format!("apz_vm{}_exit", runtime.id))
        .spawn(move || {
            wait_for_exit(&watcher_vm);
            if !watcher_vm.exit.killed.load(Ordering::SeqCst) {
                if watcher_vm.exit.guest_reset.load(Ordering::SeqCst) {
                    watcher_runtime.mark_guest_reset();
                } else {
                    watcher_runtime.mark_guest_exited();
                }
            }
            watcher_vm.stop();
            drop(devices);
        })
        .map_err(BootError::Thread)?;
    runtime.add_thread(watcher);

    for index in 0..vcpu_count {
        let vm = Arc::clone(&vm);
        let vcpu_runtime = Arc::clone(runtime);
        let handle = thread::Builder::new()
            .name(format!("apz_vm{}_vcpu{index}", runtime.id))
            .spawn(move || run_vcpu(vm, vcpu_runtime, index))
            .map_err(BootError::Thread)?;
        runtime.add_thread(handle);
    }
    Ok(())
}

/// Writes the boot processor's long-mode state.
fn set_boot_registers(partition: &Partition, regs: &BootRegisters) -> Result<(), WhpError> {
    fn segment(s: &Segment) -> WHV_REGISTER_VALUE {
        WHV_REGISTER_VALUE {
            Segment: WHV_X64_SEGMENT_REGISTER {
                Base: s.base,
                Limit: s.limit,
                Selector: s.selector,
                Anonymous: WHV_X64_SEGMENT_REGISTER_0 {
                    Attributes: s.attributes(),
                },
            },
        }
    }
    fn table(t: &TableRegister) -> WHV_REGISTER_VALUE {
        WHV_REGISTER_VALUE {
            Table: WHV_X64_TABLE_REGISTER {
                Pad: [0; 3],
                Limit: t.limit,
                Base: t.base,
            },
        }
    }
    let reg64 = |value: u64| WHV_REGISTER_VALUE { Reg64: value };
    let pairs = [
        (WHvX64RegisterCr0, reg64(regs.cr0)),
        (WHvX64RegisterCr3, reg64(regs.cr3)),
        (WHvX64RegisterCr4, reg64(regs.cr4)),
        (WHvX64RegisterEfer, reg64(regs.efer)),
        (WHvX64RegisterPat, reg64(regs.pat)),
        (WHvX64RegisterCs, segment(&regs.cs)),
        (WHvX64RegisterDs, segment(&regs.ds)),
        (WHvX64RegisterEs, segment(&regs.es)),
        (WHvX64RegisterFs, segment(&regs.fs)),
        (WHvX64RegisterGs, segment(&regs.gs)),
        (WHvX64RegisterSs, segment(&regs.ss)),
        (WHvX64RegisterTr, segment(&regs.tr)),
        (WHvX64RegisterLdtr, segment(&regs.ldtr)),
        (WHvX64RegisterGdtr, table(&regs.gdtr)),
        (WHvX64RegisterIdtr, table(&regs.idtr)),
        (WHvX64RegisterRip, reg64(regs.rip)),
        (WHvX64RegisterRsp, reg64(regs.rsp)),
        (WHvX64RegisterRbp, reg64(regs.rbp)),
        (WHvX64RegisterRsi, reg64(regs.rsi)),
        (WHvX64RegisterRflags, reg64(regs.rflags)),
    ];
    let names: Vec<WHV_REGISTER_NAME> = pairs.iter().map(|(name, _)| *name).collect();
    let values: Vec<WHV_REGISTER_VALUE> = pairs.iter().map(|(_, value)| *value).collect();
    partition.set_registers(0, &names, &values)
}

/// Polls every device interrupt event and raises its I/O APIC pin, until the
/// VM exits.
fn relay_interrupts(vm: &WhpVm, sources: Vec<(EventFd, usize)>) {
    const EXIT: u64 = u64::MAX;
    let epoll = match Epoll::new() {
        Ok(epoll) => epoll,
        Err(err) => {
            eprintln!("whp: cannot create the interrupt relay's poll set: {err}");
            vm.exit.signal();
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
            vm.exit.evt.as_raw_fd(),
            EpollEvent::new(EventSet::IN, EXIT),
        )
        .is_ok();
    if !registered {
        eprintln!("whp: cannot register the device interrupts; stopping the VM");
        vm.exit.signal();
        return;
    }
    let mut events = vec![EpollEvent::default(); sources.len() + 1];
    while !vm.exit.signalled.load(Ordering::SeqCst) {
        let ready = match epoll.wait(200, &mut events) {
            Ok(ready) => ready,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("whp: the interrupt relay failed: {err}; stopping the VM");
                vm.exit.signal();
                return;
            }
        };
        for event in &events[..ready] {
            let data = event.data();
            if data == EXIT {
                continue;
            }
            if let Some((evt, pin)) = sources.get(data as usize)
                && evt.read().is_ok()
            {
                vm.interrupts.pulse(*pin);
            }
        }
    }
}

/// Blocks until the exit event is signalled. The event is never consumed: the
/// vsock event thread observes it too.
fn wait_for_exit(vm: &WhpVm) {
    let epoll = Epoll::new().ok();
    if let Some(epoll) = &epoll {
        let _ = epoll.ctl(
            ControlOperation::Add,
            vm.exit.evt.as_raw_fd(),
            EpollEvent::new(EventSet::IN, 0),
        );
    }
    let mut events = [EpollEvent::default()];
    while !vm.exit.signalled.load(Ordering::SeqCst) {
        match &epoll {
            Some(epoll) => {
                let _ = epoll.wait(500, &mut events);
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    }
}
