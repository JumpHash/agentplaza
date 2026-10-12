// Copyright © 2020, Oracle and/or its affiliates.
//
// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/x86_64/mod.rs and
// src/vmm/src/arch/mod.rs (trimmed: `EntryPoint`, `BootProtocol` and
// `GUEST_PAGE_SIZE` move here from `arch/mod.rs`, which in this tree only
// declares the per-arch modules; the `InitrdConfig` loader moves here from
// src/initrd.rs; CPU templates, the ACPI table generation and the
// `DeviceManager`/`MachineConfig` parameters are dropped — CPUID is installed
// directly from KVM's supported set; `/dev/kvm`-dependent tests are dropped).

/// Logic for handling x86_64 CPU models. (Upstream has `cpu_model` here; it
/// belongs to the CPU-template machinery, which is trimmed.)
mod gdt;
/// Contains logic for setting up Advanced Programmable Interrupt Controller (local version).
pub mod interrupts;
/// Architecture specific KVM-related code
pub mod kvm;
/// Layout for the x86_64 system.
pub mod layout;
/// Guest RAM regions around the MMIO holes (shared with the WHP backend).
mod regions;
pub use regions::arch_memory_regions;
mod mptable;
/// Logic for configuring x86_64 model specific registers (MSRs).
pub mod msr;
/// Logic for configuring x86_64 registers.
pub mod regs;
/// Architecture specific vCPU code
pub mod vcpu;
/// Architecture specific VM state code
pub mod vm;
/// Logic for configuring XSTATE features.
pub mod xstate;

#[allow(missing_docs)]
pub mod generated;

use std::cmp::max;
use std::fs::File;
use std::fmt;
use std::os::unix::fs::MetadataExt;

use crate::vstate::memory::{u64_to_usize, usize_to_u64, Address, GuestAddress, GuestMemoryMmap, GuestMemoryRegion};
use crate::vstate::vcpu::{KvmVcpuConfigureError, Vcpu};
use kvm::Kvm;
use layout::{
    CMDLINE_START,
    PCI_MMCONFIG_SIZE, PCI_MMCONFIG_START,
};
use linux_loader::configurator::linux::LinuxBootConfigurator;
use linux_loader::configurator::pvh::PvhBootConfigurator;
use linux_loader::configurator::{BootConfigurator, BootParams};
use linux_loader::loader::bootparam::{XLF_KERNEL_64, boot_params, setup_header};
use linux_loader::loader::bzimage::BzImage as BzImageLoader;
use linux_loader::loader::elf::Error as ElfError;
use linux_loader::loader::elf::start_info::{
    hvm_memmap_table_entry, hvm_modlist_entry, hvm_start_info,
};
use linux_loader::loader::elf::Elf as ElfLoader;
use linux_loader::loader::{
    Cmdline, Error as KernelLoaderError, KernelLoader, PvhBootCapability, load_cmdline,
};
use vm_memory::GuestMemoryBackend;
use vm_memory::ReadVolatile;

// Value taken from https://elixir.bootlin.com/linux/v5.10.68/source/arch/x86/include/uapi/asm/e820.h#L31
// Usable normal RAM
const E820_RAM: u32 = 1;

// Reserved area that should be avoided during memory allocations
const E820_RESERVED: u32 = 2;
const MEMMAP_TYPE_RAM: u32 = 1;

/// Default page size for the guest OS (upstream: `arch/mod.rs`).
pub const GUEST_PAGE_SIZE: usize = 4096;

