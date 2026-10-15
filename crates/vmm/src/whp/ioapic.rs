// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! Userspace model of the Intel 82093AA I/O APIC.
//!
//! The guest's MPTable declares an I/O APIC at [`IOAPIC_BASE`] that routes ISA
//! IRQ `i` to pin `i`. Windows Hypervisor Platform (WHP) emulates only the
//! local APIC, so this model decodes the guest's register accesses to the I/O
//! APIC and turns a device's interrupt line into the [`Interrupt`] the backend
//! should request from WHP (`WHvRequestInterrupt`). Level-triggered interrupts
//! are re-delivered after the guest's end-of-interrupt (EOI) while the line
//! stays asserted.
//!
//! The model performs no I/O itself and has no platform-specific code: the
//! caller feeds it MMIO accesses and line changes, and acts on the interrupts
//! it returns.

/// Guest-physical base address of the I/O APIC.
pub const IOAPIC_BASE: u64 = 0xfec0_0000;
/// Size of the I/O APIC MMIO window in bytes.
pub const IOAPIC_SIZE: u64 = 0x1000;
/// Number of interrupt input pins (redirection table entries).
pub const NUM_PINS: usize = 24;

/// MMIO offset of the register-select register (IOREGSEL).
const REG_SELECT: u64 = 0x00;
/// MMIO offset of the data window (IOWIN).
const REG_WINDOW: u64 = 0x10;

/// Indirect register index of the ID register.
const IND_ID: u32 = 0x00;
/// Indirect register index of the (read-only) VERSION register.
const IND_VERSION: u32 = 0x01;
/// Indirect register index of the (read-only) ARBITRATION register.
const IND_ARBITRATION: u32 = 0x02;
/// Indirect register index of the low dword of redirection entry 0.
const IND_REDIRECTION_BASE: u32 = 0x10;

/// VERSION register value: version 0x11, last redirection entry index in bits 16..23.
const VERSION: u32 = 0x11 | ((NUM_PINS as u32 - 1) << 16);

const RTE_VECTOR_MASK: u64 = 0xff;
const RTE_DELIVERY_SHIFT: u32 = 8;
const RTE_LOGICAL: u64 = 1 << 11;
// Bit 12 (delivery status) is always zero in this model, so it is not named.
const RTE_POLARITY: u64 = 1 << 13;
const RTE_REMOTE_IRR: u64 = 1 << 14;
const RTE_LEVEL: u64 = 1 << 15;
const RTE_MASKED: u64 = 1 << 16;
const RTE_DEST_SHIFT: u32 = 56;
const RTE_DEST_MASK: u64 = 0xff << RTE_DEST_SHIFT;
/// Bits of the low dword that the guest may write.
const RTE_LOW_WRITABLE: u64 = RTE_VECTOR_MASK
    | (0x7 << RTE_DELIVERY_SHIFT)
    | RTE_LOGICAL
    | RTE_POLARITY
    | RTE_LEVEL
    | RTE_MASKED;

/// Interrupt delivery mode of a redirection entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Deliver the vector to the destination as a fixed interrupt.
    Fixed,
    /// Deliver to the lowest-priority processor among the destinations.
    LowestPriority,
    /// System management interrupt.
    Smi,
    /// Non-maskable interrupt.
    Nmi,
    /// INIT signal.
    Init,
    /// External interrupt, acknowledged through the INTA cycle.
    ExtInt,
}

impl DeliveryMode {
    /// Decodes the 3-bit delivery mode field. Reserved encodings act as `Fixed`.
    fn from_bits(bits: u64) -> Self {
        match bits & 0x7 {
            1 => Self::LowestPriority,
            2 => Self::Smi,
            4 => Self::Nmi,
            5 => Self::Init,
            7 => Self::ExtInt,
            _ => Self::Fixed,
        }
    }
}

