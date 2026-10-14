// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/acpi/{mod.rs,x86_64.rs} (trimmed: the
// `DeviceManager` argument is dropped — this port has no device manager, and
// the only DSDT devices are the serial console, the i8042 controller and the
// `_S5` object — and the `MCFG` table is dropped because there is no PCI
// MMCONFIG; the FADT additionally gains the sleep control/status registers
// that upstream leaves zero, because this port supports guest S5 poweroff).
//
// Upstream Firecracker builds ACPI tables but declares no `_S5`, so a guest
// `poweroff` leaves the VMM alive. This port adds the missing piece: a
// HW-reduced FADT whose SLEEP_CONTROL_REG points at the PM1-control port
// (0x604, emulated by [`crate::devices::legacy::pm1_cnt::Pm1CntDevice`]) and
// a `_S5` object in the DSDT, so `reboot(RB_POWER_OFF)` reaches that device
// and stops the VM cleanly (reset=false).

//! ACPI table generation for `x86_64` guests.

use acpi_tables::aml::{self, Aml};
use acpi_tables::fadt::{
    FADT_F_HW_REDUCED_ACPI, FADT_F_PWR_BUTTON, FADT_F_RESET_REGISTER, FADT_F_SLP_BUTTON,
    IAPC_BOOT_ARG_FLAGS_VGA_NOT_PRESENT,
};
use acpi_tables::madt::LocalAPIC;
use acpi_tables::{Dsdt, Fadt, GenericAddressStructure, Madt, Rsdp, Sdt, Xsdt};
use zerocopy::IntoBytes;

use crate::arch::x86_64::layout;
use crate::devices::legacy::{RESET_PORT, RESET_VALUE, SLEEP_CONTROL_PORT, SLEEP_STATUS_PORT};
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap, usize_to_u64};
use crate::vstate::resources::{AllocPolicy, ResourceAllocator};

// Our Original Equipment Manufacturer name, passed in every table (upstream
// uses the same value).
const OEM_ID: [u8; 6] = *b"FIRECK";

// Per-table OEM revision. Upstream sets this to a fixed value for all tables.
const OEM_REVISION: u32 = 0;

// FADT entry that lets the guest know it runs inside a microVM.
const HYPERVISOR_VENDOR_ID: [u8; 8] = *b"FIRECKVM";

// Legacy device wiring, mirroring upstream's PortIODeviceManager.
const COM1_GSI: u32 = 4;
const KBD_EVT_GSI: u32 = 1;
const SERIAL_PORT_ADDRESS: u64 = 0x3f8;
const SERIAL_PORT_SIZE: u64 = 0x8;
const I8042_KDB_DATA_REGISTER_ADDRESS: u64 = 0x060;