/// Errors thrown while configuring x86_64 system.
#[derive(Debug, thiserror::Error)]
pub enum ConfigurationError {
    /// Invalid e820 setup params.
    #[error("Invalid e820 setup params.")]
    E820Configuration,
    /// Error writing MP table to memory: {0}
    #[error("Error writing MP table to memory: {0}")]
    MpTableSetup(#[from] mptable::MptableError),
    /// Error creating ACPI tables: {0}
    #[error("Error creating ACPI tables: {0}")]
    AcpiTables(#[from] crate::acpi::AcpiError),
    /// Error writing the zero page of guest memory.
    #[error("Error writing the zero page of guest memory.")]
    ZeroPageSetup,
    /// Error writing module entry to guest memory.
    #[error("Error writing module entry to guest memory.")]
    ModlistSetup,
    /// Error writing memory map table to guest memory.
    #[error("Error writing memory map table to guest memory.")]
    MemmapTableSetup,
    /// Error writing hvm_start_info to guest memory.
    #[error("Error writing hvm_start_info to guest memory.")]
    StartInfoSetup,
    /// Cannot copy kernel file fd
    #[error("Cannot copy kernel file fd")]
    KernelFile,
    /// Cannot load kernel due to invalid memory configuration or invalid kernel image: {0}
    #[error("Cannot load kernel due to invalid memory configuration or invalid kernel image: {0}")]
    KernelLoader(linux_loader::loader::Error),
    /// bzImage does not provide a 64-bit entry point
    #[error("bzImage does not provide a 64-bit entry point")]
    BzImageMissing64BitEntry,
    /// Cannot load command line string: {0}
    #[error("Cannot load command line string: {0}")]
    LoadCommandline(linux_loader::loader::Error),
    /// Error configuring the vcpu for boot: {0}
    #[error("Error configuring the vcpu for boot: {0}")]
    VcpuConfigure(#[from] KvmVcpuConfigureError),
    /// Cannot load initrd: {0}
    #[error("Cannot load initrd: {0}")]
    Initrd(#[from] InitrdError),
}

/// Errors associated with initrd loading (upstream: `src/initrd.rs`).
#[derive(Debug, thiserror::Error)]
pub enum InitrdError {
    /// Failed to compute the initrd address.
    #[error("Failed to compute the initrd address.")]
    Address,
    /// Cannot load initrd due to an invalid memory configuration.
    #[error("Cannot load initrd due to an invalid memory configuration.")]
    Load,
    /// Cannot image metadata: {0}
    #[error("Cannot image metadata: {0}")]
    Metadata(std::io::Error),
    /// Cannot copy initrd file fd: {0}
    #[error("Cannot copy initrd file fd: {0}")]
    CloneFd(std::io::Error),
    /// Cannot load initrd due to an invalid image: {0}
    #[error("Cannot load initrd due to an invalid image: {0}")]
    Read(vm_memory::VolatileMemoryError),
}

/// Type for passing information about the initrd in the guest memory
/// (upstream: `src/initrd.rs`).
#[derive(Debug)]
pub struct InitrdConfig {
    /// Load address of initrd in guest memory
    pub address: GuestAddress,
    /// Size of initrd in guest memory
    pub size: usize,
}

impl InitrdConfig {
    /// Loads the initrd from a file into guest memory.
    pub fn from_file(vm_memory: &GuestMemoryMmap, mut file: File) -> Result<Self, InitrdError> {
        let size = file.metadata().map_err(InitrdError::Metadata)?.size();
        let size = crate::vstate::memory::u64_to_usize(size);
        let Some(address) = initrd_load_addr(vm_memory, size) else {
            return Err(InitrdError::Address);
        };
        let mut slice = vm_memory
            .get_slice(GuestAddress(address), size)
            .map_err(|_| InitrdError::Load)?;
        file.read_exact_volatile(&mut slice)
            .map_err(InitrdError::Read)?;

        Ok(InitrdConfig {
            address: GuestAddress(address),
            size,
        })
    }
}

/// Supported boot protocols
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum BootProtocol {
    /// Linux 64-bit boot protocol
    LinuxBoot,
    /// PVH boot protocol (x86/HVM direct boot ABI)
    PvhBoot,
}

impl fmt::Display for BootProtocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BootProtocol::LinuxBoot => write!(f, "Linux 64-bit boot protocol"),
            BootProtocol::PvhBoot => write!(f, "PVH boot protocol"),
        }
    }
}

#[derive(Debug, Copy, Clone)]
/// Specifies the entry point address where the guest must start
/// executing code, as well as which boot protocol is to be used
/// to configure the guest initial state.
pub struct EntryPoint {
    /// Address in guest memory where the guest must start execution
    pub entry_addr: GuestAddress,
    /// Specifies which boot protocol to use
    pub protocol: BootProtocol,
    /// Setup header from a bzImage kernel, used to seed the zero page.
    /// `None` for ELF (`vmlinux`) kernels.
    pub setup_header: Option<setup_header>,
}

/// Returns the memory address where the kernel could be loaded.
pub fn get_kernel_start() -> u64 {
    layout::HIMEM_START
}

/// Returns the memory address where the initrd could be loaded.
pub fn initrd_load_addr(guest_mem: &GuestMemoryMmap, initrd_size: usize) -> Option<u64> {
    let first_region = guest_mem.find_region(GuestAddress::new(0))?;
    let lowmem_size = u64_to_usize(first_region.len());

    if lowmem_size < initrd_size {
        return None;
    }

    Some(crate::align_down!(
        usize_to_u64(lowmem_size - initrd_size),
        usize_to_u64(GUEST_PAGE_SIZE)
    ))
}

/// Configures the vCPUs for booting Linux.
///
/// Upstream applies a CPU template (CPUID and MSR modifications) per-vCPU
/// before the boot MSRs; without templates, each vCPU installs KVM's
/// supported CPUID (normalized per-CPU), the Linux boot MSRs and the boot
/// register state.
fn configure_vcpus_for_boot(
    kvm: &Kvm,
    vcpus: &mut [Vcpu],
    vcpu_count: u8,
    smt: bool,
    guest_mem: &GuestMemoryMmap,
    entry_point: EntryPoint,
) -> Result<(), ConfigurationError> {
    for vcpu in vcpus.iter_mut() {
        vcpu.kvm_vcpu.configure_cpuid(kvm, vcpu_count, smt)?;
        vcpu.kvm_vcpu.configure_msrs_for_boot()?;
        vcpu.kvm_vcpu.configure_boot_state(guest_mem, entry_point)?;
    }

    Ok(())
}

/// Configures the system for booting Linux.
#[allow(clippy::too_many_arguments)]
pub fn configure_system_for_boot(
    kvm: &Kvm,
    vm: &crate::vstate::vm::KvmVm,
    vcpus: &mut [Vcpu],
    vcpu_count: u8,
    smt: bool,
    entry_point: EntryPoint,
    initrd: &Option<InitrdConfig>,
    boot_cmdline: Cmdline,
) -> Result<(), ConfigurationError> {
    configure_vcpus_for_boot(
        kvm,
        vcpus,
        vcpu_count,
        smt,
        vm.guest_memory(),
        entry_point,
    )?;

    // Write the kernel command line to guest memory. This is x86_64 specific, since on
    // aarch64 the command line will be specified through the FDT.
    let cmdline_size = boot_cmdline
        .as_cstring()
        .map(|cmdline_cstring| cmdline_cstring.as_bytes_with_nul().len())
        .unwrap_or_default();

    load_cmdline(
        vm.guest_memory(),
        GuestAddress(CMDLINE_START),
        &boot_cmdline,
    )
    .map_err(ConfigurationError::LoadCommandline)?;

    // Note that this puts the mptable at the last 1k of Linux's 640k base RAM
    mptable::setup_mptable(vm.guest_memory(), &mut vm.resource_allocator(), vcpu_count)
        .map_err(ConfigurationError::MpTableSetup)?;

    match entry_point.protocol {
        BootProtocol::PvhBoot => {
            configure_pvh(vm.guest_memory(), GuestAddress(CMDLINE_START), initrd)?;
        }
        BootProtocol::LinuxBoot => {
            configure_64bit_boot(
                vm.guest_memory(),
                GuestAddress(CMDLINE_START),
                cmdline_size,
                initrd,
                entry_point.setup_header,
            )?;
        }
    }

    // Create ACPI tables and write them in guest memory
    // For the time being we only support ACPI in x86_64
    crate::acpi::create_acpi_tables(
        vm.guest_memory(),
        &mut vm.resource_allocator(),
        vcpu_count,
    )
    .map_err(ConfigurationError::AcpiTables)?;
    Ok(())
}

fn configure_pvh(
    guest_mem: &GuestMemoryMmap,
    cmdline_addr: GuestAddress,
    initrd: &Option<InitrdConfig>,
) -> Result<(), ConfigurationError> {
    const XEN_HVM_START_MAGIC_VALUE: u32 = 0x336e_c578;
    let himem_start = GuestAddress(layout::HIMEM_START);

    // Vector to hold modules (currently either empty or holding initrd).
    let mut modules: Vec<hvm_modlist_entry> = Vec::new();
    if let Some(initrd_config) = initrd {
        // The initrd has been written to guest memory already, here we just need to
        // create the module structure that describes it.
        modules.push(hvm_modlist_entry {
            paddr: initrd_config.address.raw_value(),
            size: initrd_config.size as u64,
            ..Default::default()
        });
    }

    // Vector to hold the memory maps which needs to be written to guest memory
    // at MEMMAP_START after all of the mappings are recorded.
    let mut memmap: Vec<hvm_memmap_table_entry> = Vec::new();

    // Create the memory map entries.
    memmap.push(hvm_memmap_table_entry {
        addr: 0,
        size: layout::SYSTEM_MEM_START,
        type_: MEMMAP_TYPE_RAM,
        ..Default::default()
    });
    memmap.push(hvm_memmap_table_entry {
        addr: layout::SYSTEM_MEM_START,
        size: layout::SYSTEM_MEM_SIZE,
        type_: E820_RESERVED,
        ..Default::default()
    });
    memmap.push(hvm_memmap_table_entry {
        addr: PCI_MMCONFIG_START,
        size: PCI_MMCONFIG_SIZE,
        type_: E820_RESERVED,
        ..Default::default()
    });

    for region in guest_mem.iter() {
        // the first 1MB is reserved for the kernel
        let addr = max(himem_start, region.start_addr());
        memmap.push(hvm_memmap_table_entry {
            addr: addr.raw_value(),
            size: region.last_addr().unchecked_offset_from(addr) + 1,
            type_: MEMMAP_TYPE_RAM,
            ..Default::default()
        });
    }

    // Construct the hvm_start_info structure and serialize it into
    // boot_params.  This will be stored at PVH_INFO_START address, and %rbx
    // will be initialized to contain PVH_INFO_START prior to starting the
    // guest, as required by the PVH ABI.
    #[allow(clippy::cast_possible_truncation)] // the vec lengths are single digit integers
    let mut start_info = hvm_start_info {
        magic: XEN_HVM_START_MAGIC_VALUE,
        version: 1,
        cmdline_paddr: cmdline_addr.raw_value(),
        memmap_paddr: layout::MEMMAP_START,
        memmap_entries: memmap.len() as u32,
        nr_modules: modules.len() as u32,
        ..Default::default()
    };
    if !modules.is_empty() {
        start_info.modlist_paddr = layout::MODLIST_START;
    }
    let mut boot_params =
        BootParams::new::<hvm_start_info>(&start_info, GuestAddress(layout::PVH_INFO_START));

    // Copy the vector with the memmap table to the MEMMAP_START address
    // which is already saved in the memmap_paddr field of hvm_start_info struct.
    boot_params.set_sections::<hvm_memmap_table_entry>(&memmap, GuestAddress(layout::MEMMAP_START));

    // Copy the vector with the modules list to the MODLIST_START address.
    // Note that we only set the modlist_paddr address if there is a nonzero
    // number of modules, but serializing an empty list is harmless.
    boot_params.set_modules::<hvm_modlist_entry>(&modules, GuestAddress(layout::MODLIST_START));

    // Write the hvm_start_info struct to guest memory.
    PvhBootConfigurator::write_bootparams(&boot_params, guest_mem)
        .map_err(|_| ConfigurationError::StartInfoSetup)
}

fn configure_64bit_boot(
    guest_mem: &GuestMemoryMmap,
    cmdline_addr: GuestAddress,
    cmdline_size: usize,
    initrd: &Option<InitrdConfig>,
    setup_header: Option<setup_header>,
) -> Result<(), ConfigurationError> {
    const KERNEL_BOOT_FLAG_MAGIC: u16 = 0xaa55;
    const KERNEL_HDR_MAGIC: u32 = 0x5372_6448;
    const KERNEL_LOADER_OTHER: u8 = 0xff;
    const KERNEL_MIN_ALIGNMENT_BYTES: u32 = 0x0100_0000; // Must be non-zero.

    let himem_start = GuestAddress(layout::HIMEM_START);

    // A bzImage carries its own setup header; for ELF there is none, so
    // start from a zeroed header with the minimum required alignment.
    let mut hdr = setup_header.unwrap_or(setup_header {
        kernel_alignment: KERNEL_MIN_ALIGNMENT_BYTES,
        ..Default::default()
    });
    hdr.type_of_loader = KERNEL_LOADER_OTHER;
    hdr.boot_flag = KERNEL_BOOT_FLAG_MAGIC;
    hdr.header = KERNEL_HDR_MAGIC;
    hdr.cmd_line_ptr = u32::try_from(cmdline_addr.raw_value())
        .expect("the command line address always fits in u32");
    hdr.cmdline_size = u32::try_from(cmdline_size).expect("cmdline_size always fits in u32");
    if let Some(initrd_config) = initrd {
        hdr.ramdisk_image = u32::try_from(initrd_config.address.raw_value())
            .expect("the initrd address always fits in u32");
        hdr.ramdisk_size =
            u32::try_from(initrd_config.size).expect("the initrd size always fits in u32");
    }

    // Set the location of RSDP in Boot Parameters to help the guest kernel find it faster.
    let mut params = boot_params {
        hdr,
        acpi_rsdp_addr: layout::RSDP_ADDR,
        ..Default::default()
    };

    // We mark first [0x0, SYSTEM_MEM_START) region as usable RAM and the subsequent
    // [SYSTEM_MEM_START, (SYSTEM_MEM_START + SYSTEM_MEM_SIZE)) as reserved (note
    // SYSTEM_MEM_SIZE + SYSTEM_MEM_SIZE == HIMEM_START).
    add_e820_entry(&mut params, 0, layout::SYSTEM_MEM_START, E820_RAM)?;
    add_e820_entry(
        &mut params,
        layout::SYSTEM_MEM_START,
        layout::SYSTEM_MEM_SIZE,
        E820_RESERVED,
    )?;
    add_e820_entry(
        &mut params,
        PCI_MMCONFIG_START,
        PCI_MMCONFIG_SIZE,
        E820_RESERVED,
    )?;

    for region in guest_mem.iter() {
        // the first 1MB is reserved for the kernel
        let addr = max(himem_start, region.start_addr());
        add_e820_entry(
            &mut params,
            addr.raw_value(),
            region.last_addr().unchecked_offset_from(addr) + 1,
            E820_RAM,
        )?;
    }

    LinuxBootConfigurator::write_bootparams(
        &BootParams::new(&params, GuestAddress(layout::ZERO_PAGE_START)),
        guest_mem,
    )
    .map_err(|_| ConfigurationError::ZeroPageSetup)
}

/// Add an e820 region to the e820 map.
/// Returns Ok(()) if successful, or an error if there is no space left in the map.
fn add_e820_entry(
    params: &mut boot_params,
    addr: u64,
    size: u64,
    mem_type: u32,
) -> Result<(), ConfigurationError> {
    if params.e820_entries as usize >= params.e820_table.len() {
        return Err(ConfigurationError::E820Configuration);
    }

    params.e820_table[params.e820_entries as usize].addr = addr;
    params.e820_table[params.e820_entries as usize].size = size;
    params.e820_table[params.e820_entries as usize].r#type = mem_type;
    params.e820_entries += 1;

    Ok(())
}

/// Offset of the 64-bit entry point from the start of a loaded bzImage,
/// per the Linux/x86 64-bit boot protocol.
const BZIMAGE_64BIT_ENTRY_OFFSET: u64 = 0x200;

/// Load a linux kernel into guest memory.
///
/// Supports uncompressed ELF (`vmlinux`) and `bzImage`; ELF is tried
/// first, falling back to the bzImage loader.
pub fn load_kernel(
    kernel: &File,
    guest_memory: &GuestMemoryMmap,
) -> Result<EntryPoint, ConfigurationError> {
    // Need to clone the File because reading from it
    // mutates it.
    let mut kernel_file = kernel
        .try_clone()
        .map_err(|_| ConfigurationError::KernelFile)?;

    let highmem_start = Some(GuestAddress(get_kernel_start()));

    // Try to load the image as an ELF (vmlinux); if it has no ELF magic,
    // we try to load it as a bzImage.
    match ElfLoader::load(guest_memory, None, &mut kernel_file, highmem_start) {
        Ok(elf_result) => {
            let mut entry_point_addr: GuestAddress = elf_result.kernel_load;
            let mut boot_prot: BootProtocol = BootProtocol::LinuxBoot;
            if let PvhBootCapability::PvhEntryPresent(pvh_entry_addr) = elf_result.pvh_boot_cap {
                // Use the PVH kernel entry point to boot the guest
                entry_point_addr = pvh_entry_addr;
                boot_prot = BootProtocol::PvhBoot;
            }

            Ok(EntryPoint {
                entry_addr: entry_point_addr,
                protocol: boot_prot,
                setup_header: None,
            })
        }
        // Not an ELF image: fall back to the bzImage loader.
        Err(KernelLoaderError::Elf(ElfError::InvalidElfMagicNumber)) => {
            let bzimage_result =
                BzImageLoader::load(guest_memory, None, &mut kernel_file, highmem_start)
                    .map_err(ConfigurationError::KernelLoader)?;

            // We jump to the 64-bit entry point, which only exists when the
            // image advertises it via XLF_KERNEL_64 (boot protocol >= 2.12).
            let hdr = bzimage_result
                .setup_header
                .expect("bzImage load always yields a setup header");
            if hdr.version < 0x020c || hdr.xloadflags & XLF_KERNEL_64 == 0 {
                return Err(ConfigurationError::BzImageMissing64BitEntry);
            }

            // Enter at the 64-bit entry point (BZIMAGE_64BIT_ENTRY_OFFSET
            // past the load address); the setup header seeds the zero page.
            let entry_addr = bzimage_result
                .kernel_load
                .checked_add(BZIMAGE_64BIT_ENTRY_OFFSET)
                .ok_or(ConfigurationError::KernelLoader(
                    KernelLoaderError::MemoryOverflow,
                ))?;

            Ok(EntryPoint {
                entry_addr,
                protocol: BootProtocol::LinuxBoot,
                setup_header: bzimage_result.setup_header,
            })
        }
        Err(err) => Err(ConfigurationError::KernelLoader(err)),
    }
}

#[cfg(test)]
mod tests {
    use linux_loader::loader::bootparam::boot_e820_entry;