/// An interrupt the backend should request from WHP, decoded from a redirection entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interrupt {
    /// Interrupt vector (RTE bits 0..7).
    pub vector: u8,
    /// Delivery mode (RTE bits 8..10).
    pub delivery: DeliveryMode,
    /// Destination field (RTE bits 56..63).
    pub destination: u8,
    /// Destination mode: `true` for logical, `false` for physical (RTE bit 11).
    pub logical: bool,
    /// Trigger mode: `true` for level-triggered (RTE bit 15).
    pub level_triggered: bool,
}

/// Decodes a redirection entry into the interrupt it describes.
fn decode(rte: u64) -> Interrupt {
    Interrupt {
        vector: (rte & RTE_VECTOR_MASK) as u8,
        delivery: DeliveryMode::from_bits(rte >> RTE_DELIVERY_SHIFT),
        destination: (rte >> RTE_DEST_SHIFT) as u8,
        logical: (rte & RTE_LOGICAL) != 0,
        level_triggered: (rte & RTE_LEVEL) != 0,
    }
}

/// Maps an indirect register index to `(pin, is_high_dword)` in the redirection table.
fn redirection_slot(index: u32) -> Option<(usize, bool)> {
    let offset = index.checked_sub(IND_REDIRECTION_BASE)?;
    let pin = (offset / 2) as usize;
    if pin < NUM_PINS {
        Some((pin, offset % 2 == 1))
    } else {
        None
    }
}

/// Software model of one Intel 82093AA I/O APIC. It is its own snapshot state: every field is
/// guest-visible register or line state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IoApic {
    id: u8,
    regsel: u32,
    redirection: [u64; NUM_PINS],
    line: [bool; NUM_PINS],
}

impl IoApic {
    /// Creates an I/O APIC with the given ID (low 4 bits used) and every redirection entry masked.
    pub fn new(id: u8) -> Self {
        Self {
            id: id & 0x0f,
            regsel: 0,
            redirection: [RTE_MASKED; NUM_PINS],
            line: [false; NUM_PINS],
        }
    }

