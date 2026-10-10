// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/aarch64/fdt.rs (trimmed: the
// `DeviceManager` parameter is replaced by two small descriptors — [`GicFdt`]
// for the interrupt controller and [`SerialFdtInfo`] for the console — because
// this port has no device manager (its only MMIO device is the serial console,
// at the fixed [`layout::SERIAL_MEM_START`] address); the VMGenID, vmclock,
// PCI, RTC, PL061 GPIO and virtio-mmio nodes, the `rng-seed` property (needs
// `aws-lc-rs`) and the `linux,pci-probe-only` property (this port has no PCI)
// are dropped).
//
// This module is deliberately free of any KVM import: the Hypervisor.framework
// backend describes its userspace interrupt controller by implementing
// [`GicFdt`].

use std::ffi::CString;

use vm_fdt::{Error as VmFdtError, FdtWriter, FdtWriterNode};
use vm_memory::{GuestMemoryBackend, GuestMemoryError};

use super::cache_info::{CacheEntry, read_cache_config};
use super::layout;
use crate::vstate::memory::{Address, GuestMemoryMmap, GuestMemoryRegion};

// This is a value for uniquely identifying the FDT node declaring the interrupt controller.
const GIC_PHANDLE: u32 = 1;
// This is a value for uniquely identifying the FDT node containing the clock definition.
const CLOCK_PHANDLE: u32 = 2;
// You may be wondering why this big value?
// This phandle is used to uniquely identify the FDT nodes containing cache information. Each cpu
// can have a variable number of caches, some of these caches may be shared with other cpus.
// So, we start the indexing of the phandles used from a really big number and then subtract from
// it as we need more and more phandle for each cache representation.
const LAST_CACHE_PHANDLE: u32 = 4000;
// Read the documentation specified when appending the root node to the FDT.
const ADDRESS_CELLS: u32 = 0x2;
const SIZE_CELLS: u32 = 0x2;

// As per kvm tool and
// https://www.kernel.org/doc/Documentation/devicetree/bindings/interrupt-controller/arm%2Cgic.txt
// Look for "The 1st cell..."
const GIC_FDT_IRQ_TYPE_SPI: u32 = 0;
const GIC_FDT_IRQ_TYPE_PPI: u32 = 1;

// From https://elixir.bootlin.com/linux/v4.9.62/source/include/dt-bindings/interrupt-controller/irq.h#L17
const IRQ_TYPE_EDGE_RISING: u32 = 1;
const IRQ_TYPE_LEVEL_HI: u32 = 4;

/// Interrupt controller description consumed by [`create_fdt`].
///
/// Upstream takes the KVM `GICDevice` enum here; this trait keeps `fdt.rs`
/// free of KVM types so the Hypervisor.framework backend can describe its own
/// (userspace) GICv3 with the same generator.
pub trait GicFdt {
    /// The `compatible` string of the `intc` node.
    fn compatible(&self) -> &str;
    /// The `reg` cells of the `intc` node (distributor, then redistributors).
    fn device_properties(&self) -> &[u64];
    /// The PPI number of the maintenance interrupt.
    fn fdt_maint_irq(&self) -> u32;
    /// The `reg` cells of the MSI controller node, if the GIC has one.
    fn msi_properties(&self) -> Option<&[u64]>;
}

/// MMIO location and interrupt of the `ns16550a` serial console.
#[derive(Debug, Clone, Copy)]
pub struct SerialFdtInfo {
    /// Base address of the UART registers.
    pub addr: u64,
    /// Length of the MMIO window.
    pub len: u64,
    /// The GSI of the UART (the FDT encodes the SPI number, i.e. the GSI as
    /// programmed into the interrupt controller, KVM's in-kernel vGIC included).
    pub gsi: u32,
}