    use super::*;
    use crate::vstate::memory::mib_to_bytes;
    use crate::vstate::memory::tests::arch_mem;
    use crate::vstate::resources::ResourceAllocator;

    #[test]
    fn regions_lt_4gb() {
        let regions = arch_memory_regions(1usize << 29);
        assert_eq!(1, regions.len());
        assert_eq!(GuestAddress(0), regions[0].0);
        assert_eq!(1usize << 29, regions[0].1);
    }

    #[test]
    fn regions_gt_4gb() {
        const MEMORY_SIZE: usize = (1 << 32) + 0x8000;

        let regions = arch_memory_regions(MEMORY_SIZE);
        assert_eq!(2, regions.len());
        assert_eq!(GuestAddress(0), regions[0].0);
        assert_eq!(GuestAddress(1u64 << 32), regions[1].0);

        assert_eq!(
            regions[1],
            (
                GuestAddress(layout::FIRST_ADDR_PAST_32BITS),
                MEMORY_SIZE - regions[0].1
            )
        );
    }

    #[test]
    fn test_system_configuration() {
        let no_vcpus = 4;
        let gm = crate::vstate::memory::tests::single_region_mem(0x10000);
        let mut resource_allocator = ResourceAllocator::new();
        let err = mptable::setup_mptable(&gm, &mut resource_allocator, 1);
        assert!(matches!(
            err.unwrap_err(),
            mptable::MptableError::NotEnoughMemory
        ));

        // Now assigning some memory that falls before the 32bit memory hole.
        let mem_size = mib_to_bytes(128);
        let gm = arch_mem(mem_size);
        let mut resource_allocator = ResourceAllocator::new();
        mptable::setup_mptable(&gm, &mut resource_allocator, no_vcpus).unwrap();
        configure_64bit_boot(&gm, GuestAddress(0), 0, &None, None).unwrap();
        configure_pvh(&gm, GuestAddress(0), &None).unwrap();

        // Now assigning some memory that is equal to the start of the 32bit memory hole.
        let mem_size = mib_to_bytes(3328);
        let gm = arch_mem(mem_size);
        let mut resource_allocator = ResourceAllocator::new();
        mptable::setup_mptable(&gm, &mut resource_allocator, no_vcpus).unwrap();
        configure_64bit_boot(&gm, GuestAddress(0), 0, &None, None).unwrap();
        configure_pvh(&gm, GuestAddress(0), &None).unwrap();

        // Now assigning some memory that falls after the 32bit memory hole.
        let mem_size = mib_to_bytes(3330);
        let gm = arch_mem(mem_size);
        let mut resource_allocator = ResourceAllocator::new();
        mptable::setup_mptable(&gm, &mut resource_allocator, no_vcpus).unwrap();
        configure_64bit_boot(&gm, GuestAddress(0), 0, &None, None).unwrap();
        configure_pvh(&gm, GuestAddress(0), &None).unwrap();
    }