    /// Handles a guest read of `data.len()` bytes at `offset` from [`IOAPIC_BASE`].
    ///
    /// Reads fill `data` from the little-endian bytes of the 32-bit register
    /// value; bytes beyond the fourth are zero. Unknown offsets read as zero.
    pub fn mmio_read(&self, offset: u64, data: &mut [u8]) {
        let value = match offset {
            REG_SELECT => self.regsel,
            REG_WINDOW => self.read_indirect(self.regsel),
            _ => 0,
        };
        let bytes = value.to_le_bytes();
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = bytes.get(i).copied().unwrap_or(0);
        }
    }

    /// Handles a guest write at `offset` from [`IOAPIC_BASE`].
    ///
    /// Writes shorter than four bytes are zero-extended. Returns the interrupts
    /// to request from WHP as a result of the write. Unknown offsets are ignored.
    pub fn mmio_write(&mut self, offset: u64, data: &[u8]) -> Vec<Interrupt> {
        let mut bytes = [0u8; 4];
        for (dst, src) in bytes.iter_mut().zip(data) {
            *dst = *src;
        }
        let value = u32::from_le_bytes(bytes);
        match offset {
            REG_SELECT => {
                self.regsel = value & 0xff;
                Vec::new()
            }
            REG_WINDOW => self.write_indirect(self.regsel, value),
            _ => Vec::new(),
        }
    }

    /// Asserts or deasserts an interrupt input pin.
    ///
    /// Returns the interrupt to request from WHP, if this line change causes one.
    /// Pins at or above [`NUM_PINS`] are rejected with `None`.
    pub fn set_irq(&mut self, pin: usize, level: bool) -> Option<Interrupt> {
        if pin >= NUM_PINS {
            return None;
        }
        let was_high = std::mem::replace(&mut self.line[pin], level);
        let rte = self.redirection[pin];
        if (rte & RTE_MASKED) != 0 {
            return None;
        }
        if (rte & RTE_LEVEL) != 0 {
            if level && (rte & RTE_REMOTE_IRR) == 0 {
                Some(self.deliver_level(pin))
            } else {
                None
            }
        } else if level && !was_high {
            Some(decode(rte))
        } else {
            None
        }
    }

    /// Pulses an interrupt input pin high then low.
    ///
    /// Returns the delivery caused by the rising edge.
    pub fn pulse_irq(&mut self, pin: usize) -> Option<Interrupt> {
        let delivery = self.set_irq(pin, true);
        self.set_irq(pin, false);
        delivery
    }

    /// Handles the guest's end-of-interrupt for `vector`.
    ///
    /// Each level-triggered entry with that vector and remote IRR set has its
    /// remote IRR cleared. If its line is still asserted and the pin unmasked,
    /// the interrupt is delivered again. Returns all re-deliveries.
    pub fn end_of_interrupt(&mut self, vector: u8) -> Vec<Interrupt> {
        let mut out = Vec::new();
        for pin in 0..NUM_PINS {
            let rte = self.redirection[pin];
            let matches = (rte & RTE_LEVEL) != 0
                && (rte & RTE_VECTOR_MASK) as u8 == vector
                && (rte & RTE_REMOTE_IRR) != 0;
            if !matches {
                continue;
            }
            self.redirection[pin] &= !RTE_REMOTE_IRR;
            if self.line[pin] && (self.redirection[pin] & RTE_MASKED) == 0 {
                out.push(self.deliver_level(pin));
            }
        }
        out
    }

    /// Returns the raw 64-bit redirection entry for `pin`, or `None` if out of range.
    pub fn redirection_entry(&self, pin: usize) -> Option<u64> {
        self.redirection.get(pin).copied()
    }

    /// Reads an indirect register selected through IOREGSEL.
    fn read_indirect(&self, index: u32) -> u32 {
        match index {
            IND_ID | IND_ARBITRATION => u32::from(self.id) << 24,
            IND_VERSION => VERSION,
            _ => match redirection_slot(index) {
                Some((pin, true)) => (self.redirection[pin] >> 32) as u32,
                Some((pin, false)) => self.redirection[pin] as u32,
                None => 0,
            },
        }
    }

    /// Writes an indirect register selected through IOREGSEL.
    fn write_indirect(&mut self, index: u32, value: u32) -> Vec<Interrupt> {
        if index == IND_ID {
            self.id = ((value >> 24) & 0x0f) as u8;
            return Vec::new();
        }
        let (pin, high) = match redirection_slot(index) {
            Some(slot) => slot,
            None => return Vec::new(),
        };
        let rte = &mut self.redirection[pin];
        if high {
            *rte = (*rte & 0xffff_ffff) | ((u64::from(value) << 32) & RTE_DEST_MASK);
        } else {
            // Remote IRR is read-only to the guest and delivery status is always 0.
            let remote_irr = *rte & RTE_REMOTE_IRR;
            *rte =
                (*rte & 0xffff_ffff_0000_0000) | remote_irr | (u64::from(value) & RTE_LOW_WRITABLE);
        }
        self.deliver_on_write(pin).into_iter().collect()
    }

    /// Delivers a just-written entry if it is an unmasked, asserted, level-triggered
    /// interrupt that is not already awaiting EOI.
    fn deliver_on_write(&mut self, pin: usize) -> Option<Interrupt> {
        let rte = self.redirection[pin];
        let deliverable = (rte & RTE_MASKED) == 0
            && (rte & RTE_LEVEL) != 0
            && self.line[pin]
            && (rte & RTE_REMOTE_IRR) == 0;
        if deliverable {
            Some(self.deliver_level(pin))
        } else {
            None
        }
    }

    /// Marks a level-triggered entry as awaiting EOI and returns its interrupt.
    fn deliver_level(&mut self, pin: usize) -> Interrupt {
        self.redirection[pin] |= RTE_REMOTE_IRR;
        decode(self.redirection[pin])
    }
}

impl Default for IoApic {
    /// Equivalent to [`IoApic::new`] with ID 0.
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selects indirect register `index` and writes `value` to it.
    fn write_reg(io: &mut IoApic, index: u32, value: u32) -> Vec<Interrupt> {
        io.mmio_write(REG_SELECT, &index.to_le_bytes());
        io.mmio_write(REG_WINDOW, &value.to_le_bytes())
    }