/// Error type for ACPI table generation.
#[derive(Debug, thiserror::Error)]
pub enum AcpiError {
    /// Failed to allocate guest memory for an ACPI table: {0}
    #[error("failed to allocate guest memory for an ACPI table: {0}")]
    Alloc(#[from] crate::vstate::resources::Error),
    /// Failed to write an ACPI table to guest memory: {0}
    #[error("failed to write an ACPI table to guest memory: {0}")]
    Table(#[from] acpi_tables::AcpiError),
    /// Failed to build AML bytecode: {0}
    #[error("failed to build AML bytecode: {0}")]
    Aml(#[from] aml::AmlError),
}

/// Writes `table` into guest memory at an address allocated from the system
/// data region, returning that address.
fn write_acpi_table<S: Sdt>(
    mem: &GuestMemoryMmap,
    resource_allocator: &mut ResourceAllocator,
    table: &mut S,
) -> Result<u64, AcpiError> {
    let addr = *resource_allocator
        .system_memory
        .allocate(usize_to_u64(table.len()), 1, AllocPolicy::FirstMatch)?
        .start();

    table.write_to_guest(mem, GuestAddress(addr))?;
    Ok(addr)
}

/// Builds the DSDT definition block: the serial console (`_SB_.COM1`), the
/// i8042 controller (`_SB_.PS2_`) and the `_S5` object that makes the guest
/// kernel register ACPI poweroff.
fn build_dsdt_data() -> Result<Vec<u8>, aml::AmlError> {
    let mut dsdt_data = Vec::new();

    // Serial console (COM1).
    aml::Device::new(
        "_SB_.COM1".try_into()?,
        vec![
            &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0501")?)?,
            &aml::Name::new("_UID".try_into()?, &0u8)?,
            &aml::Name::new("_DDN".try_into()?, &"COM1")?,
            &aml::Name::new(
                "_CRS".try_into()?,
                &aml::ResourceTemplate::new(vec![
                    &aml::Interrupt::new(true, true, false, false, COM1_GSI),
                    &aml::Io::new(
                        SERIAL_PORT_ADDRESS.try_into().unwrap(),
                        SERIAL_PORT_ADDRESS.try_into().unwrap(),
                        1,
                        SERIAL_PORT_SIZE.try_into().unwrap(),
                    ),
                ]),
            )?,
        ],
    )
    .append_aml_bytes(&mut dsdt_data)?;

    // i8042 keyboard controller (PS/2). The command port is faked so Linux
    // stops complaining; the data port is the device we actually emulate.
    aml::Device::new(
        "_SB_.PS2_".try_into()?,
        vec![
            &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0303")?)?,
            &aml::Method::new(
                "_STA".try_into()?,
                0,
                false,
                vec![&aml::Return::new(&0x0fu8)],
            ),
            &aml::Name::new(
                "_CRS".try_into()?,
                &aml::ResourceTemplate::new(vec![
                    &aml::Io::new(
                        I8042_KDB_DATA_REGISTER_ADDRESS.try_into().unwrap(),
                        I8042_KDB_DATA_REGISTER_ADDRESS.try_into().unwrap(),
                        1,
                        1,
                    ),
                    // Fake a command port so Linux stops complaining.
                    &aml::Io::new(0x0064, 0x0064, 1, 1),
                    &aml::Interrupt::new(true, true, false, false, KBD_EVT_GSI),
                ]),
            )?,
        ],
    )
    .append_aml_bytes(&mut dsdt_data)?;

    // _S5: S5 system state (sleep type 5). This is what makes the guest
    // kernel register `acpi_power_off` as the `pm_power_off` handler, so a
    // `reboot(RB_POWER_OFF)` writes SLP_EN to the sleep control register.
    aml::Name::new("_S5_".try_into()?, &aml::Package::new(vec![&5u8]))?
        .append_aml_bytes(&mut dsdt_data)?;

    Ok(dsdt_data)
}

fn build_dsdt(
    mem: &GuestMemoryMmap,
    resource_allocator: &mut ResourceAllocator,
) -> Result<u64, AcpiError> {
    let mut dsdt = Dsdt::new(OEM_ID, *b"FCVMDSDT", OEM_REVISION, build_dsdt_data()?);
    write_acpi_table(mem, resource_allocator, &mut dsdt)
}

fn build_fadt(
    mem: &GuestMemoryMmap,
    resource_allocator: &mut ResourceAllocator,
    dsdt_addr: u64,
) -> Result<u64, AcpiError> {
    let mut fadt = Fadt::new(OEM_ID, *b"FCVMFADT", OEM_REVISION);
    fadt.set_hypervisor_vendor_id(HYPERVISOR_VENDOR_ID);
    fadt.set_x_dsdt(dsdt_addr);
    // Let the guest kernel know there is no VGA hardware present (upstream
    // sets the same flag; without it the kernel may probe for video).
    fadt.setup_iapc_flags(1 << IAPC_BOOT_ARG_FLAGS_VGA_NOT_PRESENT);
    fadt.set_flags(
        (1 << FADT_F_HW_REDUCED_ACPI)
            | (1 << FADT_F_PWR_BUTTON)
            | (1 << FADT_F_SLP_BUTTON)
            | (1 << FADT_F_RESET_REGISTER),
    );
    // Reset register: a guest reboot writes RESET_VALUE to this port, which
    // the PM1-control device turns into a clean reset (reset=true). Without
    // it, the reboot falls through to the BIOS real-mode jump and
    // double-faults on this KVM port.
    fadt.set_reset_reg(GenericAddressStructure::new(
        1, // SystemIO
        8, // bit width
        0, 1, // byte access
        RESET_PORT,
    ));
    fadt.set_reset_value(RESET_VALUE);
    // HW-reduced S5: point the sleep control/status registers at the ports the
    // PM1-control device emulates. Both are byte-wide SystemIO registers; the
    // sleep control register carries SLP_TYP (bits 4:2) and SLP_EN (bit 5),
    // and the sleep status register carries WAK_STS (bit 7).
    fadt.set_sleep_control_reg(GenericAddressStructure::new(
        1, // SystemIO
        8, // bit width
        0,
        1, // byte access
        SLEEP_CONTROL_PORT,
    ));
    fadt.set_sleep_status_reg(GenericAddressStructure::new(
        1, // SystemIO
        8, // bit width
        0,
        1, // byte access
        SLEEP_STATUS_PORT,
    ));
    write_acpi_table(mem, resource_allocator, &mut fadt)
}

/// The MADT declares only the local APICs. It deliberately carries **no**
/// I/O-APIC entry: the I/O-APIC and the legacy ISA IRQ routing live in the
/// MPTable this port also writes (`mptable::setup_mptable`), whose
/// `mpc_intsrc` records map ISA IRQ `i` -> GSI `i` for the whole 0..=23
/// legacy range. If the MADT instead declared an I/O-APIC, the guest kernel
/// would switch to ACPI (MADT) interrupt routing, which on this kernel
/// (7.2.8) does not populate the legacy ISA IRQ mapping the MPTable's
/// `mpc_intsrc` records provide — `acpi_register_gsi` then hands the DSDT
/// devices fresh vectors (serial COM1's GSI 4 -> irq 24) and leaves the raw
/// Linux IRQ a `virtio_mmio.device=...:<irq>` transport requests (the vsock
/// device, IRQ 5) unallocated, so its `request_irq` fails with `-EINVAL` and
/// the virtio-vsock probe aborts. Omitting the I/O-APIC entry keeps the
/// kernel on the MPTable's routing and the IRQ numbers stable.
fn build_madt(
    mem: &GuestMemoryMmap,
    resource_allocator: &mut ResourceAllocator,
    vcpu_count: u8,
) -> Result<u64, AcpiError> {
    let mut interrupt_controllers =
        Vec::with_capacity((vcpu_count as usize) * std::mem::size_of::<LocalAPIC>());
    for i in 0..vcpu_count {
        interrupt_controllers.extend_from_slice(LocalAPIC::new(i).as_bytes());
    }

    let mut madt = Madt::new(
        OEM_ID,
        *b"FCVMMADT",
        OEM_REVISION,
        layout::APIC_ADDR,
        interrupt_controllers,
    );
    write_acpi_table(mem, resource_allocator, &mut madt)
}

fn build_xsdt(
    mem: &GuestMemoryMmap,
    resource_allocator: &mut ResourceAllocator,
    fadt_addr: u64,
    madt_addr: u64,
) -> Result<u64, AcpiError> {
    let mut xsdt = Xsdt::new(
        OEM_ID,
        *b"FCMVXSDT",
        OEM_REVISION,
        vec![fadt_addr, madt_addr],
    );
    write_acpi_table(mem, resource_allocator, &mut xsdt)
}

fn build_rsdp(mem: &GuestMemoryMmap, xsdt_addr: u64) -> Result<(), AcpiError> {
    let mut rsdp = Rsdp::new(OEM_ID, xsdt_addr);
    rsdp.write_to_guest(mem, GuestAddress(layout::RSDP_ADDR))?;
    Ok(())
}

/// Builds the guest's ACPI tables (RSDP, XSDT, FADT, MADT, DSDT) and writes
/// them into guest memory. The RSDP is pinned at [`layout::RSDP_ADDR`]; the
/// other tables are allocated from the system data region, which the MPTable
/// also draws from (so the caller must have already written the MPTable).
pub fn create_acpi_tables(
    mem: &GuestMemoryMmap,
    resource_allocator: &mut ResourceAllocator,
    vcpu_count: u8,
) -> Result<(), AcpiError> {
    let dsdt_addr = build_dsdt(mem, resource_allocator)?;
    let fadt_addr = build_fadt(mem, resource_allocator, dsdt_addr)?;
    let madt_addr = build_madt(mem, resource_allocator, vcpu_count)?;
    let xsdt_addr = build_xsdt(mem, resource_allocator, fadt_addr, madt_addr)?;
    build_rsdp(mem, xsdt_addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vstate::memory::mib_to_bytes;
    use crate::vstate::memory::tests::arch_mem;
    use crate::vstate::resources::ResourceAllocator;
    use vm_memory::Bytes;

    /// The DSDT contains the canonical 10-byte `_S5` blob (ACPI name `_S5_`
    /// holding a Package with the S5 sleep type 5). This is the object that
    /// turns guest `poweroff` into an ACPI S5 write.
    #[test]
    fn dsdt_contains_s5_object() {
        let data = build_dsdt_data().unwrap();
        let s5 = [0x08, 0x5F, 0x53, 0x35, 0x5F, 0x12, 0x04, 0x01, 0x0A, 0x05];
        assert!(
            data.windows(s5.len()).any(|w| w == s5),
            "DSDT AML does not contain the _S5 object"
        );
    }

    fn read_bytes(mem: &GuestMemoryMmap, addr: u64, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        mem.read_slice(&mut buf, GuestAddress(addr)).unwrap();
        buf
    }

    fn u32_at(bytes: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], off: usize) -> u64 {
        u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
    }

    /// The RSDP is pinned at `layout::RSDP_ADDR`, and the pointer chain
    /// RSDP -> XSDT -> {FADT, MADT} and FADT -> DSDT resolves, with the FADT
    /// HW-reduced and its sleep control/status registers pointing at the
    /// PM1-control ports. This is the "tables land at the right addresses with
    /// a correct chain" acceptance check.
    #[test]
    fn acpi_tables_chain_and_rsdp_placement() {
        let mem = arch_mem(mib_to_bytes(128));
        let mut resource_allocator = ResourceAllocator::new();
        create_acpi_tables(&mem, &mut resource_allocator, 2).unwrap();

        // RSDP is pinned at layout::RSDP_ADDR, revision 2.
        let rsdp = read_bytes(&mem, layout::RSDP_ADDR, 36);
        assert_eq!(&rsdp[0..8], b"RSD PTR ");
        assert_eq!(rsdp[15], 2);

        // Follow RSDP -> XSDT (header + two 64-bit pointers).
        let xsdt_addr = u64_at(&rsdp, 24);
        let xsdt = read_bytes(&mem, xsdt_addr, 52);
        assert_eq!(&xsdt[0..4], b"XSDT");
        let fadt_addr = u64_at(&xsdt, 36);
        let madt_addr = u64_at(&xsdt, 44);

        // FADT: HW-reduced flag set; sleep control/status registers at the
        // PM1-control ports. The GenericAddressStructure for SLEEP_CONTROL_REG
        // starts at FADT offset 244 (space id, bit width, bit offset, access
        // size, then the 8-byte little-endian address).
        let fadt = read_bytes(&mem, fadt_addr, 276);
        assert_eq!(&fadt[0..4], b"FACP");
        let flags = u32_at(&fadt, 112);
        assert_ne!(flags & (1 << FADT_F_HW_REDUCED_ACPI), 0);
        // RESET_REGISTER flag and RESET_REG at the reset port.
        assert_ne!(flags & (1 << FADT_F_RESET_REGISTER), 0);
        assert_eq!(fadt[116], 1); // SystemIO (RESET_REG space id)
        assert_eq!(u64_at(&fadt, 120), RESET_PORT);
        assert_eq!(fadt[128], RESET_VALUE);
        assert_eq!(fadt[244], 1); // SystemIO (SLEEP_CONTROL_REG)
        assert_eq!(fadt[245], 8); // bit width
        assert_eq!(u64_at(&fadt, 248), SLEEP_CONTROL_PORT);
        assert_eq!(fadt[256], 1); // SystemIO (SLEEP_STATUS_REG at offset 256)
        assert_eq!(u64_at(&fadt, 260), SLEEP_STATUS_PORT);

        // MADT present.
        assert_eq!(&read_bytes(&mem, madt_addr, 4)[..], b"APIC");

        // DSDT reachable via the FADT x_dsdt pointer, and it carries the _S5
        // object.
        let dsdt_addr = u64_at(&fadt, 140);
        let dsdt_header = read_bytes(&mem, dsdt_addr, 36);
        assert_eq!(&dsdt_header[0..4], b"DSDT");
        let dsdt = read_bytes(&mem, dsdt_addr, u32_at(&dsdt_header, 4) as usize);
        let s5 = [0x08, 0x5F, 0x53, 0x35, 0x5F, 0x12, 0x04, 0x01, 0x0A, 0x05];
        assert!(
            dsdt.windows(s5.len()).any(|w| w == s5),
            "DSDT does not contain the _S5 object"
        );
    }
}