    /// Builds a minimal bzImage that satisfies `linux_loader`'s bzImage
    /// loader checks, with the given `xloadflags`. Not a runnable kernel, only
    /// enough to exercise loader selection and zero-page seeding in
    /// `load_kernel`.
    fn make_fake_bzimage(xloadflags: u16) -> Vec<u8> {
        const SETUP_SECTS: u8 = 4;
        // Setup area (`(setup_sects + 1) * 512`) followed by a small protected-mode payload.
        let setup_len = (SETUP_SECTS as usize + 1) * 512;
        let mut image = vec![0u8; setup_len + 0x1000];

        // setup_sects @ 0x1f1
        image[0x1f1] = SETUP_SECTS;
        // boot_flag (0xaa55) @ 0x1fe
        image[0x1fe] = 0x55;
        image[0x1ff] = 0xaa;
        // "HdrS" header magic @ 0x202
        image[0x202..0x206].copy_from_slice(&0x5372_6448u32.to_le_bytes());
        // version @ 0x206 (>= 0x020c so xloadflags is defined)
        image[0x206..0x208].copy_from_slice(&0x020fu16.to_le_bytes());
        // loadflags @ 0x211 with LOADED_HIGH (bit 0) set
        image[0x211] = 0x01;
        // code32_start @ 0x214: default load address, must be >= HIMEM_START
        let code32_start = u32::try_from(get_kernel_start()).unwrap();
        image[0x214..0x218].copy_from_slice(&code32_start.to_le_bytes());
        // xloadflags @ 0x236
        image[0x236..0x238].copy_from_slice(&xloadflags.to_le_bytes());

        image
    }