    /// Selects indirect register `index` and reads it back.
    fn read_reg(io: &mut IoApic, index: u32) -> u32 {
        io.mmio_write(REG_SELECT, &index.to_le_bytes());
        let mut buf = [0u8; 4];
        io.mmio_read(REG_WINDOW, &mut buf);
        u32::from_le_bytes(buf)
    }

    /// Indirect register index of the low dword of redirection entry `pin`.
    fn rte_index(pin: usize) -> u32 {
        IND_REDIRECTION_BASE + 2 * pin as u32
    }

    /// Writes both dwords of redirection entry `pin`.
    fn write_rte(io: &mut IoApic, pin: usize, value: u64) -> Vec<Interrupt> {
        let mut out = write_reg(io, rte_index(pin), value as u32);
        out.extend(write_reg(io, rte_index(pin) + 1, (value >> 32) as u32));
        out
    }

    #[test]
    fn version_register_reports_24_pins() {
        let mut io = IoApic::new(0);
        assert_eq!(read_reg(&mut io, 0x01), 0x0017_0011);
    }

    #[test]
    fn id_register_round_trip() {
        let mut io = IoApic::new(3);
        assert_eq!(read_reg(&mut io, 0x00), 0x0300_0000);
        assert_eq!(read_reg(&mut io, 0x02), 0x0300_0000);

        write_reg(&mut io, 0x00, 0x0700_0000);
        assert_eq!(read_reg(&mut io, 0x00), 0x0700_0000);
        assert_eq!(read_reg(&mut io, 0x02), 0x0700_0000);

        // Only bits 24..27 are stored.
        write_reg(&mut io, 0x00, 0xff00_0000);
        assert_eq!(read_reg(&mut io, 0x00), 0x0f00_0000);
    }

    #[test]
    fn redirection_round_trip_ignores_read_only_bits() {
        let mut io = IoApic::new(0);
        // Guest tries to set delivery status (bit 12) and remote IRR (bit 14).
        write_reg(&mut io, rte_index(3), 0x0000_F92D);
        write_reg(&mut io, rte_index(3) + 1, 0x3CFF_FFFF);

        assert_eq!(read_reg(&mut io, rte_index(3)), 0x0000_A92D);
        assert_eq!(read_reg(&mut io, rte_index(3) + 1), 0x3C00_0000);
        assert_eq!(io.redirection_entry(3), Some(0x3C00_0000_0000_A92D));
    }

    #[test]
    fn remote_irr_is_kept_across_guest_writes() {
        let mut io = IoApic::new(0);
        write_reg(&mut io, rte_index(4), 0x0000_8040);
        assert!(io.set_irq(4, true).is_some());

        // Guest clears nothing and sets bit 12; remote IRR stays set, so no delivery.
        let out = write_reg(&mut io, rte_index(4), 0x0000_9040);
        assert!(out.is_empty());
        assert_eq!(read_reg(&mut io, rte_index(4)), 0x0000_C040);
    }

    #[test]
    fn masked_pin_delivers_nothing() {
        let mut io = IoApic::new(0);
        assert_eq!(io.redirection_entry(0), Some(1 << 16));
        assert_eq!(io.set_irq(0, true), None);
        assert_eq!(io.pulse_irq(0), None);
        assert_eq!(io.set_irq(0, false), None);
    }

    #[test]
    fn edge_pin_delivers_once_per_rising_edge() {
        let mut io = IoApic::new(0);
        write_rte(&mut io, 1, 0x31);

        let first = io.pulse_irq(1).expect("first pulse delivers");
        assert_eq!(first.vector, 0x31);
        assert!(!first.level_triggered);
        assert!(io.pulse_irq(1).is_some());

        // Holding the line high is a single rising edge.
        assert_eq!(io.set_irq(1, true).map(|i| i.vector), Some(0x31));
        assert_eq!(io.set_irq(1, true), None);
        io.set_irq(1, false);

        // Edge interrupts are not tracked for EOI.
        assert!(io.end_of_interrupt(0x31).is_empty());
    }

