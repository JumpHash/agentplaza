// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/mod.rs, src/vmm/src/arch/mod.rs
// (the `EntryPoint` and `GUEST_PAGE_SIZE` definitions, as in this tree's
// `arch/x86_64/mod.rs`) and src/vmm/src/initrd.rs (the `InitrdConfig` loader,
// also as in `arch/x86_64/mod.rs`).
//
// Trimmed: the CPU-template machinery (`CpuConfiguration`, `CustomCpuTemplate`,
// `VcpuConfig`, the CLIDR values fed from a template) is dropped — every vCPU
// is initialized with the default KVM features; the `DeviceManager` parameter of
// `configure_system_for_boot` is replaced by a [`fdt::SerialFdtInfo`] descriptor;
// the metrics/logger crates become `eprintln!`; the KVM-dependent submodules are
// gated to Linux so `layout.rs`, `fdt.rs` and the entry-point/loader helpers
// compile on any aarch64 target (the Hypervisor.framework backend on macOS
// shares them).
//
// Runtime note: the GIC version selection (`gic::create_gic`, GICv3 first with a
// GICv2 fallback — the Raspberry Pi 4/5 path) is copied from upstream verbatim
// and can only be validated end to end on a real GICv2 host (Raspberry Pi
// 4/5), which this port's development machine is not: here the GICv3 path
// compiles and its register model is host-tested, while the GICv2 fallback
// remains unexercised at runtime.

pub(crate) mod cache_info;
mod fdt;
/// Module for the global interrupt controller configuration.
#[cfg(target_os = "linux")]
pub mod gic;
/// Architecture specific KVM-related code
#[cfg(target_os = "linux")]
pub mod kvm;
/// Layout for this aarch64 system.
pub mod layout;
/// Logic for configuring aarch64 registers.
#[cfg(target_os = "linux")]
pub mod regs;
/// Architecture specific vCPU code
#[cfg(target_os = "linux")]
pub mod vcpu;
/// Architecture specific VM state code
#[cfg(target_os = "linux")]
pub mod vm;

// The FDT generator and its descriptors are also the Hypervisor.framework
// backend's entry point into this module (`create_fdt`, the `GicFdt` trait a
// userspace interrupt controller implements, and the serial description).
pub use fdt::{FdtError, GicFdt, SerialFdtInfo, VirtioMmioFdtInfo, create_fdt};

use std::cmp::min;
use std::fs::File;
use std::os::unix::fs::MetadataExt;

use linux_loader::loader::KernelLoader;
use linux_loader::loader::pe::PE as Loader;
use vm_memory::{GuestMemoryBackend, GuestMemoryRegion, ReadVolatile};

#[cfg(target_os = "linux")]
use linux_loader::loader::Cmdline;
#[cfg(target_os = "linux")]
use vm_memory::Bytes;

#[cfg(target_os = "linux")]
use crate::arch::aarch64::vcpu::KvmVcpuError;
use crate::vstate::memory::{Address, GuestAddress, GuestMemoryMmap, u64_to_usize, usize_to_u64};
#[cfg(target_os = "linux")]
use crate::vstate::vcpu::Vcpu;
#[cfg(target_os = "linux")]
use crate::vstate::vm::KvmVm;

/// Default page size for the guest OS (upstream: `arch/mod.rs`).
pub const GUEST_PAGE_SIZE: usize = 4096;