    #[test]
    fn test_load_kernel_bzimage() {
        use std::io::{Seek, SeekFrom, Write};

        use vmm_sys_util::tempfile::TempFile;

        let gm = arch_mem(mib_to_bytes(128));

        let image = make_fake_bzimage(XLF_KERNEL_64);
        let tempfile = TempFile::new().unwrap();
        let mut file = tempfile.into_file();
        file.write_all(&image).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        let entry_point = load_kernel(&file, &gm).unwrap();

        assert_eq!(entry_point.protocol, BootProtocol::LinuxBoot);
        assert_eq!(
            entry_point.entry_addr,
            GuestAddress(get_kernel_start() + BZIMAGE_64BIT_ENTRY_OFFSET)
        );
        let hdr = entry_point
            .setup_header
            .expect("bzImage must yield a setup header");
        // Copy out of the packed struct to avoid unaligned references.
        let (header, setup_sects) = (hdr.header, hdr.setup_sects);
        assert_eq!(header, 0x5372_6448);
        assert_eq!(setup_sects, 4);
    }

    #[test]
    fn test_load_kernel_bzimage_without_64bit_entry() {
        use std::io::{Seek, SeekFrom, Write};

        use vmm_sys_util::tempfile::TempFile;

        let gm = arch_mem(mib_to_bytes(128));

        // A valid bzImage that does not advertise a 64-bit entry point.
        let image = make_fake_bzimage(0);
        let tempfile = TempFile::new().unwrap();
        let mut file = tempfile.into_file();
        file.write_all(&image).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        assert!(matches!(
            load_kernel(&file, &gm),
            Err(ConfigurationError::BzImageMissing64BitEntry)
        ));
    }