    #[test]
    fn level_pin_delivers_once_until_eoi() {
        let mut io = IoApic::new(0);
        write_rte(&mut io, 2, 0x32 | (1 << 15));

        let first = io.set_irq(2, true).expect("first assertion delivers");
        assert_eq!(first.vector, 0x32);
        assert!(first.level_triggered);
        assert_eq!(io.set_irq(2, true), None);

        // EOI for another vector does not affect this pin.
        assert!(io.end_of_interrupt(0x99).is_empty());

        // EOI while still asserted re-delivers.
        let redelivered = io.end_of_interrupt(0x32);
        assert_eq!(redelivered.len(), 1);
        assert_eq!(redelivered[0].vector, 0x32);

        // Deassert, then EOI: nothing further, and remote IRR is cleared.
        assert_eq!(io.set_irq(2, false), None);
        assert!(io.end_of_interrupt(0x32).is_empty());
        assert_eq!(io.redirection_entry(2).map(|e| e & (1 << 14)), Some(0));

        // A new assertion delivers again.
        assert!(io.set_irq(2, true).is_some());
    }

    #[test]
    fn unmasking_level_pin_with_line_high_delivers() {
        let mut io = IoApic::new(0);
        // Masked, level-triggered, vector 0x35.
        write_rte(&mut io, 5, 0x0001_8035);
        assert_eq!(io.set_irq(5, true), None);

        let delivered = write_reg(&mut io, rte_index(5), 0x0000_8035);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].vector, 0x35);
        assert!(delivered[0].level_triggered);

        // The high-dword write does not deliver again while awaiting EOI.
        assert!(write_reg(&mut io, rte_index(5) + 1, 0).is_empty());
    }

    #[test]
    fn out_of_range_pin_is_rejected() {
        let mut io = IoApic::new(0);
        assert_eq!(io.set_irq(NUM_PINS, true), None);
        assert_eq!(io.pulse_irq(NUM_PINS + 5), None);
        assert_eq!(io.redirection_entry(NUM_PINS), None);
        // Redirection indices beyond the table read as zero.
        assert_eq!(read_reg(&mut io, rte_index(NUM_PINS)), 0);
    }

    #[test]
    fn delivery_fields_decode() {
        let mut io = IoApic::new(0);
        // Vector 0x41, LowestPriority (mode 1), logical destination mode, destination 0xff.
        let value: u64 = 0x41 | (1 << 8) | (1 << 11) | (0xff << 56);
        write_rte(&mut io, 7, value);
        assert_eq!(
            io.pulse_irq(7),
            Some(Interrupt {
                vector: 0x41,
                delivery: DeliveryMode::LowestPriority,
                destination: 0xff,
                logical: true,
                level_triggered: false,
            })
        );

        // Mode 4 is NMI; reserved mode 6 acts as Fixed.
        write_rte(&mut io, 8, 4 << 8);
        assert_eq!(io.pulse_irq(8).map(|i| i.delivery), Some(DeliveryMode::Nmi));
        write_rte(&mut io, 9, 6 << 8);
        assert_eq!(
            io.pulse_irq(9).map(|i| i.delivery),
            Some(DeliveryMode::Fixed)
        );
    }

    #[test]
    fn narrow_and_unknown_accesses() {
        let mut io = IoApic::new(0);
        // A one-byte write is zero-extended, selecting VERSION.
        io.mmio_write(REG_SELECT, &[0x01]);

        let mut two = [0xffu8; 2];
        io.mmio_read(REG_WINDOW, &mut two);
        assert_eq!(two, [0x11, 0x00]);

        // Bytes beyond the fourth read as zero.
        let mut eight = [0xffu8; 8];
        io.mmio_read(REG_WINDOW, &mut eight);
        assert_eq!(eight, [0x11, 0x00, 0x17, 0x00, 0, 0, 0, 0]);

        // Unknown offsets read zero and ignore writes.
        let mut junk = [0xffu8; 4];
        io.mmio_read(0x20, &mut junk);
        assert_eq!(junk, [0; 4]);
        assert!(io.mmio_write(0x20, &[0xff; 4]).is_empty());
        assert_eq!(read_reg(&mut io, 0x01), 0x0017_0011);
    }
}
