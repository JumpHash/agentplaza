// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/x86_64/layout.rs (trimmed: the
// `device_manager::mmio::MMIO_LEN` constant is inlined, value `0x1000`) and
// src/utils/mod.rs (`mib_to_bytes` is imported from `vstate::memory`).

//! Magic addresses externally used to lay out x86_64 VMs.

use crate::vstate::memory::mib_to_bytes;

/// Initial stack for the boot CPU.
pub const BOOT_STACK_POINTER: u64 = 0x8ff0;

/// Kernel command line start address.
pub const CMDLINE_START: u64 = 0x20000;
/// Kernel command line maximum size.
pub const CMDLINE_MAX_SIZE: usize = 2048;

/// Start of the high memory.
pub const HIMEM_START: u64 = 0x0010_0000; // 1 MB.

// Typically, on x86 systems 24 IRQs are used for legacy devices (0-23).
// However, the first 5 are reserved.
// We allocate the remaining GSIs to MSIs.
/// First usable GSI for legacy interrupts (IRQ) on x86_64.
pub const GSI_LEGACY_START: u32 = 5;
/// Last usable GSI for legacy interrupts (IRQ) on x86_64.
pub const GSI_LEGACY_END: u32 = 23;
/// Number of legacy GSI (IRQ) available on x86_64.
pub const GSI_LEGACY_NUM: u32 = GSI_LEGACY_END - GSI_LEGACY_START + 1;
/// First GSI used by MSI after legacy GSI.
pub const GSI_MSI_START: u32 = GSI_LEGACY_END + 1;
/// The highest available GSI in KVM (KVM_MAX_IRQ_ROUTES=4096).
pub const GSI_MSI_END: u32 = 4095;
/// Number of GSI available for MSI.
pub const GSI_MSI_NUM: u32 = GSI_MSI_END - GSI_MSI_START + 1;

/// Address for the TSS setup.
pub const KVM_TSS_ADDRESS: u64 = 0xfffb_d000;

/// Address of the hvm_start_info struct used in PVH boot
pub const PVH_INFO_START: u64 = 0x6000;

/// Starting address of array of modules of hvm_modlist_entry type.
/// Used to enable initrd support using the PVH boot ABI.
pub const MODLIST_START: u64 = 0x6040;

/// Address of memory map table used in PVH boot. Can overlap
/// with the zero page address since they are mutually exclusive.
pub const MEMMAP_START: u64 = 0x7000;

/// The 'zero page', a.k.a linux kernel bootparams.
pub const ZERO_PAGE_START: u64 = 0x7000;

/// APIC address
pub const APIC_ADDR: u32 = 0xfee0_0000;

/// IOAPIC address
pub const IOAPIC_ADDR: u32 = 0xfec0_0000;

/// Location of RSDP pointer in x86 machines
pub const RSDP_ADDR: u64 = 0x000e_0000;

/// Start of memory region we will use for system data (MPTable, ACPI, etc). We are putting its
/// start address where EBDA normally starts, i.e. in the last 1 KiB of the first 640KiB of memory
pub const SYSTEM_MEM_START: u64 = 0x9fc00;

/// Size of memory region for system data.
///
/// We reserve the memory between the start of the EBDA up until the location of RSDP pointer,
/// [0x9fc00, 0xe0000) for system data. This is 257 KiB of memory we is enough for our needs and
/// future proof.
///
/// For ACPI we currently need:
///
/// FADT size: 276 bytes
/// XSDT size: 52 bytes (header: 36 bytes, plus pointers of FADT and MADT)
/// MADT size: 2104 bytes (header: 44 bytes, IO-APIC: 12 bytes, LocalAPIC: 8 * #vCPUS)
/// DSDT size: 1907 bytes (header: 36 bytes, legacy devices: 345, GED: 161, VMGenID: 87, VirtIO
///   devices: 71 bytes per device)
///
/// The above assumes a maximum of 256 vCPUs, because that's what ACPI allows, but currently
/// we have a hard limit of up to 32 vCPUs.
///
/// Moreover, for MPTable we need up to 5304 bytes (284 + 20 * #vCPUs) assuming again
/// a maximum number of 256 vCPUs.
///
/// 257KiB is more than we need, however we reserve this space for potential future use of
/// ACPI features (new tables and/or devices).
pub const SYSTEM_MEM_SIZE: u64 = RSDP_ADDR - SYSTEM_MEM_START;