    #[test]
    fn test_load_kernel_invalid_image_is_rejected() {
        use std::io::{Seek, SeekFrom, Write};

        use vmm_sys_util::tempfile::TempFile;

        let gm = arch_mem(mib_to_bytes(128));

        // Neither valid ELF nor valid bzImage.
        let tempfile = TempFile::new().unwrap();
        let mut file = tempfile.into_file();
        file.write_all(&[0u8; 8192]).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        load_kernel(&file, &gm).unwrap_err();
    }

    #[test]
    fn test_add_e820_entry() {
        let e820_map = [(boot_e820_entry {
            addr: 0x1,
            size: 4,
            r#type: 1,
        }); 128];

        let expected_params = boot_params {
            e820_table: e820_map,
            e820_entries: 1,
            ..Default::default()
        };

        let mut params: boot_params = Default::default();
        add_e820_entry(
            &mut params,
            e820_map[0].addr,
            e820_map[0].size,
            e820_map[0].r#type,
        )
        .unwrap();
        assert_eq!(
            format!("{:?}", params.e820_table[0]),
            format!("{:?}", expected_params.e820_table[0])
        );
        assert_eq!(params.e820_entries, expected_params.e820_entries);

        // Exercise the scenario where the field storing the length of the e820 entry table is
        // is bigger than the allocated memory.
        params.e820_entries = u8::try_from(params.e820_table.len()).unwrap() + 1;
        assert!(
            add_e820_entry(
                &mut params,
                e820_map[0].addr,
                e820_map[0].size,
                e820_map[0].r#type
            )
            .is_err()
        );
    }
}