/// Errors thrown while configuring an aarch64 system.
#[derive(Debug, thiserror::Error)]
pub enum ConfigurationError {
    /// Cannot copy kernel file fd
    #[error("Cannot copy kernel file fd")]
    KernelFile,
    /// Cannot load kernel due to invalid memory configuration or invalid kernel image: {0}
    #[error("Cannot load kernel due to invalid memory configuration or invalid kernel image: {0}")]
    KernelLoader(#[from] linux_loader::loader::Error),
    /// Cannot build the kernel command line: {0}
    #[error("Cannot build the kernel command line: {0}")]
    CommandLine(#[from] linux_loader::cmdline::Error),
    /// Failed to read host cache information: {0}
    #[error("Failed to read host cache information: {0}")]
    CacheInfo(#[from] cache_info::CacheInfoError),
    /// Failed to create a Flattened Device Tree for this aarch64 microVM: {0}
    #[cfg(target_os = "linux")]
    #[error("Failed to create a Flattened Device Tree for this aarch64 microVM: {0}")]
    SetupFdt(#[from] fdt::FdtError),
    /// Failed to write to guest memory: {0}
    #[cfg(target_os = "linux")]
    #[error("Failed to write to guest memory: {0}")]
    MemoryError(#[from] vm_memory::GuestMemoryError),
    /// Error configuring the vcpu: {0}
    #[cfg(target_os = "linux")]
    #[error("Error configuring the vcpu: {0}")]
    VcpuConfigure(#[from] KvmVcpuError),
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
/// (upstream: `src/initrd.rs`; this tree also carries a copy in
/// `arch/x86_64/mod.rs`).
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
        let size = u64_to_usize(size);
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

/// Supported boot protocol (upstream: `arch/mod.rs`; the x86_64-only PVH
/// protocol is not part of this module's copy).
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum BootProtocol {
    /// Linux 64-bit boot protocol
    LinuxBoot,
}

/// Specifies the entry point address where the guest must start
/// executing code, as well as which boot protocol is to be used
/// to configure the guest initial state (upstream: `arch/mod.rs`).
#[derive(Debug, Copy, Clone)]
pub struct EntryPoint {
    /// Address in guest memory where the guest must start execution
    pub entry_addr: GuestAddress,
    /// Specifies which boot protocol to use
    pub protocol: BootProtocol,
}

/// Adds in [`regions`] the valid memory regions suitable for RAM taking into account a gap in the
/// available address space and returns the remaining region (if any) past this gap
/// (upstream: `arch/mod.rs`).
fn arch_memory_regions_with_gap(
    regions: &mut Vec<(GuestAddress, usize)>,
    region_start: usize,
    region_size: usize,
    gap_start: usize,
    gap_size: usize,
) -> Option<(usize, usize)> {
    // 0-sized gaps don't really make sense. We should never receive such a gap.
    assert!(gap_size > 0);

    let first_addr_past_gap = gap_start + gap_size;
    match (region_start + region_size).checked_sub(gap_start) {
        // case0: region fits all before gap
        None | Some(0) => {
            regions.push((GuestAddress(region_start as u64), region_size));
            None
        }
        // case1: region starts before the gap and goes past it
        Some(remaining) if region_start < gap_start => {
            regions.push((GuestAddress(region_start as u64), gap_start - region_start));
            Some((first_addr_past_gap, remaining))
        }
        // case2: region starts past the gap
        Some(_) => Some((first_addr_past_gap.max(region_start), region_size)),
    }
}

/// Returns a Vec of the valid memory addresses for aarch64.
/// See [`layout`](layout) module for a drawing of the specific memory model for this platform.
pub fn arch_memory_regions(size: usize) -> Vec<(GuestAddress, usize)> {
    assert!(size > 0, "Attempt to allocate guest memory of length 0");

    let dram_size = min(size, layout::DRAM_MEM_MAX_SIZE);

    if dram_size != size {
        eprintln!(
            "warning: requested memory size {size} exceeds architectural maximum (1022GiB). \
             Size has been truncated to {dram_size}"
        );
    }

    let mut regions = vec![];
    if let Some((offset, remaining)) = arch_memory_regions_with_gap(
        &mut regions,
        u64_to_usize(layout::DRAM_MEM_START),
        dram_size,
        u64_to_usize(layout::MMIO64_MEM_START),
        u64_to_usize(layout::MMIO64_MEM_SIZE),
    ) {
        regions.push((GuestAddress(offset as u64), remaining));
    }

    regions
}

/// Configures the system for booting Linux.
#[cfg(target_os = "linux")]
pub fn configure_system_for_boot(
    vm: &KvmVm,
    vcpus: &mut [Vcpu],
    entry_point: EntryPoint,
    initrd: &Option<InitrdConfig>,
    boot_cmdline: Cmdline,
    serial: &SerialFdtInfo,
    virtio_mmio: &[VirtioMmioFdtInfo],
) -> Result<(), ConfigurationError> {
    // Upstream builds a `CpuConfiguration` here (from a CPU template), which is
    // also where every vCPU is initialized (`KvmVcpu::init`) and where the
    // template's register modifiers are applied. This port has no CPU
    // templates: each vCPU is initialized with the default features (PSCI 0.2,
    // plus `KVM_ARM_VCPU_POWER_OFF` for secondary vCPUs) and no registers are
    // modified before the boot registers below.
    let optional_capabilities = vm.kvm().optional_capabilities();
    for vcpu in vcpus.iter_mut() {
        vcpu.kvm_vcpu.init()?;
        vcpu.kvm_vcpu
            .configure(vm.guest_memory(), entry_point, &optional_capabilities)?;
    }

    // Override CLIDR_EL1 ctype/LoC fields on each vCPU to match the host's real cache topology.
    // See `override_clidr` for details.
    //
    // Gate the overwrite to only kernels 6.10 and newer. Before 6.10 version, KVM is
    // unconditionally resetting ID registers on secondary vcpus on PSCI bootup. This causes
    // secondary vcpus to have default CLIDR_EL1 value, different from the vcpu0 (which never goes
    // through the PSCI boot up process). This difference causes the guest kernel to misconfigure
    // its scheduling which significantly affects the performance inside the guest.
    //
    // Commit in 6.10 that resolves this issue:
    // "e016333745c70c960e02b4a9b123c807669d2b22"("KVM: arm64: Only reset vCPU-scoped feature ID
    // regs once").
    if host_kernel_release_at_least(6, 10) {
        override_clidr(vcpus)?;
    }

    let vcpu_mpidr = vcpus
        .iter_mut()
        .map(|cpu| cpu.kvm_vcpu.get_mpidr())
        .collect::<Result<Vec<_>, _>>()
        .map_err(KvmVcpuError::ConfigureRegisters)?;
    let cmdline = boot_cmdline.as_cstring()?;

    let fdt = fdt::create_fdt(
        vm.guest_memory(),
        vcpu_mpidr,
        cmdline,
        serial,
        virtio_mmio,
        vm.get_irqchip(),
        initrd,
    )?;

    let fdt_address = GuestAddress(get_fdt_addr(vm.guest_memory()));
    vm.guest_memory().write_slice(fdt.as_slice(), fdt_address)?;

    Ok(())
}

/// Override CLIDR_EL1 ctype/LoC fields on each vCPU to match the host's real
/// cache topology.
///
/// Since host kernel 6.3 (commit 7af0c2534f4c), KVM fabricates CLIDR_EL1
/// instead of passing through the host's real value. This can cause the guest
/// to see fewer cache levels than actually exist. Guest kernels >= 6.1.156
/// backported `init_of_cache_level()` which counts cache leaves from the DT,
/// while `populate_cache_leaves()` uses CLIDR_EL1. If the DT (built from host
/// sysfs) describes different cache entries than CLIDR_EL1, the mismatch
/// causes cache sysfs entries to not be created.
///
/// We read the current (possibly fabricated) CLIDR_EL1, replace only the ctype
/// and LoC fields with values derived from sysfs, and preserve all other fields
/// (LoUU, LoUIS, ICB, Ttype). This is safe on pre-6.3 kernels where CLIDR
/// already matches sysfs — the write is skipped as a no-op.
#[cfg(target_os = "linux")]
fn override_clidr(vcpus: &[Vcpu]) -> Result<(), ConfigurationError> {
    let mut l1_caches = Vec::new();
    let mut non_l1_caches = Vec::new();
    cache_info::read_cache_config(&mut l1_caches, &mut non_l1_caches)?;

    // If sysfs reports no L1 caches, we cannot build a meaningful CLIDR.
    // Writing an all-zero CLIDR would tell the guest there are no caches,
    // which is worse than whatever KVM fabricated. Leave it alone.
    if l1_caches.is_empty() {
        eprintln!("warning: no L1 caches found in sysfs, skipping CLIDR override");
        return Ok(());
    }

    let sysfs_clidr = cache_info::build_clidr_from_caches(&l1_caches, &non_l1_caches);

    let Some(first_vcpu) = vcpus.first() else {
        return Ok(());
    };

    // Reading/writing CLIDR_EL1 via KVM_SET_ONE_REG may not be supported on
    // older kernels (pre-6.3). In that case KVM passes through the real host
    // CLIDR and the override is unnecessary, so we warn and continue.
    let mut clidr_bytes = [0_u8; 8];
    if let Err(e) = first_vcpu
        .kvm_vcpu
        .fd
        .get_one_reg(regs::CLIDR_EL1, &mut clidr_bytes)
    {
        eprintln!("warning: failed to read CLIDR_EL1, skipping override: {e}");
        return Ok(());
    }
    let cur_clidr = u64::from_le_bytes(clidr_bytes);

    let new_clidr = cache_info::merge_clidr(cur_clidr, sysfs_clidr);

    if new_clidr != cur_clidr {
        for vcpu in vcpus.iter() {
            if let Err(e) = vcpu
                .kvm_vcpu
                .fd
                .set_one_reg(regs::CLIDR_EL1, &new_clidr.to_le_bytes())
            {
                eprintln!(
                    "warning: failed to set CLIDR_EL1 to {new_clidr:#x} on vCPU {}, skipping \
                     override: {e}",
                    vcpu.kvm_vcpu.index
                );
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Returns the memory address where the kernel could be loaded.
pub fn get_kernel_start() -> u64 {
    layout::SYSTEM_MEM_START + layout::SYSTEM_MEM_SIZE
}

/// Returns the memory address where the initrd could be loaded.
pub fn initrd_load_addr(guest_mem: &GuestMemoryMmap, initrd_size: usize) -> Option<u64> {
    let rounded_size = crate::align_up!(usize_to_u64(initrd_size), usize_to_u64(GUEST_PAGE_SIZE));
    GuestAddress(get_fdt_addr(guest_mem))
        .checked_sub(rounded_size)
        .filter(|&addr| guest_mem.address_in_range(addr))
        .map(|addr| addr.raw_value())
}

/// Returns the address where the device tree blob is loaded.
///
/// Upstream keeps this private; the Hypervisor.framework backend places (and
/// points the vCPU at) the same address, so it is public here.
pub fn get_fdt_addr(mem: &GuestMemoryMmap) -> u64 {
    // Find the first (and only) DRAM region.
    let Some(dram_region) = mem.iter().next() else {
        // No memory to place the FDT in: the caller allocated no DRAM region,
        // which `arch_memory_regions` rules out for any non-zero guest size.
        return layout::DRAM_MEM_START;
    };

    // If the memory allocated is smaller than the size allocated for the FDT,
    // we return the start of the DRAM so that
    // we allow the code to try and load the FDT.
    dram_region
        .last_addr()
        .checked_sub(layout::FDT_MAX_SIZE as u64 - 1)
        .filter(|&addr| mem.address_in_range(addr))
        .map(|addr| addr.raw_value())
        .unwrap_or(layout::DRAM_MEM_START)
}

/// Load linux kernel into guest memory.
pub fn load_kernel(
    kernel: &File,
    guest_memory: &GuestMemoryMmap,
) -> Result<EntryPoint, ConfigurationError> {
    // Need to clone the File because reading from it
    // mutates it.
    let mut kernel_file = kernel
        .try_clone()
        .map_err(|_| ConfigurationError::KernelFile)?;

    let entry_addr = Loader::load(
        guest_memory,
        Some(GuestAddress(get_kernel_start())),
        &mut kernel_file,
        None,
    )?;

    Ok(EntryPoint {
        entry_addr: entry_addr.kernel_load,
        protocol: BootProtocol::LinuxBoot,
    })
}

/// Returns whether the host's kernel release is at least `major.minor`.
///
/// Upstream compares full `Version`s (`utils::Version` +
/// `arch::host_kernel_version()`); this port has no `Version` type, so it reads
/// and compares the major and minor components it needs.
#[cfg(target_os = "linux")]
fn host_kernel_release_at_least(major: u8, minor: u8) -> bool {
    match crate::sys::uname_release() {
        Some(release) => kernel_release_at_least(&release, major, minor),
        None => false,
    }
}

/// Whether a Linux `uname -r` release string is at least `major.minor`.
///
/// Missing or non-numeric components count as 0, as upstream's
/// `parse_kernel_release` does.
#[cfg(target_os = "linux")]
fn kernel_release_at_least(release: &str, major: u8, minor: u8) -> bool {
    let mut components = release.split(|c: char| !c.is_ascii_digit());
    let host_major = components
        .next()
        .and_then(|component| component.parse::<u8>().ok())
        .unwrap_or(0);
    let host_minor = components
        .next()
        .and_then(|component| component.parse::<u8>().ok())
        .unwrap_or(0);

    (host_major, host_minor) >= (major, minor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::aarch64::layout::{
        DRAM_MEM_MAX_SIZE, DRAM_MEM_START, FDT_MAX_SIZE, FIRST_ADDR_PAST_64BITS_MMIO,
        MMIO64_MEM_START,
    };
    use crate::vstate::memory::tests::arch_mem;

    #[test]
    fn test_regions_lt_1024gb() {
        let regions = arch_memory_regions(1usize << 29);
        assert_eq!(1, regions.len());
        assert_eq!(GuestAddress(DRAM_MEM_START), regions[0].0);
        assert_eq!(1usize << 29, regions[0].1);
    }

    #[test]
    fn test_regions_gt_1024gb() {
        let regions = arch_memory_regions(1usize << 41);
        assert_eq!(2, regions.len());
        assert_eq!(GuestAddress(DRAM_MEM_START), regions[0].0);
        assert_eq!(MMIO64_MEM_START - DRAM_MEM_START, regions[0].1 as u64);
        assert_eq!(GuestAddress(FIRST_ADDR_PAST_64BITS_MMIO), regions[1].0);
        assert_eq!(
            DRAM_MEM_MAX_SIZE as u64 - MMIO64_MEM_START + DRAM_MEM_START,
            regions[1].1 as u64
        );
    }

    #[test]
    fn test_get_fdt_addr() {
        let mem = arch_mem(FDT_MAX_SIZE - 0x1000);
        assert_eq!(get_fdt_addr(&mem), DRAM_MEM_START);

        let mem = arch_mem(FDT_MAX_SIZE);
        assert_eq!(get_fdt_addr(&mem), DRAM_MEM_START);

        let mem = arch_mem(FDT_MAX_SIZE + 0x1000);
        assert_eq!(get_fdt_addr(&mem), 0x1000 + DRAM_MEM_START);
    }

    /// The initrd is placed below the FDT, page-aligned, and only if it fits
    /// in the guest's DRAM.
    #[test]
    fn test_initrd_load_addr() {
        let mem = arch_mem(0x2000_0000);
        let fdt_addr = get_fdt_addr(&mem);

        // A 0x1000-byte initrd ends exactly at the FDT.
        assert_eq!(initrd_load_addr(&mem, 0x1000), Some(fdt_addr - 0x1000),);
        // Unaligned sizes are rounded up to the guest page size.
        assert_eq!(initrd_load_addr(&mem, 0x1001), Some(fdt_addr - 0x2000),);
        // Larger than the guest's DRAM: no address.
        assert_eq!(initrd_load_addr(&mem, 0x1_0000_0000), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_kernel_release_at_least() {
        assert!(kernel_release_at_least("6.10.0", 6, 10));
        assert!(kernel_release_at_least("6.10.0-rc1", 6, 10));
        assert!(kernel_release_at_least("7.2.8-arch1-2", 6, 10));
        assert!(!kernel_release_at_least("6.9.12", 6, 10));
        assert!(!kernel_release_at_least("5.15.0", 6, 10));
        // Missing components count as 0, like upstream's parser.
        assert!(!kernel_release_at_least("6", 6, 10));
        assert!(kernel_release_at_least("", 0, 0));
    }
}