/// First address that cannot be addressed using 32 bit anymore.
pub const FIRST_ADDR_PAST_32BITS: u64 = 1 << 32;

/// The size of the memory area reserved for MMIO 32-bit accesses.
pub const MMIO32_MEM_SIZE: u64 = mib_to_bytes(1024) as u64;
/// The start of the memory area reserved for MMIO 32-bit accesses.
pub const MMIO32_MEM_START: u64 = FIRST_ADDR_PAST_32BITS - MMIO32_MEM_SIZE;

// We dedicate the last 256 MiB of the 32-bit MMIO address space PCIe for memory-mapped access to
// configuration.
/// Size of MMIO region for PCIe configuration accesses.
pub const PCI_MMCONFIG_SIZE: u64 = 256 << 20;
/// Start of MMIO region for PCIe configuration accesses.
pub const PCI_MMCONFIG_START: u64 = IOAPIC_ADDR as u64 - PCI_MMCONFIG_SIZE;
/// MMIO space per PCIe segment
pub const PCI_MMIO_CONFIG_SIZE_PER_SEGMENT: u64 = 4096 * 256;

// We reserve 768 MiB for devices at the beginning of the MMIO region. This includes space both for
// pure MMIO and PCIe devices.

/// Memory region start for boot device.
pub const BOOT_DEVICE_MEM_START: u64 = MMIO32_MEM_START;

/// Beginning of memory region for device MMIO 32-bit accesses
pub const MEM_32BIT_DEVICES_START: u64 = BOOT_DEVICE_MEM_START + MMIO_LEN;
/// Size of memory region for device MMIO 32-bit accesses.
pub const MEM_32BIT_DEVICES_SIZE: u64 = PCI_MMCONFIG_START - MEM_32BIT_DEVICES_START;

// 64-bits region for MMIO accesses
/// The start of the memory area reserved for MMIO 64-bit accesses.
pub const MMIO64_MEM_START: u64 = 256 << 30;
/// The size of the memory area reserved for MMIO 64-bit accesses.
pub const MMIO64_MEM_SIZE: u64 = 256 << 30;

// At the moment, all of this region goes to devices
/// Beginning of memory region for device MMIO 64-bit accesses
pub const MEM_64BIT_DEVICES_START: u64 = MMIO64_MEM_START;
/// Size of memory region for device MMIO 32-bit accesses.
pub const MEM_64BIT_DEVICES_SIZE: u64 = MMIO64_MEM_SIZE;
/// First address past the 64-bit MMIO gap
pub const FIRST_ADDR_PAST_64BITS_MMIO: u64 = MMIO64_MEM_START + MMIO64_MEM_SIZE;
/// Size of the memory past 64-bit MMIO gap
pub const PAST_64BITS_MMIO_SIZE: u64 = 512 << 30;

/// The size of one MMIO device slot (Firecracker `device_manager::mmio::MMIO_LEN`).
pub const MMIO_LEN: u64 = 0x1000;

/// Guest-physical MMIO window of the `virtio-mmio` transport that carries the
/// VM's vsock device: the first slot of the 32-bit MMIO device region, which is
/// free in this port (upstream puts its boot device at
/// [`BOOT_DEVICE_MEM_START`], one slot below).
///
/// The guest learns about the window from the `virtio_mmio.device=` command
/// line the builder appends — an `x86_64` guest has no device tree to describe
/// it in. Nothing else may be mapped there: the window is outside every guest
/// RAM region (this port's memory map ends at [`MMIO32_MEM_START`] at the
/// latest).
pub const VSOCK_MMIO_START: u64 = MEM_32BIT_DEVICES_START;