/// Errors thrown while configuring the Flattened Device Tree for aarch64.
#[derive(Debug, thiserror::Error)]
pub enum FdtError {
    /// Create FDT error: {0}
    #[error("Create FDT error: {0}")]
    CreateFdt(#[from] VmFdtError),
    /// Read cache info error: {0}
    #[error("Read cache info error: {0}")]
    ReadCacheInfo(String),
    /// Failure in writing FDT in memory.
    #[error("Failure in writing FDT in memory: {0}")]
    WriteFdtToMemory(#[from] GuestMemoryError),
    /// Guest memory has no DRAM region to describe.
    #[error("Guest memory has no DRAM region to describe")]
    NoDramRegion,
}

/// Creates the flattened device tree for this aarch64 microVM.
pub fn create_fdt(
    guest_mem: &GuestMemoryMmap,
    vcpu_mpidr: Vec<u64>,
    cmdline: CString,
    serial: &SerialFdtInfo,
    gic_device: &dyn GicFdt,
    initrd: &Option<super::InitrdConfig>,
) -> Result<Vec<u8>, FdtError> {
    // Allocate stuff necessary for storing the blob.
    let mut fdt_writer = FdtWriter::new()?;

    // For an explanation why these nodes were introduced in the blob take a look at
    // https://github.com/torvalds/linux/blob/master/Documentation/devicetree/booting-without-of.txt#L845
    // Look for "Required nodes and properties".

    // Header or the root node as per above mentioned documentation.
    let root = fdt_writer.begin_node("")?;
    fdt_writer.property_string("compatible", "linux,dummy-virt")?;
    // For info on #address-cells and size-cells read "Note about cells and address representation"
    // from the above mentioned txt file.
    fdt_writer.property_u32("#address-cells", ADDRESS_CELLS)?;
    fdt_writer.property_u32("#size-cells", SIZE_CELLS)?;
    // This is not mandatory but we use it to point the root node to the node
    // containing description of the interrupt controller for this VM.
    fdt_writer.property_u32("interrupt-parent", GIC_PHANDLE)?;
    create_cpu_nodes(&mut fdt_writer, &vcpu_mpidr)?;
    create_memory_node(&mut fdt_writer, guest_mem)?;
    create_chosen_node(&mut fdt_writer, cmdline, initrd)?;
    create_gic_node(&mut fdt_writer, gic_device)?;
    create_timer_node(&mut fdt_writer)?;
    create_clock_node(&mut fdt_writer)?;
    create_psci_node(&mut fdt_writer)?;
    create_serial_node(&mut fdt_writer, serial)?;

    // End Header node.
    fdt_writer.end_node(root)?;

    // Allocate another buffer so we can format and then write fdt to guest.
    let fdt_final = fdt_writer.finish()?;
    Ok(fdt_final)
}

// Following are the auxiliary function for creating the different nodes that we append to our FDT.
fn create_cpu_nodes(fdt: &mut FdtWriter, vcpu_mpidr: &[u64]) -> Result<(), FdtError> {
    // Since the L1 caches are not shareable among CPUs and they are direct attributes of the
    // cpu in the device tree, we process the L1 and non-L1 caches separately.
    // We use sysfs for extracting the cache information.
    let mut l1_caches: Vec<CacheEntry> = Vec::new();
    let mut non_l1_caches: Vec<CacheEntry> = Vec::new();
    // We use sysfs for extracting the cache information.
    read_cache_config(&mut l1_caches, &mut non_l1_caches)
        .map_err(|err| FdtError::ReadCacheInfo(err.to_string()))?;

    // See https://github.com/torvalds/linux/blob/master/Documentation/devicetree/bindings/arm/cpus.yaml.
    let cpus = fdt.begin_node("cpus")?;
    // As per documentation, on ARM v8 64-bit systems value should be set to 2.
    fdt.property_u32("#address-cells", 0x02)?;
    fdt.property_u32("#size-cells", 0x0)?;
    let num_cpus = vcpu_mpidr.len();
    for (cpu_index, mpidr) in vcpu_mpidr.iter().enumerate() {
        let cpu = fdt.begin_node(&format!("cpu@{:x}", cpu_index))?;
        fdt.property_string("device_type", "cpu")?;
        fdt.property_string("compatible", "arm,arm-v8")?;
        // The power state coordination interface (PSCI) needs to be enabled for
        // all vcpus.
        fdt.property_string("enable-method", "psci")?;
        // Set the field to first 24 bits of the MPIDR - Multiprocessor Affinity Register.
        // See http://infocenter.arm.com/help/index.jsp?topic=/com.arm.doc.ddi0488c/BABHBJCI.html.
        fdt.property_u64("reg", mpidr & 0x7FFFFF)?;

        for cache in l1_caches.iter() {
            // Please check out
            // https://github.com/devicetree-org/devicetree-specification/releases/download/v0.3/devicetree-specification-v0.3.pdf,
            // section 3.8.
            if let Some(size) = cache.size_ {
                fdt.property_u32(cache.type_.of_cache_size(), size)?;
            }
            if let Some(line_size) = cache.line_size {
                fdt.property_u32(cache.type_.of_cache_line_size(), u32::from(line_size))?;
            }
            if let Some(number_of_sets) = cache.number_of_sets {
                fdt.property_u32(cache.type_.of_cache_sets(), number_of_sets)?;
            }
        }

        // Some of the non-l1 caches can be shared amongst CPUs. You can see an example of a shared
        // scenario in https://github.com/devicetree-org/devicetree-specification/releases/download/v0.3/devicetree-specification-v0.3.pdf,
        // 3.8.1 Example.
        let mut prev_level = 1;
        let mut cache_node: Option<FdtWriterNode> = None;
        for cache in non_l1_caches.iter() {
            // We append the next-level-cache node (the node that specifies the cache hierarchy)
            // in the next iteration. For example,
            // L2-cache {
            //      cache-size = <0x8000> ----> first iteration
            //      next-level-cache = <&l3-cache> ---> second iteration
            // }
            // The cpus per unit cannot be 0 since the sysfs will also include the current cpu
            // in the list of shared cpus so it needs to be at least 1. Firecracker trusts the host.
            // The operation is safe since we already checked when creating cache attributes that
            // cpus_per_unit is not 0 (.e look for mask_str2bit_count function).
            let cache_phandle = LAST_CACHE_PHANDLE
                - u32::try_from(
                    num_cpus * (cache.level - 2) as usize
                        + cpu_index / cache.cpus_per_unit as usize,
                )
                .unwrap(); // Safe because the number of CPUs is bounded

            if prev_level != cache.level {
                fdt.property_u32("next-level-cache", cache_phandle)?;
                if prev_level > 1 && cache_node.is_some() {
                    fdt.end_node(cache_node.take().unwrap())?;
                }
            }

            if cpu_index % cache.cpus_per_unit as usize == 0 {
                cache_node = Some(fdt.begin_node(&format!(
                    "l{}-{}-cache",
                    cache.level,
                    cpu_index / cache.cpus_per_unit as usize
                ))?);
                fdt.property_u32("phandle", cache_phandle)?;
                fdt.property_string("compatible", "cache")?;
                fdt.property_u32("cache-level", u32::from(cache.level))?;
                if let Some(size) = cache.size_ {
                    fdt.property_u32(cache.type_.of_cache_size(), size)?;
                }
                if let Some(line_size) = cache.line_size {
                    fdt.property_u32(cache.type_.of_cache_line_size(), u32::from(line_size))?;
                }
                if let Some(number_of_sets) = cache.number_of_sets {
                    fdt.property_u32(cache.type_.of_cache_sets(), number_of_sets)?;
                }
                if let Some(cache_type) = cache.type_.of_cache_type() {
                    fdt.property_null(cache_type)?;
                }
                prev_level = cache.level;
            }
        }
        if let Some(node) = cache_node {
            fdt.end_node(node)?;
        }

        fdt.end_node(cpu)?;
    }
    fdt.end_node(cpus)?;

    Ok(())
}

fn create_memory_node(fdt: &mut FdtWriter, guest_mem: &GuestMemoryMmap) -> Result<(), FdtError> {
    // See https://github.com/torvalds/linux/blob/master/Documentation/devicetree/booting-without-of.txt#L960
    // for an explanation of this.

    // On ARM we reserve some memory so that it can be utilized for devices like VMGenID to send
    // data to kernel drivers. The range of this memory is:
    //
    // [layout::DRAM_MEM_START, layout::DRAM_MEM_START + layout::SYSTEM_MEM_SIZE)
    //
    // The reason we do this is that Linux does not allow remapping system memory. However, without
    // remap, kernel drivers cannot get virtual addresses to read data from device memory. Leaving
    // this memory region out allows Linux kernel modules to remap and thus read this region.

    // Pick the first (and only) memory region. Upstream selects it by its
    // `GuestRegionType` (part of its memory hotplug support, trimmed here);
    // this port registers every DRAM region in address order, so the first is
    // the low DRAM region the kernel is loaded into.
    let dram_region = guest_mem.iter().next().ok_or(FdtError::NoDramRegion)?;
    // Find the start of memory after the system memory region
    let start_addr = dram_region
        .start_addr()
        .unchecked_add(layout::SYSTEM_MEM_SIZE);
    // Size of the memory is the region size minus the system memory size
    let mem_size = dram_region.len() - layout::SYSTEM_MEM_SIZE;

    let mem_reg_prop = &[start_addr.raw_value(), mem_size];
    let mem = fdt.begin_node("memory@ram")?;
    fdt.property_string("device_type", "memory")?;
    fdt.property_array_u64("reg", mem_reg_prop)?;
    fdt.end_node(mem)?;

    Ok(())
}

fn create_chosen_node(
    fdt: &mut FdtWriter,
    cmdline: CString,
    initrd: &Option<super::InitrdConfig>,
) -> Result<(), FdtError> {
    let chosen = fdt.begin_node("chosen")?;
    // Workaround to be able to reuse an existing property_*() method; in property_string() method,
    // the cmdline is reconverted to a CString to be written in memory as a null terminated string.
    let cmdline_string = cmdline
        .into_string()
        .map_err(|_| vm_fdt::Error::InvalidString)?;
    fdt.property_string("bootargs", cmdline_string.as_str())?;

    if let Some(initrd_config) = initrd {
        fdt.property_u64("linux,initrd-start", initrd_config.address.raw_value())?;
        fdt.property_u64(
            "linux,initrd-end",
            initrd_config.address.raw_value() + initrd_config.size as u64,
        )?;
    }

    fdt.end_node(chosen)?;

    Ok(())
}

fn create_gic_node(fdt: &mut FdtWriter, gic_device: &dyn GicFdt) -> Result<(), FdtError> {
    let interrupt = fdt.begin_node("intc")?;
    fdt.property_string("compatible", gic_device.compatible())?;
    fdt.property_null("interrupt-controller")?;
    // "interrupt-cells" field specifies the number of cells needed to encode an
    // interrupt source. The type shall be a <u32> and the value shall be 3 if no PPI affinity
    // description is required.
    fdt.property_u32("#interrupt-cells", 3)?;
    fdt.property_array_u64("reg", gic_device.device_properties())?;
    fdt.property_u32("phandle", GIC_PHANDLE)?;
    fdt.property_u32("#address-cells", 2)?;
    fdt.property_u32("#size-cells", 2)?;
    fdt.property_null("ranges")?;

    let gic_intr = [
        GIC_FDT_IRQ_TYPE_PPI,
        gic_device.fdt_maint_irq(),
        IRQ_TYPE_LEVEL_HI,
    ];

    fdt.property_array_u32("interrupts", &gic_intr)?;

    if let Some(msi_properties) = gic_device.msi_properties() {
        let msic_node = fdt.begin_node("msic")?;
        fdt.property_string("compatible", "arm,gic-v3-its")?;
        fdt.property_null("msi-controller")?;
        fdt.property_array_u64("reg", msi_properties)?;
        fdt.end_node(msic_node)?;
    }

    fdt.end_node(interrupt)?;

    Ok(())
}

fn create_clock_node(fdt: &mut FdtWriter) -> Result<(), FdtError> {
    // The Advanced Peripheral Bus (APB) is part of the Advanced Microcontroller Bus Architecture
    // (AMBA) protocol family. It defines a low-cost interface that is optimized for minimal power
    // consumption and reduced interface complexity.
    // PCLK is the clock source and this node defines exactly the clock for the APB.
    let clock = fdt.begin_node("apb-pclk")?;
    fdt.property_string("compatible", "fixed-clock")?;
    fdt.property_u32("#clock-cells", 0x0)?;
    fdt.property_u32("clock-frequency", 24_000_000)?;
    fdt.property_string("clock-output-names", "clk24mhz")?;
    fdt.property_u32("phandle", CLOCK_PHANDLE)?;
    fdt.end_node(clock)?;
    Ok(())
}

fn create_timer_node(fdt: &mut FdtWriter) -> Result<(), FdtError> {
    // See
    // https://github.com/torvalds/linux/blob/master/Documentation/devicetree/bindings/interrupt-controller/arch_timer.txt
    // These are fixed interrupt numbers for the timer device.
    let irqs = [13, 14, 11, 10];
    let compatible = "arm,armv8-timer";

    let mut timer_reg_cells: Vec<u32> = Vec::new();
    for &irq in irqs.iter() {
        timer_reg_cells.push(GIC_FDT_IRQ_TYPE_PPI);
        timer_reg_cells.push(irq);
        timer_reg_cells.push(IRQ_TYPE_LEVEL_HI);
    }

    let timer = fdt.begin_node("timer")?;
    fdt.property_string("compatible", compatible)?;
    fdt.property_null("always-on")?;
    fdt.property_array_u32("interrupts", &timer_reg_cells)?;
    fdt.end_node(timer)?;
    Ok(())
}

fn create_psci_node(fdt: &mut FdtWriter) -> Result<(), FdtError> {
    let compatible = "arm,psci-0.2";

    let psci = fdt.begin_node("psci")?;
    fdt.property_string("compatible", compatible)?;
    // Two methods available: hvc and smc.
    // As per documentation, PSCI calls between a guest and hypervisor may use the HVC conduit
    // instead of SMC. So, since we are using kvm, we need to use hvc.
    fdt.property_string("method", "hvc")?;
    fdt.end_node(psci)?;

    Ok(())
}

fn create_serial_node(fdt: &mut FdtWriter, dev_info: &SerialFdtInfo) -> Result<(), FdtError> {
    let serial = fdt.begin_node(&format!("uart@{:x}", dev_info.addr))?;

    fdt.property_string("compatible", "ns16550a")?;
    fdt.property_array_u64("reg", &[dev_info.addr, dev_info.len])?;
    fdt.property_u32("clocks", CLOCK_PHANDLE)?;
    fdt.property_string("clock-names", "apb_pclk")?;
    fdt.property_array_u32(
        "interrupts",
        &[GIC_FDT_IRQ_TYPE_SPI, dev_info.gsi, IRQ_TYPE_EDGE_RISING],
    )?;
    fdt.end_node(serial)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;
    use crate::arch::aarch64::InitrdConfig;
    use crate::vstate::memory::tests::arch_mem;
    use crate::vstate::memory::{GuestAddress, mib_to_bytes};

    /// A GICv3-shaped interrupt controller description, as the KVM backend
    /// builds it on a host with a GICv3 (the `VmFd`-free part of the FDT
    /// generator, which is why this test runs without KVM).
    struct FakeGicv3 {
        properties: [u64; 4],
    }

    impl GicFdt for FakeGicv3 {
        fn compatible(&self) -> &str {
            "arm,gic-v3"
        }

        fn device_properties(&self) -> &[u64] {
            &self.properties
        }

        fn fdt_maint_irq(&self) -> u32 {
            9
        }

        fn msi_properties(&self) -> Option<&[u64]> {
            None
        }
    }

    /// The FDT is written where the kernel finds it, at the end of DRAM, and
    /// describes exactly the nodes the guest needs to boot this port's device
    /// set: CPUs, memory, the chosen bootargs, the GIC, the arch timer, the
    /// APB clock, PSCI and the `ns16550a` console. The blob is also checked
    /// against the flattened-device-tree (DTB) header contract, which is what
    /// the guest's early boot code parses.
    #[test]
    fn create_fdt_describes_the_boot_devices() {
        let mem = arch_mem(mib_to_bytes(128));
        let gic = FakeGicv3 {
            properties: [
                layout::MMIO32_MEM_START - 0x1_0000,
                0x1_0000,
                layout::MMIO32_MEM_START - 0x1_0000 - 2 * 0x1_0000,
                2 * 0x1_0000,
            ],
        };
        let serial = SerialFdtInfo {
            addr: layout::SERIAL_MEM_START,
            len: layout::MMIO_LEN,
            gsi: layout::GSI_LEGACY_START,
        };
        let initrd = InitrdConfig {
            address: GuestAddress(0x1000_0000),
            size: 0x1000,
        };

        let dtb = create_fdt(
            &mem,
            vec![0],
            CString::new("console=ttyS0 panic=-1").unwrap(),
            &serial,
            &gic,
            &Some(initrd),
        )
        .unwrap();

        // DTB header: magic, total size, and the two required strings.
        assert_eq!(
            u32::from_be_bytes(dtb[0..4].try_into().unwrap()),
            0xd00d_feed
        );
        assert_eq!(
            u32::from_be_bytes(dtb[4..8].try_into().unwrap()) as usize,
            dtb.len()
        );
        assert!(dtb.len() <= layout::FDT_MAX_SIZE);

        // Every node and string property the guest consumes is present.
        let text = String::from_utf8_lossy(&dtb);
        for node in [
            "cpus",
            "cpu@0",
            "memory@ram",
            "chosen",
            "intc",
            "timer",
            "apb-pclk",
            "psci",
            "uart@40002000",
        ] {
            assert!(text.contains(node), "missing node {node}");
        }
        for property in [
            "linux,dummy-virt",
            "arm,arm-v8",
            "psci",
            "bootargs",
            "console=ttyS0 panic=-1",
            "linux,initrd-start",
            "linux,initrd-end",
            "arm,gic-v3",
            "arm,armv8-timer",
            "fixed-clock",
            "ns16550a",
        ] {
            assert!(text.contains(property), "missing property {property}");
        }
        // The serial console is the only MMIO device, and its window is
        // declared at the address the builder registers it on.
        assert!(text.contains(&format!("uart@{:x}", layout::SERIAL_MEM_START)));
        // No PCI and no MSI controller in this port.
        assert!(!text.contains("pci@"));
        assert!(!text.contains("arm,gic-v3-its"));
    }
}