/// Interrupt line of the vsock device: a legacy GSI of the in-kernel irqchip.
/// The serial console is on IRQ 4 and the i8042 controller on IRQ 1, so the
/// first legacy GSI is the first one left.
pub const VSOCK_GSI: u32 = GSI_LEGACY_START;

/// Guest-physical MMIO window of the `virtio-mmio` transport that carries the
/// VM's block device — its writable workspace volume: the second slot of the
/// 32-bit MMIO device region, one slot above the vsock device's transport.
///
/// The guest learns about the window from its own `virtio_mmio.device=` command
/// line entry, exactly as it does for the vsock device, and the two entries
/// carry different addresses and interrupts. Nothing else may be mapped there:
/// the window is outside every guest RAM region (this port's memory map ends at
/// [`MMIO32_MEM_START`] at the latest).
pub const WORKSPACE_MMIO_START: u64 = VSOCK_MMIO_START + MMIO_LEN;

/// Interrupt line of the block device: the legacy GSI after the vsock device's.
///
/// It must stay inside the identity map the MPTable's `mpc_intsrc` records
/// (ISA IRQ `i` -> GSI `i` for `0..=GSI_LEGACY_END`): that map is what keeps
/// the guest's `request_irq` for a `virtio_mmio.device=...:<irq>` transport
/// working, and adding an ACPI MADT I/O-APIC entry instead would move the
/// kernel off it (see [`crate::acpi::build_madt`]). GSI 6 is free on this
/// platform: IRQs 1 (i8042) and 4 (COM1) are used by the legacy devices, and
/// the floppy controller that traditionally owns IRQ 6 is a module the guest
/// never loads.
pub const WORKSPACE_GSI: u32 = GSI_LEGACY_START + 1;

/// Guest-physical MMIO window of the `virtio-mmio` transport that carries the
/// VM's balloon: the third slot of the 32-bit MMIO device region, one slot
/// above the workspace volume's. The guest learns about it from its own
/// `virtio_mmio.device=` entry, like the other two transports.
pub const BALLOON_MMIO_START: u64 = WORKSPACE_MMIO_START + MMIO_LEN;

/// Interrupt line of the balloon: the legacy GSI after the block device's
/// (GSI 7, the parallel port's, which this platform does not have). Like
/// [`WORKSPACE_GSI`] it stays inside the MPTable's identity map.
pub const BALLOON_GSI: u32 = GSI_LEGACY_START + 2;

/// Guest-physical MMIO window of the first shared data volume's `virtio-mmio` transport: the slot
/// after the balloon's. Volume `i` (up to [`crate::spawner::MAX_DATA_VOLUMES`]) is at
/// `DATA_MMIO_START + i * MMIO_LEN`, announced by its own `virtio_mmio.device=` entry.
pub const DATA_MMIO_START: u64 = BALLOON_MMIO_START + MMIO_LEN;

/// Interrupt line of the first data volume; volume `i` has `DATA_GSI_START + i`. GSI 16 onwards: past
/// the ISA lines (8 RTC, 9 ACPI, 12 the i8042's mouse port, 14/15 IDE) and inside the MPTable's
/// identity map, which runs to [`GSI_LEGACY_END`].
pub const DATA_GSI_START: u32 = 16;

/// The lab topology's network device (`devices::virtio::net`): the slot after the data volumes, with
/// GSI 20, the next legacy line after them. Only a lab topology's VM has it (see `spawner::check_net`).
pub const NET_MMIO_START: u64 =
    DATA_MMIO_START + crate::spawner::MAX_DATA_VOLUMES as u64 * MMIO_LEN;

/// The network device's interrupt line: `DATA_GSI_START` plus the data volumes.
pub const NET_GSI: u32 = DATA_GSI_START + crate::spawner::MAX_DATA_VOLUMES as u32;

// The net device's line is a legacy line, past the data volumes' and inside the MPTable's identity map.
const _: () = assert!(NET_GSI > DATA_GSI_START && NET_GSI <= GSI_LEGACY_END);
